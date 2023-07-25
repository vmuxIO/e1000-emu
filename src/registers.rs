// Allow naming fields by their official all upper case abbreviations
#![allow(non_snake_case)]

use anyhow::Result;
use packed_struct::derive::PackedStruct;
use packed_struct::PackedStruct;

use crate::util::match_and_access_registers;
use crate::E1000;

#[derive(Default, Debug)]
pub struct Registers {
    pub ctrl: Control,
    pub status: Status,
    pub rctl: ReceiveControl,

    // Receive descriptor
    pub rd_ba_l: DescriptorBaseAddressLow,
    pub rd_ba_h: DescriptorBaseAddressHigh,
    pub rd_len: DescriptorLength,
    pub rd_h: DescriptorHead,
    pub rd_t: DescriptorTail,

    // Transmit descriptor
    pub td_ba_l: DescriptorBaseAddressLow,
    pub td_ba_h: DescriptorBaseAddressHigh,
    pub td_len: DescriptorLength,
    pub td_h: DescriptorHead,
    pub td_t: DescriptorTail,

    // Receive Address 0, Ethernet MAC address
    pub ral0: ReceiveAddressLow,
    pub rah0: ReceiveAddressHigh,
}

impl Registers {
    pub fn set_mac(&mut self, mac: [u8; 6]) {
        self.ral0.receive_address_low = u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]);
        self.rah0.receive_address_high = u16::from_le_bytes([mac[4], mac[5]]);
    }

    pub fn get_receive_descriptor_base_address(&self) -> u64 {
        let low = (self.rd_ba_l.base_address_low as u64) << 4;
        let high = (self.rd_ba_h.base_address_high as u64) << 32;
        low | high
    }
}

impl E1000 {
    pub fn access_register(
        &mut self, offset: u32, data: &mut [u8], write: bool,
    ) -> Option<Result<()>> {
        // While we could alternatively match offsets to registers and call .access(data, write)
        // after the match, that would require an additional match just to invoke actions
        // e.g. for controlling registers and registers that clear after read
        let result = match_and_access_registers!( offset, data, write, true, {
            // Offset => Register ( => and also do )
            0x0 => self.regs.ctrl => { self.ctrl_access(write) },
            0x8 => self.regs.status,
            0x100 => self.regs.rctl => { self.rctl_access(write) },

            // Receive descriptor
            0x2800 => self.regs.rd_ba_l,
            0x2804 => self.regs.rd_ba_h,
            0x2808 => self.regs.rd_len,
            0x2810 => self.regs.rd_h,
            0x2818 => self.regs.rd_t => { self.rdt_access(write) },

            // Transmit descriptor
            0x3800 => self.regs.td_ba_l,
            0x3804 => self.regs.td_ba_h,
            0x3808 => self.regs.td_len,
            0x3810 => self.regs.rd_h,
            0x3818 => self.regs.td_t,

            // Receive Address 0, Ethernet MAC address
            0x5400 => self.regs.ral0,
            0x5404 => self.regs.rah0,
        } else {
            // Wildcard, if none of the above match
            return None;
        });

        Some(result)
    }
}

#[allow(unused_variables)]
pub trait Register {
    fn read(&self) -> Result<[u8; 4]>;
    fn write(&mut self, data: [u8; 4]) -> Result<()>;

    fn access(&mut self, data: &mut [u8], write: bool) -> Result<()> {
        if write {
            let mut buffer = [0u8; 4];
            buffer.copy_from_slice(&data[..4]);
            self.write(buffer)?;
        } else {
            data[..4].copy_from_slice(self.read()?.as_slice());
        }
        Ok(())
    }
}

impl<T> Register for T
where
    T: PackedStruct<ByteArray = [u8; 4]> + Clone,
{
    fn read(&self) -> Result<[u8; 4]> {
        let mut reg = self.pack()?;
        reg.reverse(); // Reverse because of endianness
        Ok(reg)
    }

    fn write(&mut self, mut data: [u8; 4]) -> Result<()> {
        data.reverse(); // Reverse because of endianness
        self.clone_from(&T::unpack(&data)?);
        Ok(())
    }
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4")]
pub struct Control {
    #[packed_field(bits = "6")]
    pub SLU: bool, // Set link up

    #[packed_field(bits = "26")]
    pub RST: bool, // Device Reset
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4")]
pub struct Status {
    #[packed_field(bits = "1")]
    pub LU: bool, // Link up
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4")]
pub struct ReceiveControl {
    #[packed_field(bits = "1")]
    pub EN: bool, // Receiver Enable
}

// Descriptor register layouts, used by rx and tx descriptor registers
#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4", endian = "msb")]
pub struct DescriptorBaseAddressLow {
    #[packed_field(bits = "4:31")]
    pub base_address_low: u32,
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4", endian = "msb")]
pub struct DescriptorBaseAddressHigh {
    #[packed_field(bits = "0:31")]
    pub base_address_high: u32,
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4", endian = "msb")]
pub struct DescriptorLength {
    #[packed_field(bits = "7:19")]
    pub length: u16,
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4", endian = "msb")]
pub struct DescriptorHead {
    #[packed_field(bits = "0:15")]
    pub head: u16,
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4", endian = "msb")]
pub struct DescriptorTail {
    #[packed_field(bits = "0:15")]
    pub tail: u16,
}

// Receive Address
#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4", endian = "msb")]
pub struct ReceiveAddressLow {
    #[packed_field(bits = "0:31")]
    pub receive_address_low: u32,
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4", endian = "msb")]
pub struct ReceiveAddressHigh {
    #[packed_field(bits = "0:15")]
    pub receive_address_high: u16,
}
