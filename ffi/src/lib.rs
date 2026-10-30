//! C ABI around the sync core. SwiftUI calls these from a background task.
//! The hub call blocks until one peer connects. The client call blocks until
//! the session finishes. The library does not start a folder watcher.

use photosync_core::{run_client, scan_library, Config, HubServer, Role};
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::ptr;

pub struct PhotosyncConfig {
    cfg: Config,
}

/// # Safety
/// `ptr` is either null or a NUL-terminated UTF-8 string borrowed for this call.
unsafe fn cstr<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr).to_str().ok() }
}

fn write_err(buf: *mut c_char, len: usize, message: &str) {
    if buf.is_null() || len == 0 {
        return;
    }
    let bytes = message.as_bytes();
    let n = bytes.len().min(len.saturating_sub(1));
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, n);
        *buf.add(n) = 0;
    }
}

/// # Safety
/// String pointers are either null or NUL-terminated UTF-8. The returned
/// pointer is owned by the caller and released with `photosync_config_free`.
#[no_mangle]
pub unsafe extern "C" fn photosync_config_new(
    folder: *const c_char,
    peer: *const c_char,
    psk_hex: *const c_char,
    role: c_int,
    listen: *const c_char,
) -> *mut PhotosyncConfig {
    let built = catch_unwind(AssertUnwindSafe(|| {
        let folder = unsafe { cstr(folder)? };
        let psk_hex = unsafe { cstr(psk_hex)? }.to_string();
        let role = match role {
            0 => Role::Client,
            1 => Role::Hub,
            _ => return None,
        };
        let peer = unsafe { cstr(peer) }
            .map(str::to_string)
            .filter(|value| !value.is_empty());
        let listen = unsafe { cstr(listen) }
            .map(str::to_string)
            .filter(|value| !value.is_empty());
        let cfg = Config::new(role, PathBuf::from(folder), listen, peer, psk_hex).ok()?;
        Some(Box::into_raw(Box::new(PhotosyncConfig { cfg })))
    }));
    match built {
        Ok(Some(cfg)) => cfg,
        _ => ptr::null_mut(),
    }
}

/// # Safety
/// `cfg` is null or a pointer returned by `photosync_config_new` and not yet freed.
#[no_mangle]
pub unsafe extern "C" fn photosync_config_free(cfg: *mut PhotosyncConfig) {
    if cfg.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(unsafe { Box::from_raw(cfg) });
    }));
}

/// # Safety
/// `cfg` is a live config. `err_buf` is either null or writable for `err_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn photosync_scan(
    cfg: *mut PhotosyncConfig,
    err_buf: *mut c_char,
    err_len: usize,
) -> c_int {
    if cfg.is_null() {
        write_err(err_buf, err_len, "null config");
        return -1;
    }
    let folder = unsafe { &(*cfg).cfg.folder };
    match catch_unwind(AssertUnwindSafe(|| scan_library(folder))) {
        Ok(Ok(count)) => i32::try_from(count).unwrap_or(i32::MAX),
        Ok(Err(err)) => {
            write_err(err_buf, err_len, &err.to_string());
            -1
        }
        Err(_) => {
            write_err(err_buf, err_len, "panic during scan");
            -1
        }
    }
}

/// # Safety
/// `cfg` is a live config. `err_buf` is either null or writable for `err_len` bytes.
/// This call blocks. A hub waits for one connection. A client dials `peer`.
#[no_mangle]
pub unsafe extern "C" fn photosync_sync_once(
    cfg: *mut PhotosyncConfig,
    err_buf: *mut c_char,
    err_len: usize,
) -> c_int {
    if cfg.is_null() {
        write_err(err_buf, err_len, "null config");
        return -1;
    }
    let cfg_ref = unsafe { &(*cfg).cfg };
    let run = || -> photosync_core::Result<()> {
        match cfg_ref.role {
            Role::Client => {
                run_client(cfg_ref)?;
            }
            Role::Hub => {
                HubServer::bind(cfg_ref)?.serve(true)?;
            }
        }
        Ok(())
    };
    match catch_unwind(AssertUnwindSafe(run)) {
        Ok(Ok(())) => 0,
        Ok(Err(err)) => {
            write_err(err_buf, err_len, &err.to_string());
            -1
        }
        Err(_) => {
            write_err(err_buf, err_len, "panic during sync");
            -1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::fs;

    #[test]
    fn scan_counts_only_media() {
        let dir = std::env::temp_dir().join(format!("photosync-ffi-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.jpg"), b"a").unwrap();
        fs::write(dir.join("b.txt"), b"b").unwrap();

        let folder = CString::new(dir.to_str().unwrap()).unwrap();
        let peer = CString::new("127.0.0.1:9").unwrap();
        let psk = CString::new("ab".repeat(32)).unwrap();
        let cfg = unsafe {
            photosync_config_new(folder.as_ptr(), peer.as_ptr(), psk.as_ptr(), 0, ptr::null())
        };
        assert!(!cfg.is_null());
        let bad = unsafe {
            photosync_config_new(folder.as_ptr(), peer.as_ptr(), psk.as_ptr(), 4, ptr::null())
        };
        assert!(bad.is_null());

        let mut err = [0u8; 256];
        let count = unsafe { photosync_scan(cfg, err.as_mut_ptr() as *mut c_char, err.len()) };
        assert_eq!(count, 1, "err {}", String::from_utf8_lossy(&err));
        unsafe { photosync_config_free(cfg) };
        let _ = fs::remove_dir_all(&dir);
    }
}
