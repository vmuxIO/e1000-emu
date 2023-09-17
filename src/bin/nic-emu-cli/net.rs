use std::io::ErrorKind;
use std::os::fd::{AsRawFd, RawFd};
use std::process::Command;

use ipnet::IpNet;
use log::{debug, info, warn};
use tun_tap::{Iface, Mode};

pub struct Interface {
    interface: Iface,
}

impl Interface {
    pub fn initialize(non_blocking: bool, tap_name: &str, net: Option<IpNet>) -> Self {
        let interface = Iface::without_packet_info(tap_name, Mode::Tap).unwrap();

        if non_blocking {
            interface.set_non_blocking().unwrap();
        }

        if let Some(ip_net) = net {
            let ip_net = ip_net.to_string();
            let mut cmd_ip_add = Command::new("ip");
            cmd_ip_add.args(["address", "add", &ip_net, "dev", interface.name()]);

            let mut cmd_ip_up = Command::new("ip");
            cmd_ip_up.args(["link", "set", "up", interface.name()]);

            debug!("Running {:?}", cmd_ip_add);
            cmd_ip_add.spawn().unwrap().wait().unwrap();

            debug!("Running {:?}", cmd_ip_up);
            cmd_ip_up.spawn().unwrap().wait().unwrap();
        } else {
            warn!(
                "No automatic interface setup was specified (via --net), \
                 make sure link is up before attaching"
            )
        }

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
