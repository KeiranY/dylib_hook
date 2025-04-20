use dylib_hook::create_hook;
use std::ffi::{c_void, c_char};
use std::os::unix::io::RawFd;
use std::slice;
use ctor::ctor;

create_hook!(read(fd: RawFd, buf: *mut c_void, count: usize) -> isize);

fn block_read_hook(_fd: RawFd, buf: *mut c_void, count: usize, _chain: &mut read::Chain) -> isize {
    let message = b"No peeking!\n";
    let len = count.min(message.len());
    unsafe {
        let buf_slice = slice::from_raw_parts_mut(buf as *mut u8, len);
        buf_slice.copy_from_slice(&message[..len]);
    }
    count as isize
}

#[ctor]
fn init() {
    read::add_hook(block_read_hook);
}
