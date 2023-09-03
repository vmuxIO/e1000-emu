use anyhow::Result;

pub mod e1000;
mod util;

pub trait NicContext {
    // Send bytes from NIC
    fn send(&mut self, buffer: &[u8]) -> Result<usize>;

    /// Prepare range in which future dma operations will take place
    fn dma_prepare(&mut self, _address: usize, _length: usize) {} // Optional to implement

    fn dma_read(&mut self, address: usize, buffer: &mut [u8]);
    fn dma_write(&mut self, address: usize, buffer: &[u8]);

    fn trigger_interrupt(&mut self);
}
