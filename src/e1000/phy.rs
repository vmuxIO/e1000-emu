use log::{error, trace};
use packed_struct::derive::PackedStruct;
use packed_struct::{PackedStruct, PackedStructSlice};

use crate::e1000::E1000;
use crate::util::match_and_access_registers;
use crate::NicContext;

const MDI_READ: u8 = 0b10;
const MDI_WRITE: u8 = 0b01;

#[derive(Default, Debug)]
pub struct Phy {
    pub status: PhyStatus,
    phy_identifier: PhyIdentifier,
    phy_extended_identifier: PhyExtendedIdentifier,
}

impl<C: NicContext> E1000<C> {
    pub fn mdic_write(&mut self) {
        let offset = self.regs.mdic.register_address;
        let mut data = self.regs.mdic.data.to_be_bytes();
        let write = match self.regs.mdic.opcode {
            MDI_READ => false,
            MDI_WRITE => true,
            _ => {
                error!("Unknown MDIC opcode {:x}", self.regs.mdic.opcode);
                return;
            }
        };

        match_and_access_registers!(offset, data.as_mut_slice(), write, {
            // Offset => Register ( => and also do )
            0x1 => self.phy.status,
            0x2 => self.phy.phy_identifier,
            0x3 => self.phy.phy_extended_identifier,
        } else {
            // Wildcard, if none of the above match
            trace!("Unknown PHY register at {}, data={:?}, write={:?}", offset, data, write);
        });

        if !write {
            self.regs.mdic.data = u16::from_be_bytes(data);
        }

        self.regs.mdic.ready = true;
        if self.regs.mdic.interrupt_enable {
            self.report_mdac();
        }
    }
}

// Phy registers

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "2")]
pub struct PhyStatus {
    #[packed_field(bits = "2")]
    pub link_status: bool,
}

trait PhyRegister {
    fn access(&mut self, data: &mut [u8], write: bool);
}

impl<T> PhyRegister for T
where
    T: PackedStruct<ByteArray = [u8; 2]> + Clone,
{
    fn access(&mut self, data: &mut [u8], write: bool) {
        if write {
            // No reversal necessary (as compared to normal registers),
            // just use from/to be bytes when writing mdic instead
            self.clone_from(&Self::unpack_from_slice(data).unwrap())
        } else {
            self.pack_to_slice(data).unwrap();
        }
    }
}

#[derive(PackedStruct, Clone, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "2", endian = "msb")]
pub struct PhyIdentifier {
    #[packed_field(bits = "0:15")]
    pub identifier: u16, // Organizationally Unique Identifier Bit
}

impl Default for PhyIdentifier {
    fn default() -> Self {
        PhyIdentifier { identifier: 0x0141 } // ID linux kernel driver expects
    }
}

#[derive(PackedStruct, Clone, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "2")]
pub struct PhyExtendedIdentifier {
    #[packed_field(bits = "0:3")]
    pub revision: u8, // Revision Number

    #[packed_field(bits = "4:9")]
    pub model: u8, // Model Number

    #[packed_field(bits = "10:15")]
    pub identifier: u8, // Organizationally Unique Identifier Bit
}

impl Default for PhyExtendedIdentifier {
    fn default() -> Self {
        PhyExtendedIdentifier {
            // IDs linux kernel driver expects
            revision: 0,
            model: 0x02,
            identifier: 0x03,
        }
    }
}
