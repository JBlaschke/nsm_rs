//! Implementation for PS Vita
use crate::{util_libc::last_os_error, Error};
use core::mem::MaybeUninit;

pub fn getrandom_inner(dest: &mut [MaybeUninit<u8>]) -> Result<(), Error> {
    for chunk in dest.chunks_mut(256) {
        let ret = unsafe { libc::getentropy(chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if ret == -1 {
            return Err(last_os_error());
        }
    }
    Ok(())
}
