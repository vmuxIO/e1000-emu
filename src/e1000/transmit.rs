use anyhow::{ensure, Result};
use internet_checksum::Checksum;

use crate::e1000::descriptors::*;
use crate::e1000::E1000;
use crate::NicContext;

#[derive(Debug, Default)]
struct TransmitDescriptorSequence {
    data: Vec<u8>,
    done: bool,

    // Options only for tcp transmit descriptors
    tcp_context: Option<TransmitDescriptorTcpContext>,
    _insert_ip_checksum: bool,
    insert_tcp_checksum: bool,
}

impl TransmitDescriptorSequence {
    fn read_to_buffer(
        &mut self, address: usize, length: usize, nic_ctx: &mut dyn NicContext,
    ) -> Result<()> {
        // Null descriptors should only occur in *receive* descriptor padding
        ensure!(address != 0, "Transmit descriptor buffer address is null");

        let old_len = self.data.len();
        self.data.resize(old_len + length, 0);

        nic_ctx.dma_prepare(address, DESCRIPTOR_BUFFER_SIZE);
        nic_ctx.dma_read(address, &mut self.data.as_mut_slice()[old_len..]);
        Ok(())
    }

    fn add_descriptor(
        &mut self, descriptor: &TransmitDescriptor, nic_ctx: &mut dyn NicContext,
    ) -> Result<()> {
        assert!(!self.done);

        match descriptor {
            TransmitDescriptor::Legacy(descriptor) => {
                ensure!(
                    self.tcp_context.is_none(),
                    "Legacy transmit descriptor in tcp sequence"
                );

                if descriptor.cmd_ic {
                    todo!("Inserting checksum in legacy descriptor not implemented yet");
                }

                self.read_to_buffer(
                    descriptor.buffer as usize,
                    descriptor.length as usize,
                    nic_ctx,
                )?;

                self.done = descriptor.cmd_eop;
            }
            TransmitDescriptor::TcpContext(descriptor) => {
                ensure!(
                    self.tcp_context.is_none(),
                    "Second tcp context transmit descriptor in sequence"
                );

                if descriptor.tucmd_tse {
                    todo!("TCP Segmentation not yet implemented")
                }

                self.tcp_context = Some(descriptor.clone());
            }
            TransmitDescriptor::TcpData(descriptor) => {
                ensure!(
                    self.tcp_context.is_some(),
                    "Tcp data transmit descriptor without context in sequence"
                );

                // Only insert checksum options in first descriptor are valid
                // (even though kernel driver seems to repeat them)
                if self.data.is_empty() {
                    self.insert_tcp_checksum = descriptor.popts_txsm;
                }

                if descriptor.popts_ixsm {
                    todo!("Inserting IP checksum not yet implemented")
                }

                self.read_to_buffer(
                    descriptor.buffer as usize,
                    descriptor.length as usize,
                    nic_ctx,
                )?;

                self.done = descriptor.dcmd_eop;
            }
        }

        Ok(())
    }

    fn finalize(mut self) -> Result<Vec<u8>> {
        assert!(self.done);

        if self.insert_tcp_checksum {
            let tcp_context = self.tcp_context.unwrap();

            let offset = tcp_context.tu_cso as usize;
            let start = tcp_context.tu_css as usize;
            let mut end = tcp_context.tu_cse as usize;
            if end == 0 {
                end = self.data.len();
            }

            let mut checksum = Checksum::new();
            checksum.add_bytes(&self.data[start..end]);
            self.data[offset..offset + 2].copy_from_slice(&checksum.checksum());
        }

        Ok(self.data)
    }
}

impl<C: NicContext> E1000<C> {
    pub fn process_tx_ring(&mut self) {
        if let Some(tx_ring) = &mut self.tx_ring {
            // Software wants to transmit packets
            tx_ring.tail = self.regs.td_t.tail as usize;

            let mut sequence = TransmitDescriptorSequence::default();
            let mut report_status = false;
            while !tx_ring.is_empty() {
                let mut transmit_descriptor =
                    TransmitDescriptor::read_descriptor(&tx_ring, &mut self.nic_ctx).unwrap();

                //eprintln!("TX DESC: {:x?}", transmit_descriptor);

                let result = sequence.add_descriptor(&transmit_descriptor, &mut self.nic_ctx);
                if let Err(err) = result {
                    eprintln!("E1000: Error processing transmit descriptors: {}", err);
                    tx_ring.advance_head();
                    continue;
                }

                // Done processing, report if requested
                if transmit_descriptor.report_status() {
                    report_status = true;
                    *transmit_descriptor.descriptor_done_mut() = true;

                    match transmit_descriptor {
                        TransmitDescriptor::Legacy(desc) => {
                            tx_ring
                                .write_and_advance_head(desc, &mut self.nic_ctx)
                                .unwrap();
                        }
                        TransmitDescriptor::TcpContext(desc) => {
                            tx_ring
                                .write_and_advance_head(desc, &mut self.nic_ctx)
                                .unwrap();
                        }
                        TransmitDescriptor::TcpData(desc) => {
                            tx_ring
                                .write_and_advance_head(desc, &mut self.nic_ctx)
                                .unwrap();
                        }
                    }
                } else {
                    tx_ring.advance_head();
                }

                if sequence.done {
                    let data = sequence.finalize().unwrap();

                    let sent = self.nic_ctx.send(&data).unwrap();
                    assert_eq!(sent, data.len(), "Did not send specified packet length");
                    eprintln!("E1000: Sent {} bytes!", sent);

                    sequence = TransmitDescriptorSequence::default();
                }

                self.regs.td_h.head = tx_ring.head as u16;
            }

            if report_status {
                self.report_txdw_and_txqe();
            } else {
                self.report_txqe()
            }
        }
    }
}
