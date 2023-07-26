use std::process::Command;

use tun_tap::{Iface, Mode};

const INTERFACE_NAME: &str = "emutap%d";
const INTERFACE_IP: &str = "172.16.12.34/32";

pub struct Interface {
    interface: Iface,
}

impl Interface {
    pub fn initialize() -> Self {
        let interface = Iface::without_packet_info(INTERFACE_NAME, Mode::Tap).unwrap();
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

        println!("Interface \"{}\" setup!", interface.name());

        Interface { interface }
    }

    pub fn send(&self, buffer: &[u8]) {
        match self.interface.send(buffer) {
            Ok(length) => {
                println!("Interface: Sent {} bytes", length);
            }
            Err(err) => {
                println!("Interface: Send error: {}", err);
            }
        }
    }
}
