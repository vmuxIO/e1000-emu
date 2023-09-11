use crate::e1000::E1000;
use crate::NicContext;
use log::error;
use std::slice::from_raw_parts_mut;

// General FFI interface

type SendCallback = unsafe extern "C" fn(buffer: *const u8, len: usize);
type DmaReadCallback = unsafe extern "C" fn(dma_address: usize, buffer: *mut u8, len: usize);
type DmaWriteCallback = unsafe extern "C" fn(dma_address: usize, buffer: *const u8, len: usize);
type IssueInterruptCallback = unsafe extern "C" fn();

#[repr(C)]
struct FfiCallbacks {
    send_cb: SendCallback,
    dma_read_cb: DmaReadCallback,
    dma_write_cb: DmaWriteCallback,
    issue_interrupt_cb: IssueInterruptCallback,
}

impl NicContext for FfiCallbacks {
    fn send(&mut self, buffer: &[u8]) -> anyhow::Result<usize> {
        unsafe {
            (self.send_cb)(buffer.as_ptr(), buffer.len());
        }

        // Assume everything went well...
        Ok(buffer.len())
    }

    fn dma_read(&mut self, address: usize, buffer: &mut [u8]) {
        unsafe {
            (self.dma_read_cb)(address, buffer.as_mut_ptr(), buffer.len());
        }
    }

    fn dma_write(&mut self, address: usize, buffer: &[u8]) {
        unsafe {
            (self.dma_write_cb)(address, buffer.as_ptr(), buffer.len());
        }
    }

    fn trigger_interrupt(&mut self) {
        unsafe { (self.issue_interrupt_cb)() }
    }
}

// E1000 FFI Interface

struct E1000FFI {
    e1000: E1000<FfiCallbacks>,
}

impl E1000FFI {
    #[no_mangle]
    pub extern "C" fn new_e1000(callbacks: FfiCallbacks) -> *mut E1000FFI {
        let e1000_ffi = E1000FFI {
            e1000: E1000::new(callbacks),
        };
        Box::into_raw(Box::new(e1000_ffi))
    }

    #[no_mangle]
    pub extern "C" fn drop_e1000(e1000_ffi: *mut E1000FFI) {
        unsafe {
            // Box will free on drop
            let _ = Box::from_raw(e1000_ffi);
        }
    }

    #[no_mangle]
    pub extern "C" fn e1000_region_access(
        &mut self, bar: u8, offset: usize, data_ptr: *const u8, data_len: usize, write: bool,
    ) -> bool {
        let data = unsafe { from_raw_parts_mut(data_ptr as *mut u8, data_len) };

        let result = match bar {
            0 => self.e1000.region_access_bar0(offset, data, write),
            1 => self.e1000.region_access_bar1(offset, data, write),
            _ => {
                error!("Unknown bar {}", bar);
                return false;
            }
        };

        if let Err(e) = result {
            error!("Error accessing bar {}: {}", bar, e);
            false
        } else {
            true
        }
    }

    #[no_mangle]
    pub extern "C" fn e1000_reset(&mut self) {
        self.e1000.reset_e1000();
    }
}
