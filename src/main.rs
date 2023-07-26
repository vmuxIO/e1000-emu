use std::collections::HashMap;
use std::os::fd::AsRawFd;

use libvfio_user::dma::DmaMapping;
use libvfio_user::*;
use tempfile::tempfile;

use crate::descriptors::*;
use crate::net::Interface;
use crate::registers::*;
use crate::util::dummy_frame;

mod descriptors;
mod net;
mod registers;
mod util;

pub struct E1000 {
    ctx: DeviceContext,
    regs: Registers,
    fallback_buffer: [u8; 0x20000],

    rx_ring: Option<DescriptorRing<ReceiveDescriptor>>,
    tx_ring: Option<DescriptorRing<TransmitDescriptor>>,
    packet_buffers: HashMap<u64, DmaMapping>,

    interface: Interface,
}

impl Device for E1000 {
    fn new(ctx: DeviceContext) -> Self {
        let interface = Interface::initialize();

        E1000 {
            ctx,
            regs: Default::default(),
            fallback_buffer: [0; 0x20000],
            rx_ring: None,
            tx_ring: None,
            packet_buffers: Default::default(),
            interface,
        }
    }

    fn ctx(&self) -> &DeviceContext {
        &self.ctx
    }

    fn ctx_mut(&mut self) -> &mut DeviceContext {
        &mut self.ctx
    }

    fn log(&self, level: i32, msg: &str) {
        if level <= 6 {
            println!("libvfio-user log: {} - {}", level, msg);
        }
    }

    fn reset(&mut self, reason: DeviceResetReason) -> Result<(), i32> {
        println!("E1000: Resetting device, Reason: {:?}", reason);
        self.reset_e1000();
        Ok(())
    }

    fn region_access_bar0(
        &mut self, offset: usize, data: &mut [u8], write: bool,
    ) -> Result<usize, i32> {
        // Check size and offset
        if data.len() != 4 {
            if data.len() == 8 {
                unimplemented!("Automatic chunking not yet implemented")
            }
            eprintln!(
                "E1000: Warning: Out of spec region access size: {}, expected 4 or 8",
                data.len()
            );
        }
        if offset % 4 != 0 {
            eprintln!(
                "E1000: Warning: Out of spec region access offset: {}, expected multiple of 4",
                offset
            );
        }

        match self.access_register(offset as u32, data, write) {
            Some(result) => result.unwrap(),
            None => {
                print!("Unmatched register access, redirecting to fallback buffer. ");

                let len = data.len();
                if write {
                    print!(
                        "Writing {:x} bytes at {:x}: {:x?} ->",
                        len,
                        offset,
                        &self.fallback_buffer[offset..offset + len]
                    );

                    self.fallback_buffer[offset..offset + len].copy_from_slice(data);
                } else {
                    print!("Reading {:x} bytes at {:x}:", len, offset);
                    data.copy_from_slice(&self.fallback_buffer[offset..offset + len]);
                }
                println!(" {:x?}", data);
            }
        }

        Ok(data.len())
    }
}

impl E1000 {
    fn reset_e1000(&mut self) {
        self.regs = Default::default();
        self.fallback_buffer = [0; 0x20000];
        // Set to test mac
        // x2-... is in locally administered range and should hopefully not conflict with anything
        self.regs.set_mac([0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);

        // Remove previous rx, tx rings and the buffers they pointed at
        self.rx_ring = None;
        self.tx_ring = None;
        self.packet_buffers = Default::default();
    }

    fn ctrl_write(&mut self) {
        if self.regs.ctrl.RST {
            println!("E1000: Reset by driver.");
            self.reset_e1000();
            return;
        }

        if self.regs.ctrl.SLU {
            println!("E1000: Link up.");
            self.regs.status.LU = true;
        }
    }

    fn rctl_write(&mut self) {
        if self.regs.rctl.EN {
            println!("E1000: Initializing RX.");
            self.initialize_rx_ring();

            // Test receive
            self.receive_dummy();
        }
    }

    fn tctl_write(&mut self) {
        if self.regs.tctl.EN {
            println!("E1000: Initializing TX.");
            self.initialize_tx_ring();
        }
    }

    fn rdt_write(&mut self) {
        match &mut self.rx_ring {
            None => {
                // RDT was just initialized
            }
            Some(rx_ring) => {
                // Software is done with the received packet(s)
                rx_ring.tail = self.regs.rd_t.tail as usize;
            }
        }
    }

    fn tdt_write(&mut self) {
        match &mut self.tx_ring {
            None => {
                // TDT was just initialized
            }
            Some(tx_ring) => {
                // Software wants to transmit packets
                tx_ring.tail = self.regs.td_t.tail as usize;

                while !tx_ring.is_empty() {
                    println!("Transmit frame.");
                    let mut changed_descriptor = tx_ring.read_head().unwrap();

                    // Null descriptors should only occur in *receive* descriptor padding
                    assert_ne!(
                        changed_descriptor.buffer, 0,
                        "Transmit descriptor buffer is null"
                    );

                    // Send packet/frame
                    let mapping = self
                        .packet_buffers
                        .get_mut(&changed_descriptor.buffer)
                        .unwrap();
                    let length = changed_descriptor.length as usize;
                    let buffer = &mapping.dma(0)[..length];
                    self.interface.send(buffer);

                    // Done processing, report if requested
                    if changed_descriptor.cmd_rs {
                        changed_descriptor.status_dd = true;
                        tx_ring.write_and_advance_head(changed_descriptor).unwrap();
                    } else {
                        tx_ring.advance_head();
                    }
                }
            }
        }
    }

    fn receive_dummy(&mut self) {
        let ring = self.rx_ring.as_mut().unwrap();
        let mut descriptor = ring.read_head().unwrap();

        let mapping = self.packet_buffers.get_mut(&descriptor.buffer).unwrap();
        let buffer = mapping.dma_mut(0);

        let frame = dummy_frame();
        buffer[..frame.len()].copy_from_slice(&frame);
        descriptor.length = frame.len() as u16;
        descriptor.status_eop = true;
        descriptor.status_dd = true;

        ring.write_and_advance_head(descriptor).unwrap();
    }
}

fn main() {
    let socket = "/tmp/e1000-emu.sock";

    let temp_bar1 = tempfile().unwrap();

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
            file_descriptor: temp_bar1.as_raw_fd(),
            offset: 0,
            read: true,
            write: true,
            memory: false,
        })
        .setup_dma(true)
        .build()
        .unwrap();

    let e1000 = config.produce::<E1000>().unwrap();
    println!("VFU context created successfully");

    println!("Attaching...");
    e1000.ctx().attach().unwrap().unwrap();

    println!("Running...");
    e1000.ctx().run().unwrap();
}
