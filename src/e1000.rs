use anyhow::{anyhow, ensure, Context, Result};
use packed_struct::PackedStruct;

use crate::e1000::descriptors::*;
use crate::e1000::eeprom::EepromInterface;
use crate::e1000::phy::Phy;
use crate::e1000::registers::Registers;
use crate::NicContext;

mod descriptors;
mod eeprom;
mod phy;
mod registers;
mod transmit;

pub struct E1000<C: NicContext> {
    pub nic_ctx: C,
    regs: Registers,
    io_addr: u32,
    pub eeprom: EepromInterface,
    phy: Phy,

    rx_ring: Option<DescriptorRing>,
    tx_ring: Option<DescriptorRing>,
}

impl<C: NicContext> E1000<C> {
    pub fn new(nic_ctx: C) -> Self {
        E1000 {
            nic_ctx,
            regs: Default::default(),
            io_addr: 0,
            eeprom: Default::default(),
            phy: Default::default(),
            rx_ring: None,
            tx_ring: None,
        }
    }

    pub fn region_access_bar0(
        &mut self, offset: usize, data: &mut [u8], write: bool,
    ) -> Result<usize> {
        // Check size and offset
        ensure!(data.len() == 4, "Bar0 accesses need to be 4 bytes in size");
        ensure!(
            offset % 4 == 0,
            "Bar0 access offset needs to be at multiple of 4 bytes"
        );

        match self.access_register(offset as u32, data, write) {
            Some(result) => result.unwrap(),
            None => {
                println!(
                    "Unmatched register {} at {:x}",
                    if write { "write" } else { "read" },
                    offset
                );
            }
        }

        Ok(data.len())
    }

    // Bar1 IO proxies access to bar0
    pub fn region_access_bar1(
        &mut self, offset: usize, data: &mut [u8], write: bool,
    ) -> Result<usize> {
        const IO_REGISTER_SIZE: usize = 4;
        ensure!(
            data.len() == IO_REGISTER_SIZE,
            "Bar1 accesses need to be 4 bytes in size"
        );

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
            x => Err(anyhow!("Unsupported bar1 access at offset {:x}", x)),
        }
    }

    pub fn reset_e1000(&mut self) {
        self.regs = Default::default();
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
        self.process_tx_ring();
    }

    // Place received frame inside rx-ring
    pub fn receive(&mut self, received: &[u8]) -> Result<()> {
        assert!(received.len() > 0, "receive called with no data");

        let rx_ring = self
            .rx_ring
            .as_mut()
            .context("RX Ring not yet initialized")?;

        let mut descriptor: ReceiveDescriptor = rx_ring.read_head(&mut self.nic_ctx)?;

        let mut buffer = [0u8; DESCRIPTOR_BUFFER_SIZE];
        buffer[..received.len()].copy_from_slice(received);

        // With the linux kernel driver packets seem to be cut short 4 bytes, so increase length
        descriptor.length = received.len() as u16 + 4;
        descriptor.status_eop = true;
        descriptor.status_dd = true;

        self.nic_ctx.dma_write(descriptor.buffer as usize, &buffer);

        rx_ring.write_and_advance_head(descriptor, &mut self.nic_ctx)?;
        self.regs.rd_h.head = rx_ring.head as u16;

        // Workaround: Report rxt0 even though we don't emulate any timer
        self.report_rxt0();

        Ok(())
    }

    fn interrupt(&mut self) {
        println!(
            "Triggering interrupt, set causes: {:?}",
            self.regs.interrupt_cause
        );
        self.nic_ctx.trigger_interrupt();
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
