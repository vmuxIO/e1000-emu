use log::{debug, info, warn, LevelFilter};
use polling::{Event, Events, PollMode, Poller};

use crate::ctx::LibvfioUserContext;
use crate::e1000::E1000Device;
use nic_emu::e1000::E1000;

mod ctx;
mod e1000;
pub mod net;

fn main() {
    pretty_env_logger::formatted_builder()
        .filter_level(LevelFilter::Info)
        .parse_default_env() // Overwrite from RUST_LOG env var
        .init();

    let mut e1000_device = E1000Device::build();

    // Use same poller and event list for both attach and run
    let poller = Poller::new().unwrap();
    let mut events = Events::new();

    const EVENT_KEY_ATTACH: usize = 0;
    const EVENT_KEY_RUN: usize = 1;
    const EVENT_KEY_RECEIVE: usize = 2;

    let ctx = e1000_device.e1000.nic_ctx.device_context.clone();

    // 1. Wait for client to attach

    info!("Attaching...");
    unsafe {
        poller.add(&ctx, Event::all(EVENT_KEY_ATTACH)).unwrap();
    }

    loop {
        events.clear();
        poller.wait(&mut events, None).unwrap();

        match ctx.attach().unwrap() {
            Some(_) => {
                break;
            }
            None => {
                // Renew fd, not using Edge mode like we do below for run() since
                // attach probably succeeds fine the first time
                poller.modify(&ctx, Event::all(EVENT_KEY_ATTACH)).unwrap();
            }
        }
    }
    // Fd is auto-removed from poller since it polled in the default Oneshot mode

    // 2. Process client requests

    info!("Running...");
    // Auto-removed and now adding ctx again since file descriptor may change after attach
    // Poll in Edge mode to avoid having to set interest again and again
    unsafe {
        poller
            .add_with_mode(&ctx, Event::all(EVENT_KEY_RUN), PollMode::Edge)
            .unwrap();
        poller
            .add_with_mode(
                &e1000_device.e1000.nic_ctx.interface,
                Event::all(EVENT_KEY_RECEIVE),
                PollMode::Edge,
            )
            .unwrap();
    }

    // Buffer for received packets interface
    let mut interface_buffer = [0u8; 4096]; // Big enough

    loop {
        events.clear();
        poller.wait(&mut events, None).unwrap();

        for event in events.iter() {
            match event.key {
                EVENT_KEY_RUN => {
                    ctx.run().unwrap();

                    // Try to catch up on deferred packets (arrived during throttling)
                    receive_packets(&mut e1000_device.e1000, &mut interface_buffer)
                }
                EVENT_KEY_RECEIVE => {
                    receive_packets(&mut e1000_device.e1000, &mut interface_buffer)
                }
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

fn receive_packets(e1000: &mut E1000<LibvfioUserContext>, shared_buffer: &mut [u8; 4096]) {
    loop {
        if e1000.receive_state.should_defer() {
            break;
        }

        match e1000.nic_ctx.interface.receive(shared_buffer).unwrap() {
            Some(len) => {
                if !e1000.receive_state.is_ready() {
                    // Drop packet
                    debug!(
                        "Dropping {} incoming bytes, nic not ready to receive yet",
                        len
                    );
                    continue;
                }
                match e1000.receive(&shared_buffer[..len]) {
                    Ok(_) => {}
                    Err(err) => {
                        warn!("Error handling receive event, skipping ({})", err);
                    }
                }
            }
            None => {
                break;
            }
        }
    }
}
