#![feature(c_variadic)]

#[macro_use]
extern crate lazy_static;

use std::{
    env,
    ffi::{CString, CStr},
    fs::File,
    io::Write,
    os::raw::{c_char, c_int, c_void},
    sync::Mutex,
};

use libc::{DIR, dirent, O_CREAT, mode_t, c_void, size_t, ssize_t};

use redhook::{
    hook, real, real2,
};

lazy_static! {
    static ref LOG: Mutex<File> = Mutex::new(File::create("./tadb-log.txt").unwrap());
}

const BASE_DIR_ORIG: &str = "/dev/bus/usb";
const BASE_DIR_REMAPPED: &str = "./fakedev/bus/usb";

// fn init_log_file() {
//     let mut logf = LOG.lock().unwrap();
//     *logf = Some(File::create("./tadb-log.txt").unwrap())
// }

macro_rules! log {
    ($($arg:tt)*) => {
        // init_log_file();
        // _ = writeln!(LOG.lock().unwrap().as_ref().unwrap(), $($arg)*);
        _ = writeln!(&mut *LOG.lock().unwrap(), $($arg)*);
    };
}

fn to_string(s: &CStr) -> String {
    match s.to_str() {
        Ok(s) => s.to_owned(),
        Err(e) => e.to_string(),
    }
}

fn to_cstr(b: &[c_char]) -> &CStr {
    unsafe { CStr::from_ptr(b.as_ptr()) }
}

hook! {
    unsafe fn opendir(name: *const c_char) -> *mut DIR => tadb_opendir {
        let dir_name = to_string(CStr::from_ptr(name));
        let remapped_name = dir_name.replacen(BASE_DIR_ORIG, BASE_DIR_REMAPPED, 1);
        let remapped_name_c = CString::new(remapped_name.as_str()).unwrap();
        log!("[TADB] called opendir with {}, remapping to {}", &dir_name, &remapped_name);
        real!(opendir)(remapped_name_c.as_ptr())
    }
}

hook! {
    unsafe fn readdir(dirp: *mut DIR) -> *mut dirent => tadb_readdir {
        log!("[TADB] called readdir with {:?}", dirp);
        let result = real!(readdir)(dirp);
        if let Some(r) = result.as_ref() {
            log!("[TADB] readdir returned dirent with d_name={}", to_string(to_cstr(&r.d_name)));
        }
        result
    }
}

hook! {
    unsafe fn close(fd: c_int) -> c_int => tadb_close {
        if let Ok(usb_fd_str) = env::var("TERMUX_USB_FD") {
            if let Ok(usb_fd) = usb_fd_str.parse::<c_int>() {
                // usb fd must not be closed
                if usb_fd == fd {
                    return 0;
                }
            }
        }
        real!(close)(fd)
    }
}

hook! {
    unsafe fn read(fd: c_int, buf: *mut c_void, nbytes: size_t)
}

type OpenFn = unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int;

#[no_mangle]
pub unsafe extern "C" fn open(pathname: *const c_char, flags: c_int, mut args: ...) -> c_int {
    // return -1;
    // let real_open = real2!(open);
    // There is some problem with caching the real function value (TODO: fix)
    let real_open: OpenFn = std::mem::transmute(redhook::ld_preload::dlsym_next("open\0"));
    let fn_ptr: *const c_void = std::mem::transmute(&open);
    // eprintln!("DEBUG hook: {:?}", fn_ptr);
    let real_fn_ptr: *const c_void = std::mem::transmute(real_open);
    // eprintln!("DEBUG real: {:?}", real_fn_ptr);

    let name = to_string(CStr::from_ptr(pathname));
    // eprintln!("DEBUG name: {}", name);
    // prevent infinite recursion when logfile is first initialized
    if name != "./tadb-log.txt" {
        log!("[TADB] called open with pathname={} flags={}", name, flags);
    }

    if name.starts_with("/dev/bus/usb") {
        if let Ok(usb_fd_str) = env::var("TERMUX_USB_FD") {
            if let Ok(usb_fd) = usb_fd_str.parse::<c_int>() {
                log!("[TADB] open hook returning fd with value {}", usb_fd);
                return usb_fd;
            }
        }
    }

    let result = if (flags & O_CREAT) == 0 {
        real_open(pathname, flags)
    } else {
        real_open(pathname, flags, args.arg::<mode_t>())
    };

    // prevent infinite recursion when logfile is first initialized
    if name != "./tadb-log.txt" {
        log!("[TADB] open returned fd with value {}", result);
    }
    result
}
