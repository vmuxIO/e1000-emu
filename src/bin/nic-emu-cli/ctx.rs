use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

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
    pub timer: Option<Instant>,
    pub timer_has_changed: bool,

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
        let new_mapping = || {
            self.device_context
                .dma_map(address, length, 1, true, true)
                .unwrap()
        };

        // Update if length increased
        match self.dma_mappings.entry(address) {
            Entry::Occupied(mut entry) => {
                let mapping = entry.get_mut();
                if mapping.region_length(0) < length {
                    *mapping = new_mapping();
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(new_mapping());
            }
        }
    }

    fn dma_read(&mut self, address: usize, buffer: &mut [u8], offset: usize) {
        // Currently does not support reading/writing at an offset
        let mapping = self
            .dma_mappings
            .get_mut(&address)
            .expect("Missing dma mapping, dma_prepare is probably missing");

        mapping.read_into_volatile(0, buffer, offset).unwrap();
        self.dma_read_count += 1;
        self.dma_read_bytes += buffer.len() as u64;
    }

    fn dma_write(&mut self, address: usize, buffer: &[u8], offset: usize) {
        // Currently does not support reading/writing at an offset
        let mapping = self
            .dma_mappings
            .get_mut(&address)
            .expect("Missing dma mapping, dma_prepare is probably missing");

        mapping.write_volatile(0, buffer, offset).unwrap();
        self.dma_write_count += 1;
        self.dma_write_bytes += buffer.len() as u64;
    }

    fn trigger_interrupt(&mut self) {
        self.device_context.trigger_irq(0).unwrap();
        self.interrupt_count += 1;
    }

    fn set_timer(&mut self, duration: Duration) {
        self.timer = Some(Instant::now() + duration);
        self.timer_has_changed = true;
    }

    fn delete_timer(&mut self) {
        self.timer = None;
        self.timer_has_changed = true;
    }
}

impl LibvfioUserContext {
    pub fn new(device_context: Rc<DeviceContext>) -> Self {
        LibvfioUserContext {
            device_context,
            dma_mappings: Default::default(),
            timer: None,
            timer_has_changed: false,
            interface: None,
            interrupt_count: 0,
            dma_read_count: 0,
            dma_read_bytes: 0,
            dma_write_count: 0,
            dma_write_bytes: 0,
        }
    }
}
