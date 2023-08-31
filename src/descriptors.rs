use std::marker::PhantomData;

use anyhow::{ensure, Context, Result};
use packed_struct::derive::PackedStruct;
use packed_struct::PackedStruct;

use crate::{NicContext, E1000};

// Each descriptor is 16 bytes long, 8 for buffer address, rest for status, length, etc...
const DESCRIPTOR_LENGTH: usize = 16;
pub const DESCRIPTOR_BUFFER_SIZE: usize = 1920; // Default size linux kernel driver uses

#[derive(Debug)]
pub struct DescriptorRing<T> {
    ring_address: usize,
    length: usize,
    pub head: usize, // Managed by structure
    pub tail: usize, // Updated by client
    phantom: PhantomData<T>,
}

impl<T> DescriptorRing<T>
where
    T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]>,
{
    fn read_descriptor(&self, index: usize, nic_ctx: &mut dyn NicContext) -> Result<T> {
        let mut ring_buffer = vec![0u8; self.length * DESCRIPTOR_LENGTH];
        nic_ctx.dma_read(self.ring_address, ring_buffer.as_mut_slice());

        let buffer = ring_buffer
            .chunks_exact(DESCRIPTOR_LENGTH)
            .nth(index)
            .context("Descriptor not in mapping")?;

        let mut data = [0u8; DESCRIPTOR_LENGTH];
        data.copy_from_slice(buffer);
        data.reverse(); // Reverse because of endianness

        let descriptor = T::unpack(&data)?;
        Ok(descriptor)
    }

    fn write_descriptor(
        &mut self, desc: T, index: usize, nic_ctx: &mut dyn NicContext,
    ) -> Result<()> {
        let mut ring_buffer = vec![0u8; self.length * DESCRIPTOR_LENGTH];
        nic_ctx.dma_read(self.ring_address, ring_buffer.as_mut_slice());

        let buffer = ring_buffer
            .chunks_exact_mut(DESCRIPTOR_LENGTH)
            .nth(index)
            .context("Descriptor not in mapping")?;

        let mut data = desc.pack()?;
        data.reverse(); // Reverse because of endianness

        buffer.copy_from_slice(data.as_slice());
        nic_ctx.dma_write(self.ring_address, ring_buffer.as_mut_slice());
        Ok(())
    }

    // Is the section owned by hardware empty?
    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    pub fn advance_head(&mut self) {
        self.head = (self.head + 1) % self.length;
    }

    pub fn read_head(&self, nic_ctx: &mut dyn NicContext) -> Result<T> {
        ensure!(
            !self.is_empty(),
            "Cannot read head, head is currently owned by software"
        );
        self.read_descriptor(self.head, nic_ctx)
    }

    pub fn write_and_advance_head(&mut self, desc: T, nic_ctx: &mut dyn NicContext) -> Result<()> {
        ensure!(
            !self.is_empty(),
            "Cannot write head, head is currently owned by software"
        );
        self.write_descriptor(desc, self.head, nic_ctx)?;
        self.advance_head();
        Ok(())
    }
}

impl<C: NicContext> E1000<C> {
    fn map_ring<T>(
        &mut self, base_address: usize, length: usize, head: usize, tail: usize,
    ) -> DescriptorRing<T>
    where
        T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]> + Descriptor,
    {
        let ring = DescriptorRing::<T> {
            ring_address: base_address,
            length,
            head,
            tail,
            phantom: PhantomData,
        };

        ring
    }

    pub fn setup_rx_ring(&mut self) {
        println!("E1000: Initializing RX ring.");
        self.rx_ring = Some(self.map_ring::<ReceiveDescriptor>(
            self.regs.get_receive_descriptor_base_address() as usize,
            self.regs.rd_len.length as usize * 8,
            self.regs.rd_h.head as usize,
            self.regs.rd_t.tail as usize,
        ));
    }

    pub fn setup_tx_ring(&mut self) {
        println!("E1000: Initializing TX ring.");
        self.tx_ring = Some(self.map_ring::<TransmitDescriptor>(
            self.regs.get_transmit_descriptor_base_address() as usize,
            self.regs.td_len.length as usize * 8,
            self.regs.td_h.head as usize,
            self.regs.td_t.tail as usize,
        ));
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

    #[packed_field(bits = "93")]
    pub cmd_dext: bool, // Extension

    // Status field offset 96 bits
    #[packed_field(bits = "96")]
    pub status_dd: bool, // Descriptor Done
}

impl Descriptor for TransmitDescriptor {
    fn buffer(&self) -> u64 {
        self.buffer
    }
}
