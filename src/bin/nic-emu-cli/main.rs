use std::path::PathBuf;
use std::time::Instant;

use clap::ArgAction;
use clap::Parser;
use ipnet::IpNet;
use log::{debug, error, info, trace, warn, LevelFilter};
use macaddr::MacAddr6;
use polling::{Event, Events, PollMode, Poller};

use crate::ctx::LibvfioUserContext;
use crate::e1000::E1000Device;
use crate::net::Interface;
use nic_emu::e1000::E1000;

mod ctx;
mod e1000;
pub mod net;

#[derive(Parser, Debug)]
#[command(long_about = "")] // long_about required for long help, otherwise help is always short
struct Args {
    /// Libvfio-user socket
    #[arg(short, long, default_value = "/tmp/nic-emu.sock")]
    socket: PathBuf,

    /// Name of tap interface, if not already existing it will be created,
    /// %d will be replaced by a number to create new tap interface
    // Start default name with "tap" to avoid systems from managing it, if configured this way
    #[arg(short, long, default_value = "tap-nic-emu%d")]
    tap: String,

    /// Automatically run commands to add IP range to tap interface and set link to be up,
    /// for example --net 10.1.0.1/24
    #[arg(short, long)]
    net: Option<IpNet>,

    /// Ethernet address of the emulated nic inside guest
    // Default mac x2-... is in locally administered range and
    // should hopefully not conflict with anything
    #[arg(short, long, default_value_t = MacAddr6::new(0x02, 0x34, 0x56, 0x78, 0x9A, 0xBC))]
    mac: MacAddr6,

    /// Increase verbosity, 1 time => Debug logs, multiple times => Trace logs
    #[arg(short, long, action = ArgAction::Count)]
    verbose: u8,
}

fn main() {
    let args = Args::parse();

    pretty_env_logger::formatted_builder()
        .filter_level(match args.verbose {
            0 => LevelFilter::Info,
            1 => LevelFilter::Debug,
            _ => LevelFilter::Trace,
        })
        .parse_default_env() // Overwrite from RUST_LOG env var
        .init();

    let mut e1000_device = E1000Device::build(args.socket, args.mac);

    let interface = Interface::initialize(true, &args.tap, args.net);
    e1000_device.e1000.nic_ctx.interface = Some(interface);

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
                e1000_device.e1000.nic_ctx.interface.as_ref().unwrap(),
                Event::all(EVENT_KEY_RECEIVE),
                PollMode::Edge,
            )
            .unwrap();
    }

    // Buffer for received packets interface
    let mut interface_buffer = [0u8; 4096]; // Big enough

    let start = Instant::now();
    'polling: loop {
        events.clear();
        poller.wait(&mut events, None).unwrap();

        for event in events.iter() {
            match event.key {
                EVENT_KEY_RUN => {
                    trace!("Poller: Libvfio-user event");
                    if let Err(e) = ctx.run() {
                        error!("Error processing libvfio-user command: {}", e);
                        break 'polling;
                    }

                    // Try to catch up on deferred packets (arrived during throttling)
                    receive_packets(&mut e1000_device.e1000, &mut interface_buffer)
                }
                EVENT_KEY_RECEIVE => {
                    trace!("Poller: Interface event");
                    receive_packets(&mut e1000_device.e1000, &mut interface_buffer)
                }
                x => {
                    unreachable!("Unknown event key {}", x);
                }
            }
        }
    }
    // Just let poller be dropped, delete previous fds if we want to reuse it in the future

    let elapsed = start.elapsed().as_secs_f32();
    info!("Statistics:");
    info!(
        "{} total interrupts sent, ~{:.2} per second",
        e1000_device.e1000.nic_ctx.interrupt_count,
        e1000_device.e1000.nic_ctx.interrupt_count as f32 / elapsed
    );
    info!(
        "{} total dma reads, ~{:.2} per second, {}B total",
        e1000_device.e1000.nic_ctx.dma_read_count,
        e1000_device.e1000.nic_ctx.dma_read_count as f32 / elapsed,
        e1000_device.e1000.nic_ctx.dma_read_bytes
    );
    info!(
        "{} total dma writes, ~{:.2} per second, {}B total",
        e1000_device.e1000.nic_ctx.dma_write_count,
        e1000_device.e1000.nic_ctx.dma_write_count as f32 / elapsed,
        e1000_device.e1000.nic_ctx.dma_write_bytes
    );
    info!("Exiting after {:.3}s run time.", elapsed);
}

fn receive_packets(e1000: &mut E1000<LibvfioUserContext>, shared_buffer: &mut [u8; 4096]) {
    loop {
        if e1000.receive_state.should_defer() {
            break;
        }

        match e1000
            .nic_ctx
            .interface
            .as_ref()
            .unwrap()
            .receive(shared_buffer)
            .unwrap()
        {
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
