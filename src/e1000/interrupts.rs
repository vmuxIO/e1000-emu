use std::time::Instant;

use log::{trace, warn};
use packed_struct::PackedStruct;

use crate::e1000::E1000;
use crate::NicContext;

pub(crate) struct InterruptMitigation {
    expiration: Instant,
    /// Interrupt will be asserted after mitigation ends to inform system over skipped interrupts
    interrupt_after: bool,
}

impl InterruptMitigation {
    fn is_active_at(&self, time: Instant) -> bool {
        self.expiration > time
    }

    fn is_active(&self) -> bool {
        self.is_active_at(Instant::now())
    }
}

impl<C: NicContext> E1000<C> {
    pub fn timer_elapsed(&mut self) {
        trace!("Timer elapsed");
        if !self.enable_interrupt_mitigation {
            warn!("Timer elapsed called, but interrupt mitigation is disabled");
        }

        if let Some(mitigation) = &self.interrupt_mitigation {
            if !mitigation.interrupt_after {
                warn!("Timer elapsed called, but timer was not supposed to be scheduled");
                return;
            }

            if mitigation.is_active() {
                warn!(
                    "Timer elapsed called too early, mitigation still active for {:?}",
                    mitigation
                        .expiration
                        .saturating_duration_since(Instant::now())
                );
                return;
            }
        } else {
            warn!("Timer elapsed called, but mitigation is already disabled");
            return;
        }

        // Clear before interrupting, skips checks and potential timer deletion,
        // which wouldn't be needed anyway since timer should operate in one-shot mode
        self.interrupt_mitigation = None;
        self.interrupt();
    }

    /// Trigger
    pub(crate) fn interrupt(&mut self) {
        // Interrupt cause register may always be set,
        // but only generate PCI interrupt if at least one cause is not masked off

        // Check mask by checking if any bit is set, instead of comparing all fields
        let cause = u32::from_ne_bytes(self.regs.interrupt_cause.pack().unwrap());
        let mask = u32::from_ne_bytes(self.regs.interrupt_mask.pack().unwrap());

        // Interrupt mitigation
        if let Some(mitigation) = &mut self.interrupt_mitigation {
            if cause & mask == 0 {
                return;
            }
            let now = Instant::now();
            if mitigation.is_active_at(now) {
                trace!("Skipping interrupt, mitigation active");

                // Schedule timer to assert interrupt after mitigation ends
                if !mitigation.interrupt_after {
                    let delay = mitigation.expiration - now;
                    trace!("Scheduling timer for in {:?}", delay);
                    self.nic_ctx.set_timer(delay);
                    mitigation.interrupt_after = true;
                }
                return;
            }

            // Else this interrupt was triggered by other cause that was randomly called
            // at just the right time
            // (couldn't be called by timer since it clears self.interrupt_mitigation before call)
            if mitigation.interrupt_after {
                trace!("Interrupt mitigation expired before timer called, so deleting timer");
                self.nic_ctx.delete_timer();
            }
            self.interrupt_mitigation = None;
        }

        trace!(
            "Triggering interrupt, set causes: {:?}",
            self.regs.interrupt_cause
        );
        self.nic_ctx.trigger_interrupt(cause & mask == 0);

        // Re-arm interrupt throttling timer (if enabled)
        // This should not lead to an infinite loop, as this doesn't set timer yet
        if self.enable_interrupt_mitigation {
            if let Some(duration) = self.regs.interrupt_throttling.get_itr_interval() {
                trace!("Mitigating interrupts for next {:?}", duration);
                self.interrupt_mitigation = Some(InterruptMitigation {
                    expiration: Instant::now() + duration,
                    // No timer is needed until interrupts are reported during mitigation
                    interrupt_after: false,
                })
            }
        }
    }

    /// Transmit Descriptor Written Back & Transmit Queue Empty
    /// (With the latter always being the case after the former in this behavioral model)
    pub(crate) fn report_txdw_and_txqe(&mut self) {
        trace!("Reporting: Transmit Descriptor Written Back AND Transmit Queue Empty");
        self.regs.interrupt_cause.TXDW = true;
        self.regs.interrupt_cause.TXQE = true;

        self.interrupt();
    }

    /// Transmit Queue Empty
    pub(crate) fn report_txqe(&mut self) {
        trace!("Reporting: Transmit Queue Empty");
        self.regs.interrupt_cause.TXQE = true;
        self.interrupt();
    }

    /// Link Status Change
    pub(crate) fn report_lsc(&mut self) {
        trace!("Reporting: Link Status Change");
        self.regs.interrupt_cause.LSC = true;
        self.interrupt();
    }

    /// Receiver Timer Interrupt
    pub(crate) fn report_rxt0(&mut self) {
        trace!("Reporting: Receiver Timer Interrupt");
        // if !self.regs.interrupt_cause.RXT0 {
            self.regs.interrupt_cause.RXT0 = true;
            self.interrupt();
        // }
    }

    /// MDI/O Access Complete
    pub(crate) fn report_mdac(&mut self) {
        trace!("Reporting: MDI/O Access Complete");
        self.regs.interrupt_cause.MDAC = true;
        self.interrupt();
    }
}
