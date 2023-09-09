use std::io::ErrorKind;
use std::os::fd::{AsRawFd, RawFd};
use std::process::Command;

use log::info;
use tun_tap::{Iface, Mode};

// Start name with "tap" to avoid systemd-networkd from managing it (if configured this way)
const INTERFACE_NAME: &str = "tapemu%d";
// Route whole ip range to aid testing since host kernel automatically replies to
// pings to 10.1.0.1 but will route 10.1.0.2 to tap interface
const INTERFACE_IP: &str = "10.1.0.1/24";

pub struct Interface {
    interface: Iface,
}

impl Interface {
    pub fn initialize(non_blocking: bool) -> Self {
        let interface = Iface::without_packet_info(INTERFACE_NAME, Mode::Tap).unwrap();

        if non_blocking {
            interface.set_non_blocking().unwrap();
        }

        Command::new("ip")
            .args(["address", "add", INTERFACE_IP, "dev", interface.name()])
            .spawn()
            .unwrap()
            .wait()
            .unwrap();

        Command::new("ip")
            .args(["link", "set", "up", interface.name()])
            .spawn()
            .unwrap()
            .wait()
            .unwrap();

        info!("Interface \"{}\" setup!", interface.name());

        Interface { interface }
    }

    pub fn send(&self, buffer: &[u8]) -> std::io::Result<usize> {
        self.interface.send(buffer)
    }

    pub fn receive(&self, buffer: &mut [u8]) -> std::io::Result<Option<usize>> {
        // Instead of returning WouldBlock error, return None
        match self.interface.recv(buffer) {
            Ok(length) => Ok(Some(length)),
            Err(err) => {
                if err.kind() == ErrorKind::WouldBlock {
                    Ok(None)
                } else {
                    Err(err)
                }
            }
        }
    }
}

impl AsRawFd for Interface {
    fn as_raw_fd(&self) -> RawFd {
        self.interface.as_raw_fd()
    }
}
