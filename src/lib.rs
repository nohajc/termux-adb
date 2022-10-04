#![feature(c_variadic)]

#[macro_use]
extern crate lazy_static;

use std::{
    ffi::{CString, CStr},
    fs::File,
    io::Write,
    os::raw::{c_char, c_int},
    sync::Mutex,
};

use libc::{DIR, dirent, O_CREAT, mode_t, c_void};

use redhook::{
    hook, real, real2,
};

lazy_static! {
    static ref LOG: Mutex<File> = Mutex::new(File::create("./tadb-log.txt").unwrap());
}

const BASE_DIR_ORIG: &str = "/dev/bus/usb";
const BASE_DIR_REMAPPED: &str = "./fakedev/bus/usb";

macro_rules! log {
    ($($arg:tt)*) => {
        _ = writeln!(&mut*LOG.lock().unwrap(), $($arg)*);
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
    unsafe fn __open_2(pathname: *const c_char, flags: c_int) -> c_int => tadb_open {
        let fn_ptr: *const c_void = std::mem::transmute(&__open_2);
        log!("[TADB] called open at {:?}", fn_ptr);
        let real_open = real!(__open_2);
        let real_fn_ptr: *const c_void = std::mem::transmute(real_open);
        log!("[TADB] real open at {:?}", real_fn_ptr);

        let name = to_string(CStr::from_ptr(pathname));

        log!("[TADB] called open with name={} flags={}", name, flags);
        let result = real_open(pathname, flags);

        log!("[TADB] open returned fd with value {}", result);
        result
    }
}

// #[no_mangle]
// pub unsafe extern "C" fn open(pathname: *const c_char, flags: c_int, mut args: ...) -> c_int {
//     let fn_ptr: *const c_void = std::mem::transmute(&open);
//     log!("[TADB] called open at {:?}", fn_ptr);
//     let real_open = real2!(open);
//     let real_fn_ptr: *const c_void = std::mem::transmute(real_open);
//     log!("[TADB] real open at {:?}", real_fn_ptr);

//     let name = to_string(CStr::from_ptr(pathname));

//     let result = if (flags & O_CREAT) == 0 {
//         log!("[TADB] called open with name={} flags={}", name, flags);
//         real_open(pathname, flags)
//     }
//     else {
//         let mode = args.arg::<mode_t>();
//         log!("[TADB] called open with name={} flags={} mode={}", name, flags, mode);
//         real_open(pathname, flags, mode)
//     };

//     log!("[TADB] open returned fd with value {}", result);
//     result
// }
