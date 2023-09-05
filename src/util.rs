use etherparse::PacketBuilder;

// Macro to provide easier offset to register match syntax
// and optional debugging including field names, since some registers share the same struct type
macro_rules! match_and_access_registers {
    ($offset:expr, $data:expr, $write:expr, $debug:expr,
    { $( $reg_offset:pat $(if $guard:expr)? => $reg:expr $( => $do:block )? ),* $(,)? }
    else $catch:block ) => {
        match $offset {
            $(
                $reg_offset $(if $guard)? => {
                    if $debug {
                        print!("Register Debug: ");
                        if $write {
                            print!("Writing {:x?} to {}: {:?} -> ", $data, stringify!($reg), $reg);
                        } else {
                            print!("Reading {}: {:?} -> ", stringify!($reg), $reg);
                        }
                    }

                    let result = $reg.access($data, $write);
                    if $debug {
                        if $write {
                            println!("{:?}", $reg);
                        } else {
                            println!("{:x?}", $data);
                        }
                    }

                    $( $do )?
                    result
                },
            )*
            _ => $catch
        }
    };
}

pub(crate) use match_and_access_registers;

pub fn _dummy_frame() -> Vec<u8> {
    let builder = PacketBuilder::ethernet2([1, 2, 3, 4, 5, 6], [1, 2, 3, 4, 5, 6])
        .ipv4([192, 168, 0, 1], [192, 168, 0, 2], 64)
        .icmpv4_echo_request(1234, 5678);

    let payload = b"Hello world!";
    let size = builder.size(payload.len());

    let mut frame = Vec::with_capacity(size);
    builder.write(&mut frame, payload).unwrap();

    frame
}

pub fn wrapping_add_to_u16_be_bytes(data: &mut [u8], by: u16) {
    let mut n = [0u8; 2];
    n.copy_from_slice(data);
    data.copy_from_slice(&u16::from_be_bytes(n).wrapping_add(by).to_be_bytes());
}

pub fn wrapping_add_to_u32_be_bytes(data: &mut [u8], by: u32) {
    let mut n = [0u8; 4];
    n.copy_from_slice(data);
    data.copy_from_slice(&u32::from_be_bytes(n).wrapping_add(by).to_be_bytes());
}
