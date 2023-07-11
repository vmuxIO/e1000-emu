use std::os::fd::AsRawFd;

use libvfio_user::*;
use tempfile::tempfile;

use crate::registers::*;

mod registers;

const OFFSET_CTRL: usize = 0x0;
const OFFSET_STATUS: usize = 0x8;

#[derive(Debug)]
struct E1000 {
    ctrl: Ctrl,
    status: Status,
    leftover_data: [u8; 0x20000],
}

impl Default for E1000 {
    fn default() -> Self {
        Self {
            leftover_data: [0; 0x20000],
            ctrl: Default::default(),
            status: Default::default(),
        }
    }
}

impl Device for E1000 {
    fn log(&self, level: i32, msg: &str) {
        if level <= 6 {
            println!("E1000: {} - {}", level, msg);
        }
    }

    fn reset(&mut self, reason: DeviceResetReason) -> Result<(), i32> {
        println!("E1000: Resetting device, Reason: {:?}", reason);
        self.leftover_data = [0; 0x20000];
        Ok(())
    }

    fn region_access_bar0(
        &mut self, offset: usize, data: &mut [u8], write: bool,
    ) -> Result<usize, i32> {
        // Check size and offset
        if data.len() != 4 && data.len() != 8 {
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

        // TODO: Use some sort of map instead of matching offsets ourselves
        match offset {
            // Control register
            OFFSET_CTRL => {
                if write {
                    self.ctrl.write(data).unwrap();
                    self.ctrl_access(write);
                } else {
                    self.ctrl.read(data).unwrap();
                }
            }
            // Status register
            OFFSET_STATUS => {
                if write {
                    eprintln!("E1000: Attempted to write into status register");
                } else {
                    self.status.read(data).unwrap();
                    eprintln!(
                        "E1000: Reading status register: {:?} -> {:?}",
                        self.status, data
                    );
                }
            }
            // Unimplemented registers, just save values
            _ => {
                let len = data.len();
                if write {
                    print!(
                        "E1000: Writing {:x} bytes to BAR0 at {:x}: {:x?} ->",
                        len,
                        offset,
                        &self.leftover_data[offset..offset + len]
                    );

                    self.leftover_data[offset..offset + len].copy_from_slice(data);
                } else {
                    print!("E1000: Reading {:x} bytes from BAR0 at {:x}:", len, offset);
                    data.copy_from_slice(&self.leftover_data[offset..offset + len]);
                }
                println!(" {:x?}", data);
            }
        }

        Ok(data.len())
    }
}

impl E1000 {
    fn ctrl_access(&mut self, _write: bool) {
        println!("E1000: Ctrl access: {:?}", self.ctrl);
        if self.ctrl.SLU {
            self.status.LU = true;
        }
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
