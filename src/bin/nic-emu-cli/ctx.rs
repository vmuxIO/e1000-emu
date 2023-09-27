use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use anyhow::Result;
use libvfio_user::dma::DmaMapping;
use libvfio_user::DeviceContext;

use crate::net::Interface;
use nic_emu::NicContext;

// Adapt from NicContext to libvfio_user's DeviceContext
pub struct LibvfioUserContext {
    pub device_context: Rc<DeviceContext>,

    // Cache dma mappings instead of releasing them after each op
    dma_mappings: HashMap<usize, DmaMapping>,

    // Keep track of requested timer and update real timer in main
    pub timer: Option<Duration>,

    pub interface: Option<Interface>, // will be set later

    // Statistics
    pub interrupt_count: u64,
    pub dma_read_count: u64,
    pub dma_read_bytes: u64,
    pub dma_write_count: u64,
    pub dma_write_bytes: u64,
}

impl NicContext for LibvfioUserContext {
    fn send(&mut self, buffer: &[u8]) -> Result<usize> {
        self.interface
            .as_ref()
            .unwrap()
            .send(buffer)
            .map_err(anyhow::Error::msg)
    }

    fn dma_prepare(&mut self, address: usize, length: usize) {
        self.dma_mappings.entry(address).or_insert_with(|| {
            self.device_context
                .dma_map(address, length, 1, true, true)
                .unwrap()
        });
    }

    fn dma_read(&mut self, address: usize, buffer: &mut [u8]) {
        // Currently does not support reading/writing at an offset
        let mapping = self
            .dma_mappings
            .get_mut(&address)
            .expect("Missing dma mapping, dma_prepare is probably missing");

        let dma_buffer = mapping.dma(0);
        buffer.copy_from_slice(&dma_buffer[..buffer.len()]);
        self.dma_read_count += 1;
        self.dma_read_bytes += buffer.len() as u64;
    }

    fn dma_write(&mut self, address: usize, buffer: &[u8]) {
        // Currently does not support reading/writing at an offset
        let mapping = self
            .dma_mappings
            .get_mut(&address)
            .expect("Missing dma mapping, dma_prepare is probably missing");

        let dma_buffer = mapping.dma_mut(0);
        dma_buffer[..buffer.len()].copy_from_slice(buffer);
        self.dma_write_count += 1;
        self.dma_write_bytes += buffer.len() as u64;
    }

    fn trigger_interrupt(&mut self) {
        self.device_context.trigger_irq(0).unwrap();
        self.interrupt_count += 1;
    }

    fn set_timer(&mut self, duration: Duration) {
        self.timer = Some(duration);
    }

    fn delete_timer(&mut self) {
        self.timer = None;
    }
}

impl LibvfioUserContext {
    pub fn new(device_context: Rc<DeviceContext>) -> Self {
        LibvfioUserContext {
            device_context,
            dma_mappings: Default::default(),
            timer: None,
            interface: None,
            interrupt_count: 0,
            dma_read_count: 0,
            dma_read_bytes: 0,
            dma_write_count: 0,
            dma_write_bytes: 0,
        }
    }
}
