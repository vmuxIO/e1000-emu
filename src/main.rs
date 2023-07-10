use std::os::fd::AsRawFd;

use libvfio_user::*;
use tempfile::tempfile;

#[derive(Debug)]
struct E1000 {
    bar0_data: [u8; 0x20000],
    initialized: bool,
}

impl Default for E1000 {
    fn default() -> Self {
        Self {
            bar0_data: [0; 0x20000],
            initialized: false,
        }
    }
}

impl Device for E1000 {
    fn log(&self, level: i32, msg: &str) {
        println!("E1000: {} - {}", level, msg);
    }

    fn reset(&mut self, reason: DeviceResetReason) -> Result<(), i32> {
        println!("E1000: Resetting device, Reason: {:?}", reason);
        self.bar0_data = [0; 0x20000];
        self.initialized = false;
        Ok(())
    }

    fn region_access_bar0(
        &mut self, offset: usize, data: &mut [u8], write: bool,
    ) -> Result<usize, i32> {
        // Hacky workarounds
        if offset == 0x8 && !self.initialized {
            // TODO: Listen for Ctrl SLU instead
            self.initialize();
        }

        // Region access
        let len = data.len();

        if write {
            print!("E1000: Writing {:x} bytes to BAR0 at {:x}", len, offset);

            if len < 32 {
                print!(": {:x?} ->", &self.bar0_data[offset..offset + len]);
            }

            self.bar0_data[offset..offset + len].copy_from_slice(data);
        } else {
            print!("E1000: Reading {:x} bytes from BAR0 at {:x}:", len, offset);
            data.copy_from_slice(&self.bar0_data[offset..offset + len]);
        }

        if len < 32 {
            print!(" {:x?}", data);
        }
        println!();

        Ok(len)
    }
}

impl E1000 {
    fn initialize(&mut self) {
        eprintln!("################### E1000: Initializing device #######################");
        self.initialized = true;

        // "Link up"
        self.bar0_data[0x8] |= 0x2;
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
