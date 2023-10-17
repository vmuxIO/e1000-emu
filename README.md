# nic-emu
An E1000 emulation library and standalone executable for qemu via [libvfio-user](https://github.com/nutanix/libvfio-user)

## E1000
The exact model emulated is the Intel 82540EM Gigabit Ethernet Controller.

Only a subset of the device's functionality will be emulated.
Some notable features currently missing are:
- VLAN support
- PCI-X mode (not to be confused with PCI-E)
- Filter, Wakeup, Statistics, Diagnostic registers
- PHY & EEPROM/FLASH beyond their required registers for startup and error-free operation

It has been tested with both the linux e1000 kernel driver
and the simple [vfio-e1000](https://github.com/mmisono/vfio-e1000) driver for testing.
Other drivers may need functionality not yet implemented.

## Building
### Library
If you only want to build the library you can omit the binary's dependencies by excluding the default-features.

`cargo build --lib --no-default-features`

#### Staticlib & Bindings
If you want to integrate nic-emu into a non Rust project,
building will also produce a staticlib `libnic_emu.a`.
C bindings can automatically be generated using [cbindgen](https://github.com/mozilla/cbindgen)
by including the generate-bindings feature.

`cargo build --lib --no-default-features --features generate-bindings`

### Binary
`cargo build` will produce both the library and `nic-emu-cli` to use with qemu

#### Dependencies
To build the binary you will need
- All the dependencies of [libvfio-user](https://github.com/nutanix/libvfio-user),
namely the libraries of `json-c` and `cmocka`.
- `libclang` for [libvfio-user-rs](https://github.com/vmuxIO/libvfio-user-rs).

### Release build
If you want to actually use or benchmark the emulated device **please build nic-emu in release mode!**
Crude benchmarks reveal the release build can sustain much higher bandwidths. **(~8-10x higher!)**

Building in release mode is as simple as adding `--release` to the appropriate build command,
e.g. `cargo build --release`

## CLI usage
To run nic-emu-cli, run either the generated `nic-emu-cli` binary inside target/debug or target/release
or use `cargo run`/`cargo run --release`.

Note that since nic-emu-cli creates/opens a tap interface to send and receive traffic,
it will need the appropriate permissions to do so.

nic-emu-cli supports several command line arguments, use the `--help` argument to display them. `cargo run -- --help`

### QEMU
This project has been developed using
[a special fork of QEMU](https://github.com/oracle/qemu/tree/vfio-user-7.1.5)
to use it as a libvfio-user client.

A socket is used to communicate with QEMU, by default it will be created at `/tmp/nic-emu.sock`.
QEMU can then connect to it using this parameter: `-device vfio-user-pci,socket="/tmp/nic-emu.sock"`.

If you use the intel-iommu device in QEMU make sure to add `caching-mode=on` for it to work!

Newer libvfio-user and QEMU versions may change the mechanism of the underlying communication
and thus may require updates to [libvfio-user-rs](https://github.com/vmuxIO/libvfio-user-rs) and nic-emu.

## References
- https://www.intel.com/content/dam/doc/manual/pci-pci-x-family-gbe-controllers-software-dev-manual.pdf
- https://github.com/qemu/qemu/blob/master/hw/net/e1000.c - reference implementation
- https://github.com/mmisono/vfio-e1000 - simple testing driver
- https://github.com/torvalds/linux/tree/master/drivers/net/ethernet/intel/e1000 - target driver
- https://wiki.osdev.org/Intel_8254x - explains IO register and EEPROM access 
