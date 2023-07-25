use std::marker::PhantomData;
use std::slice::{ChunksExact, ChunksExactMut};

use anyhow::{ensure, Context, Result};
use libvfio_user::dma::DmaMapping;
use packed_struct::derive::PackedStruct;
use packed_struct::PackedStruct;

use crate::E1000;

// Each descriptor is 16 bytes long, 8 for buffer address, rest for status, length, etc...
pub const DESCRIPTOR_LENGTH: usize = 16;

pub struct DescriptorRing<T> {
    mapping: DmaMapping,
    length: usize,
    head: usize,
    pub tail: usize,
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
            "Cannot advance head, head is currently owned by software"
        );
        self.write_descriptor(desc, self.head)?;
        self.head = (self.head + 1) % self.length;
        Ok(())
    }
}

impl E1000 {
    pub fn initialize_rx_ring(&mut self) {
        let base_address = self.regs.get_receive_descriptor_base_address() as usize;
        let length = self.regs.rd_len.length as usize * 8;
        let head = self.regs.rd_h.head as usize;
        let tail = self.regs.rd_t.tail as usize;

        let rx_ring_mapping = self
            .ctx
            .map_range(base_address, length * DESCRIPTOR_LENGTH, 1, true, true)
            .unwrap();

        let rx_ring = DescriptorRing::<ReceiveDescriptor> {
            mapping: rx_ring_mapping,
            length,
            head,
            tail,
            phantom: PhantomData,
        };
        let descriptors = rx_ring.read_all_descriptors().unwrap();
        for descriptor in descriptors {
            // Skip null descriptors used for padding
            if descriptor.buffer == 0 {
                continue;
            }

            let len = self
                .ctx
                .dma_regions()
                .get(&(descriptor.buffer as usize))
                .unwrap();
            let mapping = self
                .ctx
                .map_range(descriptor.buffer as usize, *len, 1, true, true)
                .unwrap();
            self.packet_buffers.insert(descriptor.buffer, mapping);
        }

        self.rx_ring = Some(rx_ring);
    }
}

#[derive(PackedStruct, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "16", endian = "msb")]
pub struct ReceiveDescriptor {
    #[packed_field(bits = "0:63")]
    pub buffer: u64,

    #[packed_field(bits = "64:79")]
    pub length: u16,

    #[packed_field(bits = "96:103")]
    pub status: ReceiveDescriptorStatus,
}

#[derive(PackedStruct, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "1")]
pub struct ReceiveDescriptorStatus {
    #[packed_field(bits = "0")]
    pub descriptor_done: bool, // DD
}
