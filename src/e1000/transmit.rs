use anyhow::{ensure, Result};
use internet_checksum::{update, Checksum};
use log::{debug, error, trace};

use crate::e1000::descriptors::*;
use crate::e1000::E1000;
use crate::util::{wrapping_add_to_u16_be_bytes, wrapping_add_to_u32_be_bytes};
use crate::NicContext;

// Field offsets in headers
const IPV4_PAYLOAD_LENGTH_OFFSET: usize = 2;
const IPV4_IDENTIFICATION_OFFSET: usize = 4;
const IPV6_PAYLOAD_LENGTH_OFFSET: usize = 4;
const UDP_LENGTH_OFFSET: usize = 4;
const TCP_SEQUENCE_NUMBER_OFFSET: usize = 4;
const TCP_FLAGS_OFFSET: usize = 13; // Byte that contains FIN and PSH flag
const TCP_FLAGS_MASK: u8 = 9; // FIN + PSH flag

#[derive(Debug, Default)]
struct TransmitDescriptorSequence {
    data: Vec<u8>,
    done: bool,

    // Options only for tcp transmit descriptors
    tcp_context: Option<TransmitDescriptorTcpContext>,
    insert_ip_checksum: bool,
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

        nic_ctx.dma_prepare(address, 4096); // Map whole page
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
                    // Not sure under what circumstances this is being used.
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
                    self.insert_ip_checksum = descriptor.popts_ixsm;
                    self.insert_tcp_checksum = descriptor.popts_txsm;
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

    fn finalize(self) -> Result<Vec<Vec<u8>>> {
        assert!(self.done);

        let mut packets: Vec<Vec<u8>> = Vec::new();

        if let Some(tcp_context) = self.tcp_context {
            // TCP Segmentation
            if tcp_context.tucmd_tse {
                let header_length = tcp_context.hdrlen as usize;
                let payload_length = tcp_context.paylen as usize;
                let segment_size = tcp_context.mss as usize;

                let prototype_header = &self.data[..header_length];
                let segment_data = &self.data[header_length..];

                ensure!(
                    segment_data.len() == payload_length,
                    "Payload length doesn't match, expected {}B, got {}B",
                    payload_length,
                    segment_data.len()
                );

                let segment_count = segment_data.chunks(segment_size).len();
                for (i, segment) in segment_data.chunks(segment_size).enumerate() {
                    let mut packet = Vec::new();
                    packet.extend_from_slice(prototype_header);
                    packet.extend_from_slice(segment);

                    update_prototype_headers(
                        packet.as_mut_slice(),
                        &tcp_context,
                        i,
                        i == segment_count - 1,
                        self.insert_tcp_checksum,
                    );

                    // Omit Frame check sequence (FCS) for now
                    packets.push(packet);
                }
            } else {
                packets.push(self.data);
            }

            // Fill checksums
            for packet in packets.iter_mut().map(|v| v.as_mut_slice()) {
                if self.insert_ip_checksum {
                    let offset = tcp_context.ip_cso as usize;
                    let start = tcp_context.ip_css as usize;
                    let end = tcp_context.ip_cse as usize;
                    write_internet_checksum(packet, offset, start, end);
                }

                if self.insert_tcp_checksum {
                    let offset = tcp_context.tu_cso as usize;
                    let start = tcp_context.tu_css as usize;
                    let end = tcp_context.tu_cse as usize;
                    write_internet_checksum(packet, offset, start, end);
                }
            }
        } else {
            packets.push(self.data);
        }

        Ok(packets)
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

                trace!("Processing TX descriptor: {:?}", transmit_descriptor);

                let result = sequence.add_descriptor(&transmit_descriptor, &mut self.nic_ctx);
                if let Err(err) = result {
                    error!("Error processing transmit descriptors: {}", err);
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
                    let packets = sequence.finalize().unwrap();

                    for data in packets {
                        let sent = self.nic_ctx.send(&data).unwrap();
                        assert_eq!(sent, data.len(), "Did not send specified packet length");
                        debug!("Sent {} bytes!", sent);
                    }

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

fn write_internet_checksum(data: &mut [u8], offset: usize, start: usize, inclusive_end: usize) {
    // Note this range may include the checksum itself,
    // which does *not* have to be zeroed, because it is used to include partial checksums
    let mut checksum = Checksum::new();
    if inclusive_end != 0 {
        checksum.add_bytes(&data[start..inclusive_end + 1]);
    } else {
        checksum.add_bytes(&data[start..]);
    }
    data[offset..offset + 2].copy_from_slice(&checksum.checksum());
}

// Update/Fill the prototype headers prepended to the data when using TSE,
// returns ip payload length as that needs to be included in checksum calculation
fn update_prototype_headers(
    data: &mut [u8], tcp_context: &TransmitDescriptorTcpContext, segment_index: usize,
    last_frame: bool, tcp_checksum_offloaded: bool,
) {
    let header_length = tcp_context.hdrlen as usize;
    let segment_size = tcp_context.mss as usize;

    // Checksum starts double down as offset
    let ip_offset = tcp_context.ip_css as usize;
    let tcp_udp_offset = tcp_context.tu_css as usize;

    // 1. IP total length always = MSS + HDRLEN - IPCSS
    let ip_total_length = (segment_size + header_length - ip_offset) as u16;
    let ip_total_length = ip_total_length.to_be_bytes();

    if tcp_context.tucmd_ip {
        // IPv4
        let offset = ip_offset + IPV4_PAYLOAD_LENGTH_OFFSET;
        data[offset..offset + 2].copy_from_slice(&ip_total_length);

        // 2. IP identification increments by 1
        let offset = ip_offset + IPV4_IDENTIFICATION_OFFSET;
        wrapping_add_to_u16_be_bytes(&mut data[offset..offset + 2], segment_index as u16);
    } else {
        // IPv6
        let offset = ip_offset + IPV6_PAYLOAD_LENGTH_OFFSET;
        data[offset..offset + 2].copy_from_slice(&ip_total_length);
    }

    let length_after_ip = (data.len() - tcp_udp_offset) as u16;
    let length_after_ip = length_after_ip.to_be_bytes();

    // Length is included in TCP checksum, so update partial TCP checksum if offloaded
    if tcp_checksum_offloaded {
        let offset = tcp_context.tu_cso as usize;
        let previous_checksum = [data[offset], data[offset + 1]];
        data[offset..offset + 2].copy_from_slice(&update(
            previous_checksum,
            &length_after_ip,
            &[0, 0], // Reversed for some reason
        ));
    }

    if tcp_context.tucmd_tcp {
        // TCP

        // 3. Sequence number get incremented by segment size
        let offset = tcp_udp_offset + TCP_SEQUENCE_NUMBER_OFFSET;
        let previous_total_size = segment_size * segment_index;
        wrapping_add_to_u32_be_bytes(&mut data[offset..offset + 4], previous_total_size as u32);

        // 4. Clear FIN and PSH flags if not last frame
        if !last_frame {
            let offset = tcp_udp_offset + TCP_FLAGS_OFFSET;
            data[offset] = data[offset] & !TCP_FLAGS_MASK;
        }
    } else {
        // UDP
        // 5. Set length
        let offset = tcp_udp_offset + UDP_LENGTH_OFFSET;
        data[offset..offset + 2].copy_from_slice(&length_after_ip);
    }
}
