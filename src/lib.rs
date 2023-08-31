use anyhow::Result;

pub mod e1000;
mod util;

pub trait NicContext {
    // Send bytes from NIC
    fn send(&mut self, buffer: &[u8]) -> Result<usize>;

    fn dma_read(&mut self, address: usize, buffer: &mut [u8]);
    fn dma_write(&mut self, address: usize, buffer: &[u8]);

    fn trigger_interrupt(&mut self);
}
