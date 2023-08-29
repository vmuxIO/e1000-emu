use libvfio_user::*;
use polling::{Event, PollMode, Poller};

use e1000_emu::E1000;

fn main() {
    let socket = "/tmp/e1000-emu.sock";

    let config = DeviceConfigurator::default()
        .socket_path(socket.parse().unwrap())
        .overwrite_socket(true)
        .pci_type(PciType::Pci)
        .pci_config(PciConfig {
            vendor_id: 0x8086, // Intel 82540EM Gigabit Ethernet Controller
            device_id: 0x100e,
            subsystem_vendor_id: 0x0000, // Empty subsystem ids
            subsystem_id: 0x0000,
            class_code_base: 0x02, // Ethernet Controller class code
            class_code_subclass: 0x00,
            class_code_programming_interface: 0x00,
            revision_id: 3, // Revision 3, same as in QEMU
        })
        .add_device_region(DeviceRegion {
            region_type: DeviceRegionKind::Bar0,
            size: 0x20000, // 128 KiB
            file_descriptor: -1,
            offset: 0,
            read: true,
            write: true,
            memory: true,
        })
        .add_device_region(DeviceRegion {
            region_type: DeviceRegionKind::Bar1,
            size: 0x40, // 64 B
            file_descriptor: -1,
            offset: 0,
            read: true,
            write: true,
            memory: false,
        })
        .using_interrupt_requests(InterruptRequestKind::IntX, 1)
        .using_interrupt_requests(InterruptRequestKind::Msi, 1)
        .setup_dma(true)
        .non_blocking(true)
        .build()
        .unwrap();

    let mut e1000 = config.produce::<E1000>().unwrap();
    println!("VFU context created successfully");

    // Setup initial eeprom, should not be changed afterwards

    // Set to test mac
    // x2-... is in locally administered range and should hopefully not conflict with anything
    e1000
        .eeprom
        .initial_eeprom
        .set_ethernet_address([0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);
    e1000.eeprom.pack_initial_eeprom();

    // Use same poller and event list for both attach and run
    let poller = Poller::new().unwrap();
    let mut events = vec![];

    const EVENT_KEY_ATTACH: usize = 0;
    const EVENT_KEY_RUN: usize = 1;
    const EVENT_KEY_RECEIVE: usize = 2;

    // 1. Wait for client to attach

    println!("Attaching...");
    poller
        .add(&e1000.ctx, Event::all(EVENT_KEY_ATTACH))
        .unwrap();

    loop {
        events.clear();
        poller.wait(&mut events, None).unwrap();

        match e1000.ctx.attach().unwrap() {
            Some(_) => {
                break;
            }
            None => {
                // Renew fd, not using Edge mode like we do below for run() since
                // attach probably succeeds fine the first time
                poller
                    .modify(&e1000.ctx, Event::all(EVENT_KEY_ATTACH))
                    .unwrap();
            }
        }
    }
    // Fd is auto-removed from poller since it polled in the default Oneshot mode

    // 2. Process client requests

    println!("Running...");
    // Removed and now adding it again since file descriptor may change after attach
    // Poll in Edge mode to avoid having to set interest again and again
    poller
        .add_with_mode(&e1000.ctx, Event::all(EVENT_KEY_RUN), PollMode::Edge)
        .unwrap();
    poller
        .add_with_mode(
            &e1000.interface,
            Event::all(EVENT_KEY_RECEIVE),
            PollMode::Edge,
        )
        .unwrap();

    loop {
        events.clear();
        poller.wait(&mut events, None).unwrap();

        for event in &events {
            match event.key {
                EVENT_KEY_RUN => {
                    e1000.ctx().run().unwrap();
                }
                EVENT_KEY_RECEIVE => match e1000.receive() {
                    Ok(_) => {}
                    Err(err) => {
                        println!("Error handling receive event, skipping ({})", err);
                    }
                },
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
