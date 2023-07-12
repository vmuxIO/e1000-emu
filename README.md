# e1000-emu
A minimal E1000 emulator for qemu via libvfio-user

# Emulated Device
The exact model emulated is the Intel 82540EM Gigabit Ethernet Controller.
Only a small subset of the device's functionality will be emulated.

# Building
To build you need all the dependencies of https://github.com/nutanix/libvfio-user, namely the `json-c` and `cmocka` libraries.
You will also need `libclang` required to generate the bindings.
Finally just use `cargo build` or `cargo build --release` to build.

# Usage
To run e1000-emu run either the generated binary inside target/debug or target/release or use `cargo run`/`cargo run --release`

Libvfio-user-rs links libvfio-user statically by default, if you change this inside the Cargo.toml you might have problems finding the shared library file when not using `cargo run`.
To work around that you can use `LD_LIBRARY_PATH` to supply your own `libvfio-user.so`.

A socket will be created at /tmp/e1000-emu.sock, which qemu can connect to using this parameter:
`-device vfio-user-pci,socket="/tmp/e1000-emu.sock"`
