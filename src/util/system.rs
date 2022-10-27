use std::ffi::{CStr, OsString};
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;

use anyhow::{Context, Result};

pub fn get_sudo_uid() -> Result<Option<u32>> {
    match std::env::var("SUDO_UID") {
        Ok(uid) => Ok(Some(uid.parse().context("invalid SUDO_UID")?)),
        Err(_) => Ok(None),
    }
}

pub fn user_name(uid: u32) -> Option<String> {
    // SAFETY: `buf` outlives `pwd.pw_name`
    unsafe {
        let mut buf = make_buffer();
        let pwd = get_passwd(uid, &mut buf)?;
        Some(CStr::from_ptr(pwd.pw_name).to_string_lossy().into_owned())
    }
}

pub fn home_dir(uid: u32) -> Option<PathBuf> {
    // SAFETY: `buf` outlives `pwd.pw_dir`
    unsafe {
        let mut buf = make_buffer();
        let pwd = get_passwd(uid, &mut buf)?;

        let bytes = CStr::from_ptr(pwd.pw_dir).to_bytes().to_vec();
        let pw_dir = OsString::from_vec(bytes);

        Some(PathBuf::from(pw_dir))
    }
}

unsafe fn get_passwd(uid: u32, buf: &mut Buffer) -> Option<libc::passwd> {
    let mut pwd: MaybeUninit<libc::passwd> = MaybeUninit::uninit();
    let mut pwdp = ptr::null_mut();
    match libc::getpwuid_r(
        uid,
        pwd.as_mut_ptr(),
        buf.as_mut_ptr(),
        buf.capacity(),
        &mut pwdp,
    ) {
        0 if !pwdp.is_null() => Some(pwd.assume_init()),
        _ => None,
    }
}

fn make_buffer() -> Buffer {
    // SAFETY: `name` arg is valid
    let init_size = match unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) } {
        -1 => 1024,
        n => n as usize,
    };
    Buffer::with_capacity(init_size)
}

type Buffer = Vec<libc::c_char>;
