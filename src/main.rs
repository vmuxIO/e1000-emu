use std::os::fd::AsRawFd;

use libvfio_user::*;
use tempfile::tempfile;

use crate::registers::*;

mod registers;
mod util;

#[derive(Debug)]
pub struct E1000 {
    regs: Registers,
    fallback_buffer: [u8; 0x20000],
}

impl Default for E1000 {
    fn default() -> Self {
        Self {
            regs: Default::default(),
            fallback_buffer: [0; 0x20000],
        }
    }
}

impl Device for E1000 {
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
        self.set_mac([0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);
    }

    fn ctrl_access(&mut self, write: bool) {
        if !write {
            return;
        }

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

    fn set_mac(&mut self, mac: [u8; 6]) {
        self.regs.ral0.receive_address_low = u32::from_be_bytes([mac[0], mac[1], mac[2], mac[3]]);
        self.regs.rah0.receive_address_high = u16::from_be_bytes([mac[4], mac[5]]);
    }
}

fn main() {
    let socket = "/tmp/e1000-emu.sock";

    let temp_bar1 = tempfile().unwrap();

    let config = DeviceConfigurator::default()
        .socket_path(socket.parse().unwrap())
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
        .build()
        .unwrap();

    let ctx = config.produce::<E1000>().unwrap();
    println!("VFU context created successfully");

    println!("Attaching...");
    ctx.attach().unwrap().unwrap();

    println!("Running...");
    ctx.run().unwrap();
}
