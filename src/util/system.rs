use std::ffi::{CStr, CString, OsString};
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::prelude::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;

use anyhow::{Context, Result};

#[allow(unused)]
pub fn get_sudo_uid() -> Result<Option<u32>> {
    match std::env::var("SUDO_UID") {
        Ok(uid) => Ok(Some(uid.parse().context("invalid SUDO_UID")?)),
        Err(_) => Ok(None),
    }
}

#[allow(unused)]
pub fn user_id() -> u32 {
    // SAFETY: no errors are defined
    unsafe { libc::getuid() }
}

#[allow(unused)]
pub fn user_name(uid: u32) -> Option<String> {
    // SAFETY: `buf` outlives `pwd.pw_name`
    unsafe {
        let mut buf = make_buffer();
        let pwd = get_passwd(uid, &mut buf)?;
        Some(CStr::from_ptr(pwd.pw_name).to_string_lossy().into_owned())
    }
}

#[allow(unused)]
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

pub fn make_shell_path(path: &str) -> PathBuf {
    // Replace `~` with a path to the home directory
    if let Some(path_after_tilde) = path.strip_prefix('~') {
        if path_after_tilde.is_empty() || path_after_tilde.starts_with('/') {
            if let Some(home) = home::home_dir() {
                return home.join(path_after_tilde.trim_start_matches('/'));
            }
        }
    }

    PathBuf::from(path)
}

#[derive(Debug, Clone, Copy)]
pub struct FsStats {
    pub free_space: u64,
    pub available_space: u64,
    pub total_space: u64,
    pub allocation_granularity: u64,
}

pub fn statvfs<P: AsRef<Path>>(path: P) -> Result<FsStats> {
    let path =
        CString::new(path.as_ref().as_os_str().as_bytes()).context("invalid path for statvfs")?;

    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };

    let res = unsafe { libc::statvfs(path.as_ptr() as *const _, &mut stat) };
    anyhow::ensure!(res == 0, std::io::Error::last_os_error());

    Ok(FsStats {
        free_space: stat.f_frsize * stat.f_bfree,
        available_space: stat.f_frsize * stat.f_bavail,
        total_space: stat.f_frsize * stat.f_blocks,
        allocation_granularity: stat.f_frsize,
    })
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
