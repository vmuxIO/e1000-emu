use std::num::Wrapping;

use anyhow::{anyhow, Result};
use packed_struct::derive::PackedStruct;
use packed_struct::PackedStruct;

use crate::E1000;

const MICROWIRE_OPCODE_READ: u8 = 0x6;
// There is also write, erase, erase/write enable, erase/write disable opcodes
// but they are not used by linux e1000 kernel driver, at least in normal operation

// Properties of small Microwire eeprom, are different for big microwire and for SPI
const OPCODE_BITS: u16 = 3;
const ADDRESS_BITS: u16 = 6;

const DESIRED_CHECKSUM: u16 = 0xBABA;

#[derive(Debug)]
struct EepromWires {
    clock_input: bool,
    chip_select: bool,
    data_input: bool,
    data_output: bool,
}

#[derive(Debug)]
enum EepromOperationStage {
    WaitingOpcode { written_opcode: u8 },
    WaitingAddress { written_address: u16 },
    Reading { address: u16 },
}

#[derive(Debug)]
pub struct EepromInterface {
    pub initial_eeprom: Eeprom,
    data: [u16; 64],

    previous_chip_select: bool,
    previous_clock: bool,

    stage: EepromOperationStage,
    bit_index: u16,
}

// Reference eeprom emulation:
// https://github.com/qemu/qemu/blob/64d3be986f9e2379bc688bf1d0aca0557e0035ca/hw/net/e1000.c#L489
// But use single position and split reading opcode, address and accessing data stages into enum
impl EepromInterface {
    fn process_wires(&mut self, wires: &mut EepromWires) -> Result<()> {
        // Check Chip select
        if !wires.chip_select {
            self.previous_chip_select = false;
            return Ok(());
        }

        if !self.previous_chip_select {
            // Chip select was just activated
            self.stage = EepromOperationStage::WaitingOpcode { written_opcode: 0 };
            self.bit_index = 0;

            self.previous_chip_select = true;
        }

        // Check Clock input
        if wires.clock_input == self.previous_clock {
            return Ok(());
        }
        self.previous_clock = wires.clock_input;

        if wires.clock_input {
            // Low -> High: Process data in or provide data out

            match self.stage {
                EepromOperationStage::WaitingOpcode {
                    ref mut written_opcode,
                } => {
                    // Shift in opcode for if it is read or write
                    *written_opcode <<= 1;
                    *written_opcode |= if wires.data_input { 1 } else { 0 };
                }
                EepromOperationStage::WaitingAddress {
                    ref mut written_address,
                } => {
                    // Shift in address for read or write
                    *written_address <<= 1;
                    *written_address |= if wires.data_input { 1 } else { 0 };
                }
                EepromOperationStage::Reading { address } => {
                    let total_bit_offset = address * 16 + self.bit_index;
                    // Compute which word and which bit in that word to read, wrap around data
                    let word_index = (total_bit_offset / 16) as usize % self.data.len();
                    let bit_index = total_bit_offset % 16;

                    let word = self.data[word_index];
                    let mask = 0x8000 >> bit_index;
                    wires.data_output = word & mask != 0;
                }
            }
        } else {
            // High -> Low: Increment index, update stage accordingly
            self.bit_index += 1;

            match self.stage {
                EepromOperationStage::WaitingOpcode { written_opcode } => {
                    if self.bit_index == OPCODE_BITS {
                        match written_opcode {
                            MICROWIRE_OPCODE_READ => {
                                self.stage =
                                    EepromOperationStage::WaitingAddress { written_address: 0 }
                            }
                            op => {
                                return Err(anyhow!(
                                    "Unknown/Unimplemented microwire opcode {:x}",
                                    op
                                ))
                            }
                        }
                        self.bit_index = 0;
                    }
                }
                EepromOperationStage::WaitingAddress { written_address } => {
                    if self.bit_index == ADDRESS_BITS {
                        self.stage = EepromOperationStage::Reading {
                            address: written_address,
                        };

                        self.bit_index = 0;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn pack_initial_eeprom(&mut self) {
        let mut pack = self.initial_eeprom.pack().unwrap();
        pack.reverse();

        let mut sum = Wrapping(0u16);

        // Skip copying last word, since checksum word will be placed there
        for (i, chunk) in pack[..pack.len() - 2].chunks_exact(2).enumerate() {
            let mut buffer = [0u8; 2];
            buffer.copy_from_slice(chunk);
            let word = u16::from_le_bytes(buffer);

            self.data[i] = word;
            sum += word;
        }

        // DESIRED_CHECKSUM = sum + checksum word -> checksum word = DESIRED_CHECKSUM - sum
        self.data[self.data.len() - 1] = DESIRED_CHECKSUM.wrapping_sub(sum.0);
    }
}

impl Default for EepromInterface {
    fn default() -> Self {
        EepromInterface {
            data: [0u16; 64],
            initial_eeprom: Default::default(),
            previous_chip_select: false,
            previous_clock: false,
            stage: EepromOperationStage::WaitingOpcode { written_opcode: 0 },
            bit_index: 0,
        }
    }
}

impl E1000 {
    pub fn eecd_write(&mut self) {
        let mut wires = EepromWires {
            clock_input: self.regs.eecd.SK,
            chip_select: self.regs.eecd.CS,
            data_input: self.regs.eecd.DI,
            data_output: false,
        };
        self.eeprom.process_wires(&mut wires).unwrap();
        self.regs.eecd.DO = wires.data_output;
    }
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "128", endian = "msb")]
pub struct Eeprom {
    #[packed_field(bytes = "0:5")] // Words 00h - 02h
    ethernet_address: [u8; 6],
    // Checksum word will be computed automatically
}

impl Eeprom {
    // Provide getter and setter for ethernet_address since it needs to be packed in reverse
    // and endianness attribute doesn't affect byte arrays
    pub fn ethernet_address(&self) -> [u8; 6] {
        let mut b = self.ethernet_address.clone();
        b.reverse();
        b
    }

    pub fn set_ethernet_address(&mut self, mut ethernet_address: [u8; 6]) {
        ethernet_address.reverse();
        self.ethernet_address = ethernet_address;
    }
}
