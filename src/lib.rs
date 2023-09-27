use std::time::Duration;

use anyhow::Result;

pub mod e1000;
mod ffi;
mod util;

pub trait NicContext {
    // Send bytes from NIC
    fn send(&mut self, buffer: &[u8]) -> Result<usize>;

    /// Prepare range in which future dma operations will take place
    fn dma_prepare(&mut self, _address: usize, _length: usize) {} // Optional to implement

    fn dma_read(&mut self, address: usize, buffer: &mut [u8]);
    fn dma_write(&mut self, address: usize, buffer: &[u8]);

    fn trigger_interrupt(&mut self);

    /// Set or adjust the one-shot timer
    fn set_timer(&mut self, duration: Duration);
    /// Delete timer, timer might not have been set before
    fn delete_timer(&mut self);
}
