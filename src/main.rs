use std::collections::HashMap;

use anyhow::{Context, Result};
use libvfio_user::dma::DmaMapping;
use libvfio_user::*;
use polling::{Event, PollMode, Poller};

use crate::descriptors::*;
use crate::eeprom::EepromInterface;
use crate::net::Interface;
use crate::registers::*;

mod descriptors;
mod eeprom;
mod net;
mod registers;
mod util;

pub struct E1000 {
    ctx: DeviceContext,
    regs: Registers,
    fallback_buffer: [u8; 0x20000],
    io_addr: u32,
    eeprom: EepromInterface,

    rx_ring: Option<DescriptorRing<ReceiveDescriptor>>,
    tx_ring: Option<DescriptorRing<TransmitDescriptor>>,
    packet_buffers: HashMap<u64, DmaMapping>,

    interface: Interface,
}

impl Device for E1000 {
    fn new(ctx: DeviceContext) -> Self {
        let interface = Interface::initialize(true);

        E1000 {
            ctx,
            regs: Default::default(),
            fallback_buffer: [0; 0x20000],
            io_addr: 0,
            eeprom: Default::default(),
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

    // Bar1 IO proxies access to bar0
    fn region_access_bar1(
        &mut self, offset: usize, data: &mut [u8], write: bool,
    ) -> std::result::Result<usize, i32> {
        const IO_REGISTER_SIZE: usize = 4;

        if data.len() != IO_REGISTER_SIZE {
            eprintln!("Unsupported bar1 access size {:x}", data.len());
            return Err(22); //EINVAL
        }

        match offset {
            0 => {
                // IOADDR: Set where to read/write from/to
                match write {
                    true => {
                        let mut buffer = [0u8; IO_REGISTER_SIZE];
                        buffer.copy_from_slice(data);
                        self.io_addr = u32::from_le_bytes(buffer);
                    }
                    false => data.copy_from_slice(self.io_addr.to_le_bytes().as_slice()),
                }
                Ok(IO_REGISTER_SIZE)
            }
            4 => {
                // IODATA: Access data at previously written IOADDR
                self.region_access_bar0(self.io_addr as usize, data, write)
            }
            x => {
                eprintln!("Unsupported bar1 access at offset {:x}", x);
                Err(22) //EINVAL
            }
        }
    }
}

impl E1000 {
    fn reset_e1000(&mut self) {
        self.regs = Default::default();
        self.fallback_buffer = [0; 0x20000];
        // Set to test mac
        // x2-... is in locally administered range and should hopefully not conflict with anything
        self.regs
            .set_mac(self.eeprom.initial_eeprom.ethernet_address());

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
            self.report_lsc();
        }
    }

    fn rctl_write(&mut self) {
        if self.regs.rctl.EN && self.rx_ring.is_none() {
            self.setup_rx_ring();
        }
    }

    fn tctl_write(&mut self) {
        if self.regs.tctl.EN && self.tx_ring.is_none() {
            self.setup_tx_ring();
        }
    }

    fn rdt_write(&mut self) {
        match &mut self.rx_ring {
            None => {
                // RDT was just initialized, if rx is enabled try to initialize ring
                if self.regs.rctl.EN {
                    self.setup_rx_ring();
                }
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
                // TDT was just initialized, if tx is enabled try to initialize ring
                if self.regs.tctl.EN {
                    self.setup_tx_ring();
                }
            }
            Some(tx_ring) => {
                // Software wants to transmit packets
                tx_ring.tail = self.regs.td_t.tail as usize;

                while !tx_ring.is_empty() {
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
                    self.interface.send(buffer).unwrap();

                    // Done processing, report if requested
                    if changed_descriptor.cmd_rs {
                        changed_descriptor.status_dd = true;
                        tx_ring.write_and_advance_head(changed_descriptor).unwrap();
                    } else {
                        tx_ring.advance_head();
                    }
                    self.regs.td_h.head = tx_ring.head as u16;
                }
            }
        }
    }

    // Receive available frames and place them inside rx-ring
    fn receive(&mut self) -> Result<()> {
        let rx_ring = self
            .rx_ring
            .as_mut()
            .context("RX Ring not yet initialized")?;

        let mut has_received_packets = false;
        loop {
            let mut descriptor = rx_ring.read_head()?;

            let mapping = self
                .packet_buffers
                .get_mut(&descriptor.buffer)
                .context("Packet buffer not found")?;
            let buffer = mapping.dma_mut(0);

            let length = match self.interface.receive(buffer)? {
                Some(length) => length,
                None => {
                    break;
                }
            };

            descriptor.length = length as u16;
            descriptor.status_eop = true;
            descriptor.status_dd = true;

            rx_ring.write_and_advance_head(descriptor)?;
            self.regs.rd_h.head = rx_ring.head as u16;

            has_received_packets = true;
        }
        if has_received_packets {
            // Workaround: Report rxt0 even though we don't emulate any timer
            self.report_rxt0();
        }

        Ok(())
    }

    fn interrupt(&mut self) {
        println!(
            "Triggering interrupt, set causes: {:?}",
            self.regs.interrupt_cause
        );
        self.ctx.trigger_irq(0).unwrap();
    }

    // Link Status Change
    fn report_lsc(&mut self) {
        if self.regs.interrupt_mask.LSC {
            self.regs.interrupt_cause.LSC = true;
            self.interrupt();
        }
    }

    // Link Status Change
    fn report_rxt0(&mut self) {
        if self.regs.interrupt_mask.RXT0 {
            self.regs.interrupt_cause.RXT0 = true;
            self.interrupt();
        }
    }
}

fn main() {
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
        .using_interrupt_requests(InterruptRequestKind::Msi, 1)
        .setup_dma(true)
        .non_blocking(true)
        .build()
        .unwrap();

    let mut e1000 = config.produce::<E1000>().unwrap();
    println!("VFU context created successfully");

    // Setup initial eeprom, should not be changed afterwards
    e1000
        .eeprom
        .initial_eeprom
        .set_ethernet_address([0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);
    e1000.eeprom.pack_initial_eeprom();

    // Use same poller and event list for both attach and run
    let poller = Poller::new().unwrap();
    let mut events = vec![];

    const EVENT_KEY_ATTACH: usize = 0;
    const EVENT_KEY_RUN: usize = 1;
    const EVENT_KEY_RECEIVE: usize = 2;

    // 1. Wait for client to attach

    println!("Attaching...");
    poller
        .add(&e1000.ctx, Event::all(EVENT_KEY_ATTACH))
        .unwrap();

    loop {
        events.clear();
        poller.wait(&mut events, None).unwrap();

        match e1000.ctx.attach().unwrap() {
            Some(_) => {
                break;
            }
            None => {
                // Renew fd, not using Edge mode like we do below for run() since
                // attach probably succeeds fine the first time
                poller
                    .modify(&e1000.ctx, Event::all(EVENT_KEY_ATTACH))
                    .unwrap();
            }
        }
    }
    // Fd is auto-removed from poller since it polled in the default Oneshot mode

    // 2. Process client requests

    println!("Running...");
    // Removed and now adding it again since file descriptor may change after attach
    // Poll in Edge mode to avoid having to set interest again and again
    poller
        .add_with_mode(&e1000.ctx, Event::all(EVENT_KEY_RUN), PollMode::Edge)
        .unwrap();
    poller
        .add_with_mode(
            &e1000.interface,
            Event::all(EVENT_KEY_RECEIVE),
            PollMode::Edge,
        )
        .unwrap();

    loop {
        events.clear();
        poller.wait(&mut events, None).unwrap();

        for event in &events {
            match event.key {
                EVENT_KEY_RUN => {
                    e1000.ctx().run().unwrap();
                }
                EVENT_KEY_RECEIVE => match e1000.receive() {
                    Ok(_) => {}
                    Err(err) => {
                        println!("Error handling receive event, skipping ({})", err);
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
