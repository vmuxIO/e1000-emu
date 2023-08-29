use anyhow::{Context, Result};
use libvfio_user::*;
use packed_struct::PackedStruct;

use crate::descriptors::*;
use crate::eeprom::EepromInterface;
use crate::net::Interface;
use crate::phy::Phy;
use crate::registers::*;

mod descriptors;
mod eeprom;
mod net;
mod phy;
mod registers;
mod util;

pub struct E1000 {
    pub ctx: DeviceContext,
    regs: Registers,
    fallback_buffer: [u8; 0x20000],
    io_addr: u32,
    pub eeprom: EepromInterface,
    phy: Phy,

    rx_ring: Option<DescriptorRing<ReceiveDescriptor>>,
    tx_ring: Option<DescriptorRing<TransmitDescriptor>>,

    pub interface: Interface,
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
            phy: Default::default(),
            rx_ring: None,
            tx_ring: None,
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
        self.regs
            .set_mac(self.eeprom.initial_eeprom.ethernet_address());
        self.phy = Default::default();

        // Remove previous rx, tx rings and the buffers they pointed at
        self.rx_ring = None;
        self.tx_ring = None;
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
            self.phy.status.link_status = true;
            self.report_lsc();
        }
    }

    fn ics_write(&mut self) {
        // Client can manually trigger interrupts through this register,
        // Just have to check if they are masked off

        // Check mask by checking if any bit is set, instead of comparing all fields
        let cause = u32::from_ne_bytes(self.regs.interrupt_cause.pack().unwrap());
        let mask = u32::from_ne_bytes(self.regs.interrupt_mask.pack().unwrap());

        if cause & mask != 0 {
            self.interrupt();
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
            Some(ref mut tx_ring) => {
                // Software wants to transmit packets
                tx_ring.tail = self.regs.td_t.tail as usize;

                while !tx_ring.is_empty() {
                    let mut changed_descriptor = tx_ring.read_head().unwrap();
                    if changed_descriptor.cmd_dext {
                        todo!("Only legacy TX descriptors are currently supported");
                    }

                    // Null descriptors should only occur in *receive* descriptor padding
                    assert_ne!(
                        changed_descriptor.buffer, 0,
                        "Transmit descriptor buffer is null"
                    );

                    // Send packet/frame
                    let mapping = tx_ring
                        .get_descriptor_mapping(
                            &mut self.ctx,
                            changed_descriptor.buffer,
                            tx_ring.head,
                        )
                        .unwrap();
                    let length = changed_descriptor.length as usize;
                    let buffer = &mapping.dma(0)[..length];
                    let sent = self.interface.send(buffer).unwrap();
                    assert_eq!(length, sent, "Did not send specified packet length");
                    eprintln!("E1000: Sent {} bytes!", sent);

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
    pub fn receive(&mut self) -> Result<()> {
        let rx_ring = self
            .rx_ring
            .as_mut()
            .context("RX Ring not yet initialized")?;

        let mut has_received_packets = false;
        loop {
            let mut descriptor = rx_ring.read_head()?;

            let mapping =
                rx_ring.get_descriptor_mapping(&mut self.ctx, descriptor.buffer, rx_ring.head)?;
            let buffer = mapping.dma_mut(0);

            let length = match self.interface.receive(buffer)? {
                Some(length) => length,
                None => {
                    break;
                }
            };
            eprintln!("E1000: Received {} bytes!", length);

            // With the linux kernel driver packets seem to be cut short 4 bytes, so increase length
            descriptor.length = length as u16 + 4;
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

    // MDI/O Access Complete
    fn report_mdac(&mut self) {
        if self.regs.interrupt_mask.MDAC {
            self.regs.interrupt_cause.MDAC = true;
            self.interrupt();
        }
    }
}
