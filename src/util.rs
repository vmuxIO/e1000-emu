// Macro to provide easier offset to register match syntax
// and optional debugging including field names, since some registers share the same struct type
macro_rules! match_and_access_registers {
    ($offset:expr, $data:expr, $write:expr, $debug:expr,
    { $( $reg_offset:expr => $reg:expr $( => $do:block )? ),* $(,)? }
    else $catch:block ) => {
        match $offset {
            $(
                $reg_offset => {
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
