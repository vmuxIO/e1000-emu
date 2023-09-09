// Macro to provide easier offset to register match syntax
// and optional debugging including field names, since some registers share the same struct type
macro_rules! match_and_access_registers {
    ($offset:expr, $data:expr, $write:expr,
    { $( $reg_offset:pat $(if $guard:expr)? => $reg:expr $( => $do:block )? ),* $(,)? }
    else $catch:block ) => {
        match $offset {
            $(
                $reg_offset $(if $guard)? => {
                    let result = $reg.access($data, $write);

                    if $write {
                        log::trace!("Writing {:x?} to {} -> {:?}", $data, stringify!($reg), $reg);
                    } else {
                        log::trace!("Reading {}: {:?} -> {:x?}", stringify!($reg), $reg, $data);
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
