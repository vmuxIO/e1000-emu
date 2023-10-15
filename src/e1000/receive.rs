use anyhow::{Context, Result};
use log::{debug, trace};

use crate::e1000::descriptors::*;
use crate::e1000::E1000;
use crate::NicContext;

// Number of hardware owned descriptors to keep in reserve
// TODO: increase if packets are processed to use multiple descriptors
const RX_QUEUE_RESERVE: usize = 1;

#[derive(Debug, PartialEq)]
pub enum ReceiveState {
    Offline,
    Online,
    Throttled,
}

// Simple equality check functions, easier to modify in case ReceiveState is extended
impl ReceiveState {
    pub fn is_ready(&self) -> bool {
        *self == ReceiveState::Online
    }

    pub fn should_defer(&self) -> bool {
        *self == ReceiveState::Throttled
    }
}

impl<C: NicContext> E1000<C> {
    // Place received frame inside rx-ring
    pub fn receive(&mut self, received: &[u8]) -> Result<()> {
        debug!("Receiving {} bytes", received.len());
        assert!(received.len() > 0, "receive called with no data");
        assert!(
            self.receive_state.is_ready(),
            "receive called but nic is not ready"
        );

        let rx_ring = self
            .rx_ring
            .as_mut()
            .context("RX Ring not yet initialized")?;

        let mut descriptor: ReceiveDescriptor = rx_ring.read_head(&mut self.nic_ctx)?;

        // Unless SECRC (Strip Ethernet CRC) is set,
        // a Frame Check Sequence (FCS) is expected to be present at end and already checked by nic,
        // but because we receive just the frame, assume it's ok and increase length to compensate
        // otherwise packets would just be cut short by 4 bytes
        let mut received_length = received.len();
        if !self.regs.rctl.SECRC {
            received_length += 4;
        }

        descriptor.length = received_length as u16;
        descriptor.status_eop = true;
        descriptor.status_dd = true;

        let buffer_size = self.regs.rctl.get_buffer_size();
        if received_length > buffer_size {
            todo!(
                "Multiple RX descriptors per packet not yet supported, buffer size={}B, packet={}B",
                buffer_size,
                received_length,
            );
        }

        let address = descriptor.buffer as usize;
        if address == 0 {
            todo!("RX Descriptor null padding not yet supported");
        }
        self.nic_ctx.dma_prepare(address, buffer_size);
        self.nic_ctx.dma_write(address, &received, 0);

        trace!("Put RX descriptor: {:?}", descriptor);
        rx_ring.write_and_advance_head(descriptor, &mut self.nic_ctx)?;
        self.regs.rd_h.head = rx_ring.head as u16;

        self.update_rx_throttling();

        // Workaround: Report rxt0 even though we don't emulate any timer
        self.report_rxt0();

        Ok(())
    }

    pub fn update_rx_throttling(&mut self) {
        if let Some(rx_ring) = &self.rx_ring {
            let hw_descriptors = rx_ring.hardware_owned_descriptors();

            let currently_throttled = self.receive_state == ReceiveState::Throttled;
            let should_throttle = hw_descriptors <= RX_QUEUE_RESERVE;

            trace!("RX Ring: {} free descriptors remaining", hw_descriptors);

            if !currently_throttled && should_throttle {
                self.receive_state = ReceiveState::Throttled;
                debug!("Throttling RX, ring full");
            } else if currently_throttled && !should_throttle {
                self.receive_state = ReceiveState::Online;
                debug!("RX not throttled anymore.")
            }
        }
    }
}
