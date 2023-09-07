use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Result;
use libvfio_user::dma::DmaMapping;
use libvfio_user::*;
use polling::{Event, PollMode, Poller};

use crate::net::Interface;
use nic_emu::e1000::E1000;
use nic_emu::NicContext;

pub mod net;

// Adapt from NicContext to libvfio_user's DeviceContext
struct LibvfioUserContext {
    device_context: Rc<DeviceContext>,

    // Cache dma mappings instead of releasing them after each op
    dma_mappings: HashMap<usize, DmaMapping>,

    interface: Interface,
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

// Device facing libvfio_user for callbacks, forwarding them to behavioral model
struct E1000Device {
    e1000: E1000<LibvfioUserContext>,
}

impl Device for E1000Device {
    fn new(ctx: Rc<DeviceContext>) -> Self {
        let nic_ctx = LibvfioUserContext {
            device_context: ctx,
            dma_mappings: Default::default(),
            interface: Interface::initialize(true),
        };

        E1000Device {
            e1000: E1000::new(nic_ctx),
        }
    }

    fn log(&self, level: i32, msg: &str) {
        if level <= 6 {
            println!("libvfio-user log: {} - {}", level, msg);
        }
    }

    fn reset(&mut self, reason: DeviceResetReason) -> Result<(), i32> {
        println!("E1000: Resetting device, Reason: {:?}", reason);
        self.e1000.reset_e1000();
        Ok(())
    }

    fn region_access_bar0(
        &mut self, offset: usize, data: &mut [u8], write: bool,
    ) -> Result<usize, i32> {
        self.e1000
            .region_access_bar0(offset, data, write)
            .or_else(|e| {
                eprintln!("E1000: Error accessing Bar0: {}", e);
                Err(22) // EINVAL
            })
    }

    fn region_access_bar1(
        &mut self, offset: usize, data: &mut [u8], write: bool,
    ) -> std::result::Result<usize, i32> {
        self.e1000
            .region_access_bar1(offset, data, write)
            .or_else(|e| {
                eprintln!("E1000: Error accessing Bar1: {}", e);
                Err(22) // EINVAL
            })
    }
}

fn main() {
    // Initialize E1000
    let socket = "/tmp/e1000-emu.sock";

    let config = DeviceConfigurator::default()
        .socket_path(socket.parse().unwrap())
        .overwrite_socket(true)
        .pci_type(PciType::Pci)
        .pci_config(PciConfig {
            vendor_id: 0x8086, // Intel 82540EM Gigabit Ethernet Controller
            device_id: 0x100e,
            subsystem_vendor_id: 0x0000, // Empty subsystem ids
            subsystem_id: 0x0000,
            class_code_base: 0x02, // Ethernet Controller class code
            class_code_subclass: 0x00,
            class_code_programming_interface: 0x00,
            revision_id: 3, // Revision 3, same as in QEMU
        })
        .add_device_region(DeviceRegion {
            region_type: DeviceRegionKind::Bar0,
            size: 0x20000, // 128 KiB
            file_descriptor: -1,
            offset: 0,
            read: true,
            write: true,
            memory: true,
        })
        .add_device_region(DeviceRegion {
            region_type: DeviceRegionKind::Bar1,
            size: 0x40, // 64 B
            file_descriptor: -1,
            offset: 0,
            read: true,
            write: true,
            memory: false,
        })
        .using_interrupt_requests(InterruptRequestKind::IntX, 1)
        .using_interrupt_requests(InterruptRequestKind::Msi, 1)
        .setup_dma(true)
        .non_blocking(true)
        .build()
        .unwrap();

    let mut e1000_device = config.produce::<E1000Device>().unwrap();
    println!("VFU context created successfully");

    // Setup initial eeprom, should not be changed afterwards

    // Set to test mac
    // x2-... is in locally administered range and should hopefully not conflict with anything
    e1000_device
        .e1000
        .eeprom
        .initial_eeprom
        .set_ethernet_address([0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);
    e1000_device.e1000.eeprom.pack_initial_eeprom();

    // Use same poller and event list for both attach and run
    let poller = Poller::new().unwrap();
    let mut events = vec![];

    const EVENT_KEY_ATTACH: usize = 0;
    const EVENT_KEY_RUN: usize = 1;
    const EVENT_KEY_RECEIVE: usize = 2;

    let ctx = e1000_device.e1000.nic_ctx.device_context.clone();

    // 1. Wait for client to attach

    println!("Attaching...");
    poller.add(&ctx, Event::all(EVENT_KEY_ATTACH)).unwrap();

    loop {
        events.clear();
        poller.wait(&mut events, None).unwrap();

        match ctx.attach().unwrap() {
            Some(_) => {
                break;
            }
            None => {
                // Renew fd, not using Edge mode like we do below for run() since
                // attach probably succeeds fine the first time
                poller.modify(&ctx, Event::all(EVENT_KEY_ATTACH)).unwrap();
            }
        }
    }
    // Fd is auto-removed from poller since it polled in the default Oneshot mode

    // 2. Process client requests

    println!("Running...");
    // Removed and now adding it again since file descriptor may change after attach
    // Poll in Edge mode to avoid having to set interest again and again
    poller
        .add_with_mode(&ctx, Event::all(EVENT_KEY_RUN), PollMode::Edge)
        .unwrap();
    poller
        .add_with_mode(
            &e1000_device.e1000.nic_ctx.interface,
            Event::all(EVENT_KEY_RECEIVE),
            PollMode::Edge,
        )
        .unwrap();

    // Buffer for received packets interface
    let mut interface_buffer = [0u8; 4096]; // Big enough

    loop {
        events.clear();
        poller.wait(&mut events, None).unwrap();

        for event in &events {
            match event.key {
                EVENT_KEY_RUN => {
                    ctx.run().unwrap();
                }
                EVENT_KEY_RECEIVE => loop {
                    match e1000_device
                        .e1000
                        .nic_ctx
                        .interface
                        .receive(&mut interface_buffer)
                        .unwrap()
                    {
                        Some(len) => {
                            println!("E1000: Received {} bytes!", len);
                            match e1000_device.e1000.receive(&interface_buffer[..len]) {
                                Ok(_) => {}
                                Err(err) => {
                                    println!("Error handling receive event, skipping ({})", err);
                                }
                            }
                        }
                        None => {
                            break;
                        }
                    }
                },
                x => {
                    unreachable!("Unknown event key {}", x);
                }
            }
        }
    }
    // Fd would need to be removed if break is added in the future
    //poller.delete(&e1000.ctx).unwrap();
    //poller.delete(&e1000.interface).unwrap();
}
