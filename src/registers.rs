// Allow naming fields by their official all upper case abbreviations
#![allow(non_snake_case)]

use anyhow::{anyhow, ensure, Result};
use packed_struct::derive::PackedStruct;
use packed_struct::PackedStruct;

#[allow(unused_variables)]
pub trait Register {
    // read also has mutable reference to self since there are fields that clear after read
    fn read(&mut self, data: &mut [u8]) -> Result<()> {
        Err(anyhow!("Read not implemented"))
    }
    fn write(&mut self, data: &[u8]) -> Result<()> {
        Err(anyhow!("Write not implemented"))
    }
}

impl<T> Register for T
where
    T: PackedStruct<ByteArray = [u8; 4]> + Clone,
{
    fn read(&mut self, data: &mut [u8]) -> Result<()> {
        ensure!(data.len() == 4);
        let mut reg = self.pack().unwrap();
        reg.reverse(); // TODO: Figure out why it needs reversal
        data.copy_from_slice(reg.as_slice());
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> Result<()> {
        ensure!(data.len() == 4);
        let mut reg = [data[0], data[1], data[2], data[3]];
        reg.reverse(); // TODO: Figure out why it needs reversal
        self.clone_from(&T::unpack(&reg).unwrap());
        Ok(())
    }
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4")]
pub struct Ctrl {
    #[packed_field(bits = "6")]
    pub SLU: bool, // Set link up
}

#[derive(PackedStruct, Clone, Default, Debug)]
#[packed_struct(bit_numbering = "lsb0", size_bytes = "4")]
pub struct Status {
    #[packed_field(bits = "1")]
    pub LU: bool, // Link up
}
