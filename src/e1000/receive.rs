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

        let mut buffer = [0u8; DESCRIPTOR_BUFFER_SIZE];
        buffer[..received.len()].copy_from_slice(received);

        // With the linux kernel driver packets seem to be cut short 4 bytes, so increase length
        descriptor.length = received.len() as u16 + 4;
        descriptor.status_eop = true;
        descriptor.status_dd = true;

        self.nic_ctx
            .dma_prepare(descriptor.buffer as usize, buffer.len());
        self.nic_ctx.dma_write(descriptor.buffer as usize, &buffer);

        rx_ring.write_and_advance_head(descriptor, &mut self.nic_ctx)?;
        self.regs.rd_h.head = rx_ring.head as u16;

        debug!("Received {} bytes!", received.len());
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

            trace!("RX Ring: {} descriptors remaining", hw_descriptors);

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
