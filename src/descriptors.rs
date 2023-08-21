use std::marker::PhantomData;
use std::slice::{ChunksExact, ChunksExactMut};

use anyhow::{ensure, Context, Result};
use libvfio_user::dma::DmaMapping;
use packed_struct::derive::PackedStruct;
use packed_struct::PackedStruct;

use crate::util::is_all_zeros;
use crate::E1000;

// Each descriptor is 16 bytes long, 8 for buffer address, rest for status, length, etc...
const DESCRIPTOR_LENGTH: usize = 16;

// Size of each descriptor's buffer is automatically determined by distance between buffer pointers,
// fallback size for when there is only one descriptor
const DESCRIPTOR_BUFFER_FALLBACK_SIZE: u64 = 1920; // Default size linux kernel driver uses

#[derive(Debug)]
pub struct DescriptorRing<T> {
    mapping: DmaMapping,
    length: usize,
    pub head: usize, // Managed by structure
    pub tail: usize, // Updated by client
    phantom: PhantomData<T>,
}

impl<T> DescriptorRing<T>
where
    T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]>,
{
    fn ring_chunks(&self) -> ChunksExact<'_, u8> {
        self.mapping.dma(0).chunks_exact(DESCRIPTOR_LENGTH)
    }

    fn ring_chunks_mut(&mut self) -> ChunksExactMut<'_, u8> {
        self.mapping.dma_mut(0).chunks_exact_mut(DESCRIPTOR_LENGTH)
    }

    fn read_descriptor(&self, index: usize) -> Result<T> {
        let buffer = self
            .ring_chunks()
            .nth(index)
            .context("Descriptor not in mapping")?;

        let mut data = [0u8; DESCRIPTOR_LENGTH];
        data.copy_from_slice(buffer);
        data.reverse(); // Reverse because of endianness

        let descriptor = T::unpack(&data)?;
        Ok(descriptor)
    }

    fn read_all_descriptors(&self) -> Result<Vec<T>> {
        let mut descriptors = vec![];

        for chunk in self.ring_chunks() {
            let mut buffer = [0u8; DESCRIPTOR_LENGTH];
            buffer.copy_from_slice(chunk);
            buffer.reverse(); // Reverse because of endianness

            descriptors.push(T::unpack(&buffer)?);
        }
        Ok(descriptors)
    }

    fn write_descriptor(&mut self, desc: T, index: usize) -> Result<()> {
        let buffer = self
            .ring_chunks_mut()
            .nth(index)
            .context("Descriptor not in mapping")?;

        let mut data = desc.pack()?;
        data.reverse(); // Reverse because of endianness

        buffer.copy_from_slice(data.as_slice());
        Ok(())
    }

    // Is the section owned by hardware empty?
    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    pub fn advance_head(&mut self) {
        self.head = (self.head + 1) % self.length;
    }

    pub fn read_head(&self) -> Result<T> {
        ensure!(
            !self.is_empty(),
            "Cannot read head, head is currently owned by software"
        );
        self.read_descriptor(self.head)
    }

    pub fn write_and_advance_head(&mut self, desc: T) -> Result<()> {
        ensure!(
            !self.is_empty(),
            "Cannot write head, head is currently owned by software"
        );
        self.write_descriptor(desc, self.head)?;
        self.advance_head();
        Ok(())
    }
}

impl E1000 {
    fn read_ring_and_descriptors<T>(
        &mut self, base_address: usize, length: usize, head: usize, tail: usize,
    ) -> Result<Option<DescriptorRing<T>>>
    where
        T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]> + Descriptor,
    {
        // 1. Read ring
        let mapping = self
            .ctx
            .dma_map(base_address, length * DESCRIPTOR_LENGTH, 1, true, true)?;

        // Ring buffer might not yet be filled
        if is_all_zeros(mapping.dma(0)) {
            return Ok(None);
        }

        let ring = DescriptorRing::<T> {
            mapping,
            length,
            head,
            tail,
            phantom: PhantomData,
        };

        // 2. Read descriptors to populate packet buffer
        let descriptors = ring.read_all_descriptors()?;
        let len = find_descriptor_distance(&descriptors).unwrap_or(DESCRIPTOR_BUFFER_FALLBACK_SIZE)
            as usize;

        for descriptor in descriptors {
            // Skip null descriptors used for padding
            if descriptor.buffer() == 0 {
                continue;
            }

            let mapping = self
                .ctx
                .dma_map(descriptor.buffer() as usize, len, 1, true, true)?;
            // TODO: Remove previous ring descriptor mappings, if driver changes them
            self.packet_buffers.insert(descriptor.buffer(), mapping);
        }

        Ok(Some(ring))
    }

    pub fn setup_rx_ring(&mut self) {
        println!("E1000: Trying to initialize RX ring.");
        self.rx_ring = self
            .read_ring_and_descriptors::<ReceiveDescriptor>(
                self.regs.get_receive_descriptor_base_address() as usize,
                self.regs.rd_len.length as usize * 8,
                self.regs.rd_h.head as usize,
                self.regs.rd_t.tail as usize,
            )
            .unwrap();
        println!("Set rx ring to {:?}", self.rx_ring);
    }

    pub fn setup_tx_ring(&mut self) {
        println!("E1000: Trying to initialize TX ring.");
        self.tx_ring = self
            .read_ring_and_descriptors::<TransmitDescriptor>(
                self.regs.get_transmit_descriptor_base_address() as usize,
                self.regs.td_len.length as usize * 8,
                self.regs.td_h.head as usize,
                self.regs.td_t.tail as usize,
            )
            .unwrap();
        println!("Set tx ring to {:?}", self.tx_ring);
    }
}

// Simple trait to allow common ring setup process
trait Descriptor {
    fn buffer(&self) -> u64;
}

#[derive(PackedStruct, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "16", endian = "msb")]
pub struct ReceiveDescriptor {
    #[packed_field(bits = "0:63")]
    pub buffer: u64,

    #[packed_field(bits = "64:79")]
    pub length: u16,

    // Status field offset 96 bits
    #[packed_field(bits = "96")]
    pub status_dd: bool, // Descriptor Done

    #[packed_field(bits = "97")]
    pub status_eop: bool, // End of packet
}

impl Descriptor for ReceiveDescriptor {
    fn buffer(&self) -> u64 {
        self.buffer
    }
}

// Legacy Transmit Descriptor Format
#[derive(PackedStruct, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "16", endian = "msb")]
pub struct TransmitDescriptor {
    #[packed_field(bits = "0:63")]
    pub buffer: u64,

    #[packed_field(bits = "64:79")]
    pub length: u16,

    // Command field offset 88 bits
    #[packed_field(bits = "88")]
    pub cmd_eop: bool, // End of packet

    #[packed_field(bits = "91")]
    pub cmd_rs: bool, // Report status, if set status_dd should be set after processing packet

    // Status field offset 96 bits
    #[packed_field(bits = "96")]
    pub status_dd: bool, // Descriptor Done
}

impl Descriptor for TransmitDescriptor {
    fn buffer(&self) -> u64 {
        self.buffer
    }
}

// Find max distance between descriptor buffer pointers, provided they are allocated in succession
fn find_descriptor_distance<T: Descriptor>(descriptors: &Vec<T>) -> Option<u64> {
    let buffers: Vec<u64> = descriptors
        .iter()
        .map(|d| d.buffer())
        .filter(|b| *b != 0)
        .collect();

    buffers.windows(2).map(|w| w[0].abs_diff(w[1])).min()
}
