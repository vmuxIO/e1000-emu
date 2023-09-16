use std::collections::HashMap;
use std::rc::Rc;

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

    pub interface: Interface,
}

impl NicContext for LibvfioUserContext {
    fn send(&mut self, buffer: &[u8]) -> Result<usize> {
        self.interface.send(buffer).map_err(anyhow::Error::msg)
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
    }

    fn dma_write(&mut self, address: usize, buffer: &[u8]) {
        // Currently does not support reading/writing at an offset
        let mapping = self
            .dma_mappings
            .get_mut(&address)
            .expect("Missing dma mapping, dma_prepare is probably missing");

        let dma_buffer = mapping.dma_mut(0);
        dma_buffer[..buffer.len()].copy_from_slice(buffer);
    }

    fn trigger_interrupt(&mut self) {
        self.device_context.trigger_irq(0).unwrap()
    }
}

impl LibvfioUserContext {
    pub fn new(device_context: Rc<DeviceContext>, interface: Interface) -> Self {
        LibvfioUserContext {
            device_context,
            dma_mappings: Default::default(),
            interface,
        }
    }
}
