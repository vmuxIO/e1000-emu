// Allow naming fields by their official all upper case abbreviations
#![allow(non_snake_case)]

use std::time::Duration;

use anyhow::Result;
use log::trace;
use packed_struct::derive::PackedStruct;
use packed_struct::prelude::{packed_bits, ReservedOne};
use packed_struct::PackedStruct;

use crate::e1000::E1000;
use crate::util::match_and_access_registers;
use crate::NicContext;

#[derive(Default, Debug)]
pub struct Registers {
    // General control and status
    pub ctrl: Control,
    pub status: Status,

    // Eeprom Control & Data
    pub eecd: EepromControlAndData,

    // Management Data Interface Control, for reading/writing PHY
    pub mdic: MdiControl,

    // Interrupts
    pub interrupt_throttling: InterruptDelay,
    pub interrupt_cause: InterruptCauses,
    pub interrupt_mask: InterruptCauses,
    // Temporary register required for mask and causes updates, since writes to them are indirect
    // IMS and IMC do not directly set mask but instead just set what bits to enable/disable
    // ICS (and probably ICR) writes do as well for causes to avoid races
    interrupt_temp: InterruptCauses,

    // Receive and Transmit Control
    pub rctl: ReceiveControl,
    pub tctl: TransmitControl,

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

    pub fn get_transmit_descriptor_base_address(&self) -> u64 {
        let low = (self.td_ba_l.base_address_low as u64) << 4;
        let high = (self.td_ba_h.base_address_high as u64) << 32;
        low | high
    }
}

fn clear(register: &mut impl Default) {
    *register = Default::default();
    trace!("Cleared register.");
}

impl<C: NicContext> E1000<C> {
    pub fn access_register(
        &mut self, offset: u32, data: &mut [u8], write: bool,
    ) -> Option<Result<()>> {
        // While we could alternatively match offsets to registers and call .access(data, write)
        // after the match, that would require an additional match just to invoke actions
        // e.g. for controlling registers and registers that clear after read
        // So instead do it in one go using custom macro
        let result = match_and_access_registers!( offset, data, write, {
            // Offset => Register ( => and also do )
            0x0 => self.regs.ctrl => { if write { self.ctrl_write() } },
            0x8 => self.regs.status,

            // Eeprom Control & Data
            0x10 => self.regs.eecd => { if write { self.eecd_write() } },

            // Management Data Interface Control, for reading/writing PHY
            0x20 => self.regs.mdic => { if write { self.mdic_write() } },

            0xC4 => self.regs.interrupt_throttling,

            // ICR (0xC0) reads: clear-on-read, writes: out of spec but will clear specific causes
            // ICS (0xC8) writes: manually trigger interrupts, reads: out of spec but
            // real e1000 still allows ICS reads, which some drivers use to read without clear
            0xC0 if !write => self.regs.interrupt_cause => { clear(&mut self.regs.interrupt_cause); },
            0xC8 if !write => self.regs.interrupt_cause,
            0xC0 | 0xC8 => self.regs.interrupt_temp => {
                // Add causes for ICS, remove causes if ICR
                let clear = offset == 0xC0;
                self.regs.interrupt_cause.modify(&self.regs.interrupt_temp, clear);

                trace!(
                    "Updated interrupt cause, with clear={}, now: {:?}",
                    clear,
                    self.regs.interrupt_cause
                );
                // if write {
                    self.interrupt();
                // }
            },

            // IMS (0xD0) for reading interrupt mask (read) and for enabling interrupts (write)
            // IMC (0xD8) for disabling interrupts (only write)
            0xD0 | 0xD8 if !write => self.regs.interrupt_mask,
            0xD0 | 0xD8 => self.regs.interrupt_temp => {
                // Add causes for IMS, remove causes if IMC
                let clear = offset == 0xD8;
                self.regs.interrupt_mask.modify(&self.regs.interrupt_temp, clear);

                trace!(
                    "Updated interrupt mask, with clear={}, now: {:?}",
                    clear,
                    self.regs.interrupt_mask
                );
                self.interrupt();
            },

            // Receive and Transmit Control
            0x100 => self.regs.rctl => { if write { self.rctl_write() } },
            0x400 => self.regs.tctl => { if write { self.tctl_write() } },

            // Receive descriptor
            0x2800 => self.regs.rd_ba_l,
            0x2804 => self.regs.rd_ba_h,
            0x2808 => self.regs.rd_len,
            0x2810 => self.regs.rd_h,
            0x2818 => self.regs.rd_t => { if write { self.rdt_write() } },

            // Transmit descriptor
            0x3800 => self.regs.td_ba_l,
            0x3804 => self.regs.td_ba_h,
            0x3808 => self.regs.td_len,
            0x3810 => self.regs.td_h,
            0x3818 => self.regs.td_t => { if write { self.tdt_write() } },

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

// General control and status
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
    // Always indicate Full duplex
    #[packed_field(bits = "0")]
    pub FD: ReservedOne<packed_bits::Bits<1>>, // 0: Half duplex, 1: Full duplex

    #[packed_field(bits = "1")]
    pub LU: bool, // Link up

    // Always indicate 1000Mbit/s speed
    #[packed_field(bits = "6")]
    pub speed1: ReservedOne<packed_bits::Bits<1>>,
    #[packed_field(bits = "7")]
    pub speed2: ReservedOne<packed_bits::Bits<1>>,
}

// Interrupt register layouts, shared by ICR, ICS, IMS, IMC
#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4")]
pub struct InterruptCauses {
    #[packed_field(bits = "0")]
    pub TXDW: bool, // Transmit Descriptor written back

    #[packed_field(bits = "1")]
    pub TXQE: bool, // Transmit Queue Empty

    #[packed_field(bits = "2")] // Also manually triggered by linux kernel driver
    pub LSC: bool, // Link Status Change

    #[packed_field(bits = "4")] // Manually triggered by linux kernel driver
    pub RXDMT0: bool, // Receive Descriptor Minimum Threshold Reached

    #[packed_field(bits = "7")]
    pub RXT0: bool, // Receive Timer Interrupt

    #[packed_field(bits = "9")]
    pub MDAC: bool, // MDI/O Access Complete
} // Omitted a lot more causes, which are not yet emulated

impl InterruptCauses {
    fn modify(&mut self, mask: &InterruptCauses, clear: bool) {
        let previous_bits = u32::from_ne_bytes(self.pack().unwrap());
        let mask_bits = u32::from_ne_bytes(mask.pack().unwrap());

        let new_bits = if clear {
            previous_bits & !mask_bits
        } else {
            previous_bits | mask_bits
        };
        *self = InterruptCauses::unpack(&new_bits.to_ne_bytes()).unwrap();
    }
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4", endian = "msb")]
pub struct InterruptDelay {
    #[packed_field(bits = "0:15")]
    pub interval: u16, // Interval in 256ns increments for ITR, 1024ns for other regs
}

impl InterruptDelay {
    /// Function only for ITR! Other interrupt delay registers use different time increments!
    pub(crate) fn get_itr_interval(&self) -> Option<Duration> {
        if self.interval == 0 {
            None
        } else {
            Some(Duration::new(0, self.interval as u32 * 256)) // 256ns increments
        }
    }
}

// Rx and Tx
#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4")]
pub struct ReceiveControl {
    #[packed_field(bits = "1")]
    pub EN: bool, // Receiver Enable

    #[packed_field(bits = "16:17")]
    BSIZE: u8, // Receive Buffer Size

    #[packed_field(bits = "25")]
    BSEX: bool, // Buffer Size Extension

    #[packed_field(bits = "26")]
    pub SECRC: bool, // Strip Ethernet CRC from incoming packet
}

impl ReceiveControl {
    pub fn get_buffer_size(&self) -> usize {
        let mut size = match self.BSIZE {
            0b00 => 2048,
            0b01 => 1024,
            0b10 => 512,
            0b11 => 256,
            _ => unreachable!("Invalid RCTL BSIZE"),
        };
        if self.BSEX {
            // BSEX is normally only supported for BSIZE values != 00, but support it anyway
            size *= 16;
        }
        size
    }
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4")]
pub struct TransmitControl {
    #[packed_field(bits = "1")]
    pub EN: bool, // Transmit Enable
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

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4")]
pub struct EepromControlAndData {
    #[packed_field(bits = "0")]
    pub SK: bool, // Clock input

    #[packed_field(bits = "1")]
    pub CS: bool, // Chip select

    #[packed_field(bits = "2")]
    pub DI: bool, // Data input

    #[packed_field(bits = "3")]
    pub DO: bool, // Data output

    // Omit FWE (Flash Write Enable Control) to leave it at 00b -> Not allowed
    #[packed_field(bits = "6")]
    pub EE_REQ: bool, // Request EEPROM Access

    // Eeprom always present and accessible
    #[packed_field(bits = "7")]
    pub EE_GNT: ReservedOne<packed_bits::Bits<1>>, // Grant EEPROM Access

    #[packed_field(bits = "8")]
    pub EE_PRES: ReservedOne<packed_bits::Bits<1>>, // EEPROM Present
}

// Management Data Interface Control, for reading/writing PHY
#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4", endian = "msb")]
pub struct MdiControl {
    #[packed_field(bits = "0:15")]
    pub data: u16,

    #[packed_field(bits = "16:20")]
    pub register_address: u8, // PHY register address

    #[packed_field(bits = "26:27")]
    pub opcode: u8,

    #[packed_field(bits = "28")]
    pub ready: bool,

    #[packed_field(bits = "29")]
    pub interrupt_enable: bool,
}
