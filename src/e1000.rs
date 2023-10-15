use anyhow::{anyhow, ensure, Result};
use log::{info, trace};

use crate::e1000::descriptors::*;
use crate::e1000::eeprom::EepromInterface;
use crate::e1000::interrupts::InterruptMitigation;
use crate::e1000::phy::Phy;
use crate::e1000::receive::ReceiveState;
use crate::e1000::registers::Registers;
use crate::NicContext;

mod descriptors;
mod eeprom;
mod interrupts;
mod phy;
mod receive;
mod registers;
mod transmit;

pub struct E1000<C: NicContext> {
    // Configuration
    pub nic_ctx: C,
    enable_interrupt_mitigation: bool,

    // Status
    pub receive_state: ReceiveState,

    // E1000 internals
    regs: Registers,
    io_addr: u32,
    pub eeprom: EepromInterface,
    phy: Phy,

    // Nic-emu internals
    rx_ring: Option<DescriptorRing>,
    tx_ring: Option<DescriptorRing>,
    transmit_tcp_context: Option<TransmitDescriptorTcpContext>,
    interrupt_mitigation: Option<InterruptMitigation>,
}

impl<C: NicContext> E1000<C> {
    /// Create a new E1000 instance, if mitigate_interrupts is true
    /// the provided nic_ctx must have a one-shot timer implementation calling e1000.timer_elapsed()
    pub fn new(nic_ctx: C, mitigate_interrupts: bool) -> Self {
        E1000 {
            nic_ctx,
            enable_interrupt_mitigation: mitigate_interrupts,
            receive_state: ReceiveState::Offline,
            regs: Default::default(),
            io_addr: 0,
            eeprom: Default::default(),
            phy: Default::default(),
            rx_ring: None,
            tx_ring: None,
            transmit_tcp_context: None,
            interrupt_mitigation: Default::default(),
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
                trace!(
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
        self.receive_state = ReceiveState::Offline;
        self.regs = Default::default();
        self.regs
            .set_mac(self.eeprom.initial_eeprom.ethernet_address());
        self.phy = Default::default();

        // Reset previous rx, tx values
        self.rx_ring = None;
        self.tx_ring = None;
        self.transmit_tcp_context = None;

        // Reset interrupt mitigation
        self.nic_ctx.delete_timer();
        self.interrupt_mitigation = None;
    }

    fn ctrl_write(&mut self) {
        if self.regs.ctrl.RST {
            info!("Reset by driver.");
            self.reset_e1000();
            return;
        }

        if self.regs.ctrl.SLU {
            info!("Link up.");
            self.regs.status.LU = true;
            self.phy.status.link_status = true;
            self.report_lsc();
        }
    }

    fn rctl_write(&mut self) {
        if self.regs.rctl.EN && self.rx_ring.is_none() {
            self.setup_rx_ring();
            self.update_rx_throttling();
        }
    }

    fn tctl_write(&mut self) {
        if self.regs.tctl.EN && self.tx_ring.is_none() {
            self.setup_tx_ring();
        }
    }

    fn rdt_write(&mut self) {
        if let Some(rx_ring) = &mut self.rx_ring {
            // Software is done with the received packet(s)
            rx_ring.tail = self.regs.rd_t.tail as usize;

            self.update_rx_throttling();
        }
        // Else RDT was just initialized
    }

    fn tdt_write(&mut self) {
        self.process_tx_ring();
    }
}
