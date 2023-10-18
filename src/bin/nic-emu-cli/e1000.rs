use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Result;
use libvfio_user::*;
use log::{debug, error, info, log, Level};
use macaddr::MacAddr6;

use crate::ctx::LibvfioUserContext;
use nic_emu::e1000::E1000;

// Device facing libvfio_user for callbacks, forwarding them to behavioral model
pub struct E1000Device {
    pub e1000: E1000<LibvfioUserContext>,
}

impl Device for E1000Device {
    fn new(ctx: Rc<DeviceContext>) -> Self {
        let nic_ctx = LibvfioUserContext::new(ctx);

        E1000Device {
            e1000: E1000::new(nic_ctx, true),
        }
    }

    fn log(&self, level: i32, msg: &str) {
        log!(
            // Match syslog levels
            match level {
                0 => panic!("libvfio-user panic: {}", msg),
                1..=4 => Level::Error,
                5 => Level::Warn,
                6 => Level::Info,
                7 => Level::Debug,
                8.. => Level::Trace,
                _ => unreachable!("Invalid syslog level {}", level),
            },
            "libvfio-user log ({}): {}",
            level,
            msg
        )
    }

    fn reset(&mut self, reason: DeviceResetReason) -> Result<(), i32> {
        info!("Resetting device, Reason: {:?}", reason);
        self.e1000.reset_e1000();
        Ok(())
    }

    fn region_access_bar0(
        &mut self, offset: usize, data: &mut [u8], write: bool,
    ) -> Result<usize, i32> {
        self.e1000
            .region_access_bar0(offset, data, write)
            .or_else(|e| {
                error!("Error accessing Bar0: {}", e);
                Err(22) // EINVAL
            })
    }

    fn region_access_bar1(
        &mut self, offset: usize, data: &mut [u8], write: bool,
    ) -> std::result::Result<usize, i32> {
        self.e1000
            .region_access_bar1(offset, data, write)
            .or_else(|e| {
                error!("Error accessing Bar1: {}", e);
                Err(22) // EINVAL
            })
    }
}

impl E1000Device {
    pub fn build(path: PathBuf, mac: MacAddr6) -> Box<Self> {
        let config = DeviceConfigurator::default()
            .socket_path(path)
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
        debug!("VFU context created successfully");

        // TODO: Move this inside E1000 constructor, would require changes to libvfio-user-rs
        // Setup initial eeprom, should not be changed afterwards
        e1000_device
            .e1000
            .eeprom
            .initial_eeprom
            .set_ethernet_address(mac.into_array());
        e1000_device.e1000.eeprom.pack_initial_eeprom();

        e1000_device
    }
}
