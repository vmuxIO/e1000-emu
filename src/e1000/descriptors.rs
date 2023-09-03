use anyhow::{anyhow, ensure, Context, Result};
use packed_struct::derive::PackedStruct;
use packed_struct::PackedStruct;

use crate::e1000::E1000;
use crate::NicContext;

// Each descriptor is 16 bytes long, 8 for buffer address, rest for status, length, etc...
const DESCRIPTOR_LENGTH: usize = 16;
pub const DESCRIPTOR_BUFFER_SIZE: usize = 1920; // Default size linux kernel driver uses

#[derive(Debug)]
pub struct DescriptorRing {
    ring_address: usize,
    length: usize,
    pub head: usize, // Managed by structure
    pub tail: usize, // Updated by client
}

impl DescriptorRing {
    fn read_descriptor_raw(
        &self, index: usize, nic_ctx: &mut dyn NicContext,
    ) -> Result<[u8; DESCRIPTOR_LENGTH]> {
        let mut ring_buffer = vec![0u8; self.length * DESCRIPTOR_LENGTH];
        nic_ctx.dma_read(self.ring_address, ring_buffer.as_mut_slice());

        let buffer = ring_buffer
            .chunks_exact(DESCRIPTOR_LENGTH)
            .nth(index)
            .context("Descriptor not in mapping")?;

        let mut data = [0u8; DESCRIPTOR_LENGTH];
        data.copy_from_slice(buffer);
        data.reverse(); // Reverse because of endianness

        Ok(data)
    }

    fn read_descriptor<T>(&self, index: usize, nic_ctx: &mut dyn NicContext) -> Result<T>
    where
        T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]>,
    {
        let data = self.read_descriptor_raw(index, nic_ctx)?;

        let descriptor = T::unpack(&data)?;
        Ok(descriptor)
    }

    fn write_descriptor<T>(
        &mut self, desc: T, index: usize, nic_ctx: &mut dyn NicContext,
    ) -> Result<()>
    where
        T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]>,
    {
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

    pub fn read_head_raw(&self, nic_ctx: &mut dyn NicContext) -> Result<[u8; DESCRIPTOR_LENGTH]> {
        ensure!(
            !self.is_empty(),
            "Cannot read head, head is currently owned by software"
        );
        self.read_descriptor_raw(self.head, nic_ctx)
    }

    pub fn read_head<T>(&self, nic_ctx: &mut dyn NicContext) -> Result<T>
    where
        T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]>,
    {
        ensure!(
            !self.is_empty(),
            "Cannot read head, head is currently owned by software"
        );
        self.read_descriptor(self.head, nic_ctx)
    }

    pub fn write_and_advance_head<T>(&mut self, desc: T, nic_ctx: &mut dyn NicContext) -> Result<()>
    where
        T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]>,
    {
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
    pub fn setup_rx_ring(&mut self) {
        println!("E1000: Initializing RX ring.");
        self.rx_ring = Some(DescriptorRing {
            ring_address: self.regs.get_receive_descriptor_base_address() as usize,
            length: self.regs.rd_len.length as usize * 8,
            head: self.regs.rd_h.head as usize,
            tail: self.regs.rd_t.tail as usize,
        });
    }

    pub fn setup_tx_ring(&mut self) {
        println!("E1000: Initializing TX ring.");
        self.tx_ring = Some(DescriptorRing {
            ring_address: self.regs.get_transmit_descriptor_base_address() as usize,
            length: self.regs.td_len.length as usize * 8,
            head: self.regs.td_h.head as usize,
            tail: self.regs.td_t.tail as usize,
        });
    }
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

// Base transmit descriptor for differentiating between the different transmit descriptor types
#[derive(PackedStruct, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "16", endian = "msb")]
pub struct TransmitDescriptorBase {
    #[packed_field(bits = "84")]
    pub dtyp: u8, // Extension type, 0000b -> TCP/IP context, 0001b -> TCP/IP data

    #[packed_field(bits = "93")]
    pub dext: bool, // Extension, 0 -> Legacy descriptor, 1 -> TCP/IP context or data descriptor
}

// Legacy Transmit Descriptor Format
#[derive(PackedStruct, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "16", endian = "msb")]
pub struct TransmitDescriptorLegacy {
    #[packed_field(bits = "0:63")]
    pub buffer: u64,

    #[packed_field(bits = "64:79")]
    pub length: u16,

    #[packed_field(bits = "80:87")]
    pub cso: u8, // Checksum Offset

    // Command field offset 88 bits
    #[packed_field(bits = "88")]
    pub cmd_eop: bool, // End of packet

    #[packed_field(bits = "90")]
    pub cmd_ic: bool, // Insert Checksum

    #[packed_field(bits = "91")]
    pub cmd_rs: bool, // Report status, if set status_dd should be set after processing packet

    // Status field offset 96 bits
    #[packed_field(bits = "96")]
    pub status_dd: bool, // Descriptor Done

    #[packed_field(bits = "104:111")]
    pub css: u8, // Checksum Start Field
}

// TCP/IP context transmit descriptor, does not contain any data by itself,
// always in front of one or multiple TCP/IP data transmit descriptors
#[derive(PackedStruct, Debug, Clone)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "16", endian = "msb")]
pub struct TransmitDescriptorTcpContext {
    #[packed_field(bits = "0:7")]
    pub ip_css: u8, // IP Checksum Start

    #[packed_field(bits = "8:15")]
    pub ip_cso: u8, // IP Checksum Offset

    #[packed_field(bits = "16:31")]
    pub ip_cse: u16, // IP Checksum Ending

    #[packed_field(bits = "32:39")]
    pub tu_css: u8, // TCP/UDP Checksum Start

    #[packed_field(bits = "40:47")]
    pub tu_cso: u8, // TCP/UDP Checksum Offset

    #[packed_field(bits = "48:63")]
    pub tu_cse: u16, // TCP/UDP Checksum Ending

    #[packed_field(bits = "64:83")]
    pub paylen: u32, // Payload Length

    // Command field offset 88 bits
    #[packed_field(bits = "90")]
    pub tucmd_tse: bool, // TCP Segmentation Enable

    #[packed_field(bits = "91")]
    pub tucmd_rs: bool, // Report status, if set status_dd should be set after processing packet

    // Status field offset 96 bits
    #[packed_field(bits = "96")]
    pub status_dd: bool, // Descriptor Done

    #[packed_field(bits = "104:111")]
    pub hdrlen: u8, // Header Length

    #[packed_field(bits = "112:127")]
    pub mss: u16, // Maximum Segment Size
}

// TCP/IP data transmit descriptor
#[derive(PackedStruct, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "16", endian = "msb")]
pub struct TransmitDescriptorTcpData {
    #[packed_field(bits = "0:63")]
    pub buffer: u64,

    #[packed_field(bits = "64:83")]
    pub length: u32,

    // Command field offset 88 bits
    #[packed_field(bits = "88")]
    pub dcmd_eop: bool, // End of packet

    #[packed_field(bits = "91")]
    pub dcmd_rs: bool, // Report status, if set status_dd should be set after processing packet

    // Status field offset 96 bits
    #[packed_field(bits = "96")]
    pub status_dd: bool, // Descriptor Done

    // Packet Options field offset 104 bits
    #[packed_field(bits = "104")]
    pub popts_ixsm: bool, // Insert IP Checksum

    #[packed_field(bits = "105")]
    pub popts_txsm: bool, // Insert TCP/UDP Checksum
}

#[derive(Debug)]
pub enum TransmitDescriptor {
    Legacy(TransmitDescriptorLegacy),
    TcpContext(TransmitDescriptorTcpContext),
    TcpData(TransmitDescriptorTcpData),
}

impl TransmitDescriptor {
    pub fn read_descriptor(tx_ring: &DescriptorRing, nic_ctx: &mut dyn NicContext) -> Result<Self> {
        let raw_descriptor = tx_ring.read_head_raw(nic_ctx)?;

        let base_descriptor = TransmitDescriptorBase::unpack(&raw_descriptor)?;

        match (base_descriptor.dext, base_descriptor.dtyp) {
            (false, _) => Ok(Self::Legacy(TransmitDescriptorLegacy::unpack(
                &raw_descriptor,
            )?)),
            (true, 0) => Ok(Self::TcpContext(TransmitDescriptorTcpContext::unpack(
                &raw_descriptor,
            )?)),
            (true, 1) => Ok(Self::TcpData(TransmitDescriptorTcpData::unpack(
                &raw_descriptor,
            )?)),
            _ => Err(anyhow!("Failed to match transmit descriptor type.")),
        }
    }

    pub fn report_status(&self) -> bool {
        match self {
            TransmitDescriptor::Legacy(desc) => desc.cmd_rs,
            TransmitDescriptor::TcpContext(desc) => desc.tucmd_rs,
            TransmitDescriptor::TcpData(desc) => desc.dcmd_rs,
        }
    }

    pub fn descriptor_done_mut(&mut self) -> &mut bool {
        match self {
            TransmitDescriptor::Legacy(ref mut desc) => &mut desc.status_dd,
            TransmitDescriptor::TcpContext(ref mut desc) => &mut desc.status_dd,
            TransmitDescriptor::TcpData(ref mut desc) => &mut desc.status_dd,
        }
    }
}
