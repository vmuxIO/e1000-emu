use anyhow::{anyhow, Result};
use log::{debug, error};
use packed_struct::derive::PackedStruct;
use packed_struct::prelude::*;
use packed_struct::{PackedStruct, PackingResult};

use crate::e1000::E1000;
use crate::NicContext;

// Each descriptor is 16 bytes long, 8 for buffer address, rest for status, length, etc...
const DESCRIPTOR_LENGTH: usize = 16;

#[derive(Debug)]
pub struct DescriptorRing {
    ring_address: usize,
    length: usize,
    pub head: usize, // Managed by structure
    pub tail: usize, // Updated by client
}

impl DescriptorRing {
    fn read_descriptor<T>(&self, index: usize, nic_ctx: &mut dyn NicContext) -> Result<T>
    where
        T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]>,
    {
        nic_ctx.dma_prepare(self.ring_address, self.length * DESCRIPTOR_LENGTH);

        let mut data = [0u8; DESCRIPTOR_LENGTH];
        nic_ctx.dma_read(
            self.ring_address,
            data.as_mut_slice(),
            index * DESCRIPTOR_LENGTH,
        );
        data.reverse(); // Reverse because of endianness

        let descriptor = T::unpack(&data)?;
        Ok(descriptor)
    }

    fn write_descriptor<T>(
        &mut self, desc: &T, index: usize, nic_ctx: &mut dyn NicContext,
    ) -> Result<()>
    where
        T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]>,
    {
        nic_ctx.dma_prepare(self.ring_address, self.length * DESCRIPTOR_LENGTH);

        let mut data = desc.pack()?;
        data.reverse(); // Reverse because of endianness
        nic_ctx.dma_write(self.ring_address, &mut data, index * DESCRIPTOR_LENGTH);
        Ok(())
    }

    pub fn hardware_owned_descriptors(&self) -> usize {
        let mut tail = self.tail;
        if tail < self.head {
            tail += self.length;
        }

        tail - self.head
    }

    // Is the section owned by hardware empty?
    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    pub fn advance_head(&mut self) {
        self.head = (self.head + 1) % self.length;
    }

    fn head_check(&self) -> Result<()> {
        if self.is_empty() {
            Err(anyhow!(
                "Cannot access head, head is currently owned by software"
            ))
        } else {
            Ok(())
        }
    }

    pub fn read_head<T>(&self, nic_ctx: &mut dyn NicContext) -> Result<T>
    where
        T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]>,
    {
        self.head_check()?;
        self.read_descriptor(self.head, nic_ctx)
    }

    pub fn write_and_advance_head<T>(
        &mut self, desc: &T, nic_ctx: &mut dyn NicContext,
    ) -> Result<()>
    where
        T: PackedStruct<ByteArray = [u8; DESCRIPTOR_LENGTH]>,
    {
        self.head_check()?;
        self.write_descriptor(desc, self.head, nic_ctx)?;
        self.advance_head();
        Ok(())
    }
}

impl<C: NicContext> E1000<C> {
    pub fn setup_rx_ring(&mut self) {
        debug!("Initializing RX ring.");
        self.rx_ring = Some(DescriptorRing {
            ring_address: self.regs.get_receive_descriptor_base_address() as usize,
            length: self.regs.rd_len.length as usize * 8,
            head: self.regs.rd_h.head as usize,
            tail: self.regs.rd_t.tail as usize,
        });
    }

    pub fn setup_tx_ring(&mut self) {
        debug!("Initializing TX ring.");
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

    #[packed_field(bits = "98")]
    status_ixsm: ReservedOne<packed_bits::Bits<1>>, // Ignore checksum indication, always on
}

// Common transmit descriptor for differentiating between the different transmit descriptor types
#[derive(PackedStruct, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "16", endian = "msb")]
pub struct TransmitDescriptorCommon {
    #[packed_field(bits = "84")]
    dtyp: u8, // Extension type, 0000b -> TCP/IP context, 0001b -> TCP/IP data

    // Command field offset 88 bits in all transmit descriptor variants
    #[packed_field(bits = "91")]
    cmd_rs: bool, // Report Status, if set status_dd should be set after processing packet

    #[packed_field(bits = "92")] // Reserved in context descriptor
    cmd_rps: bool, // Report Packet Sent, but treat just like Report Status

    #[packed_field(bits = "93")]
    cmd_dext: bool, // Extension, 0 -> Legacy descriptor, 1 -> TCP/IP context or data descriptor

    // Status field offset 96 bits in all transmit descriptor variants
    #[packed_field(bits = "96")]
    pub status_dd: bool, // Descriptor Done
}

impl TransmitDescriptorCommon {
    pub fn report_status(&self) -> bool {
        self.cmd_rs || self.cmd_rps
    }
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

    #[packed_field(bits = "104:111")]
    pub css: u8, // Checksum Start Field

    #[packed_field(bits = "112:127")]
    special: u16, // Unused but keep to not delete it when writing back descriptor
}

// TCP/IP context transmit descriptor, does not contain any data by itself,
// always in front of one or multiple TCP/IP data transmit descriptors
#[derive(PackedStruct, Debug)]
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
    #[packed_field(bits = "88")]
    pub tucmd_tcp: bool, // Packet Type, 0 -> UDP, 1 -> TCP

    #[packed_field(bits = "89")]
    pub tucmd_ip: bool, // Packet Type, 0 -> IPv6, 1 -> IPv4

    #[packed_field(bits = "90")]
    pub tucmd_tse: bool, // TCP Segmentation Enable

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

    // Packet Options field offset 104 bits
    #[packed_field(bits = "104")]
    pub popts_ixsm: bool, // Insert IP Checksum

    #[packed_field(bits = "105")]
    pub popts_txsm: bool, // Insert TCP/UDP Checksum

    #[packed_field(bits = "112:127")]
    special: u16, // Unused but keep to not delete it when writing back descriptor
}

#[derive(Debug)]
pub enum TransmitDescriptorVariant {
    Legacy(TransmitDescriptorLegacy),
    TcpContext(TransmitDescriptorTcpContext),
    TcpData(TransmitDescriptorTcpData),
}

#[derive(Debug)]
pub struct TransmitDescriptor {
    pub common: TransmitDescriptorCommon,
    pub variant: TransmitDescriptorVariant,
}

impl PackedStruct for TransmitDescriptor {
    type ByteArray = [u8; DESCRIPTOR_LENGTH];

    fn pack(&self) -> PackingResult<Self::ByteArray> {
        let common_packed = self.common.pack()?;
        let variant_packed = match &self.variant {
            TransmitDescriptorVariant::Legacy(desc) => desc.pack(),
            TransmitDescriptorVariant::TcpContext(desc) => desc.pack(),
            TransmitDescriptorVariant::TcpData(desc) => desc.pack(),
        }?;

        // Bitwise or in place
        let mut combined = common_packed;
        combined
            .iter_mut()
            .zip(variant_packed)
            .for_each(|(x, y)| *x |= y);

        Ok(combined)
    }

    fn unpack(src: &Self::ByteArray) -> PackingResult<Self> {
        let common = TransmitDescriptorCommon::unpack(&src)?;
        let variant = match (common.cmd_dext, common.dtyp) {
            (false, _) => {
                TransmitDescriptorVariant::Legacy(TransmitDescriptorLegacy::unpack(&src)?)
            }
            (true, 0) => {
                TransmitDescriptorVariant::TcpContext(TransmitDescriptorTcpContext::unpack(&src)?)
            }
            (true, 1) => {
                TransmitDescriptorVariant::TcpData(TransmitDescriptorTcpData::unpack(&src)?)
            }
            _ => {
                error!("Failed to match transmit descriptor type.");
                return Err(PackingError::InternalError);
            }
        };

        Ok(Self { common, variant })
    }
}
