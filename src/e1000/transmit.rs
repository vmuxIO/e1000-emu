use crate::e1000::descriptors::*;
use crate::e1000::E1000;
use crate::NicContext;
use internet_checksum::Checksum;

impl<C: NicContext> E1000<C> {
    pub fn process_tx_ring(&mut self) {
        if let Some(tx_ring) = &mut self.tx_ring {
            // Software wants to transmit packets
            tx_ring.tail = self.regs.td_t.tail as usize;

            let mut descriptor_buffer = [0u8; DESCRIPTOR_BUFFER_SIZE];
            let mut tcp_context: Option<TransmitDescriptorTcpContext> = None;
            while !tx_ring.is_empty() {
                let mut transmit_descriptor =
                    TransmitDescriptor::read_descriptor(tx_ring, &mut self.nic_ctx).unwrap();

                if let Some(buffer) = transmit_descriptor.buffer() {
                    // Null descriptors should only occur in *receive* descriptor padding
                    assert_ne!(buffer, 0, "Transmit descriptor buffer is null");
                }
                //eprintln!("TX DESC: {:x?}", transmit_descriptor);

                match &transmit_descriptor {
                    TransmitDescriptor::Legacy(descriptor) => {
                        if !descriptor.cmd_eop {
                            todo!("Multiple descriptors per packet not yet implemented")
                        }
                        if descriptor.cmd_ic {
                            todo!("Inserting checksum not implemented yet");
                        }

                        // Send packet/frame
                        self.nic_ctx
                            .dma_read(descriptor.buffer as usize, descriptor_buffer.as_mut_slice());

                        let length = descriptor.length as usize;
                        let buffer = &descriptor_buffer[..length];
                        let sent = self.nic_ctx.send(buffer).unwrap();
                        assert_eq!(length, sent, "Did not send specified packet length");
                        eprintln!("E1000: Sent {} bytes!", sent);
                    }
                    TransmitDescriptor::TcpContext(descriptor) => {
                        if descriptor.tucmd_tse {
                            todo!("TCP Segmentation not yet implemented")
                        }

                        tcp_context = Some(descriptor.clone());
                    }
                    TransmitDescriptor::TcpData(descriptor) => {
                        let tcp_context = match tcp_context.as_ref() {
                            Some(tcp_context) => tcp_context,
                            None => {
                                eprintln!(
                                    "E1000: Error: Tcp data descriptor without context descriptor!"
                                );
                                continue;
                            }
                        };

                        if !descriptor.dcmd_eop {
                            todo!("Multiple descriptors per packet not yet implemented")
                        }
                        if descriptor.popts_ixsm {
                            todo!("Inserting IP checksum not yet implemented")
                        }

                        // Send packet/frame
                        self.nic_ctx
                            .dma_read(descriptor.buffer as usize, descriptor_buffer.as_mut_slice());

                        if descriptor.popts_txsm {
                            let offset = tcp_context.tu_cso as usize;
                            let start = tcp_context.tu_css as usize;
                            let mut end = tcp_context.tu_cse as usize;
                            if end == 0 {
                                end = descriptor.length as usize;
                            }

                            let mut checksum = Checksum::new();
                            checksum.add_bytes(&descriptor_buffer[start..end]);
                            descriptor_buffer[offset..offset + 2]
                                .copy_from_slice(&checksum.checksum());
                        }

                        let length = descriptor.length as usize;
                        let buffer = &descriptor_buffer[..length];
                        let sent = self.nic_ctx.send(buffer).unwrap();
                        assert_eq!(length, sent, "Did not send specified packet length");
                        eprintln!("E1000: Sent {} bytes!", sent);
                    }
                }

                // Done processing, report if requested
                if transmit_descriptor.report_status() {
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

                self.regs.td_h.head = tx_ring.head as u16;
            }
        }
    }
}
