#![feature(c_variadic)]

#[macro_use]
extern crate lazy_static;

use std::{
    env,
    ffi::{CStr, OsStr},
    fs::File,
    io::Write,
    mem,
    os::raw::{c_char, c_int},
    sync::Mutex, collections::HashMap, path::PathBuf, ptr::null_mut,
};

use std::os::unix::ffi::OsStrExt;

use libc::{DIR, dirent, O_CREAT, mode_t, DT_CHR, DT_DIR, off_t, c_ushort, c_uchar};

use rand::Rng;

use redhook::{
    hook, real,
};

use nix::unistd::{lseek, Whence};

lazy_static! {
    static ref LOG: Mutex<File> = Mutex::new(File::create("./tadb-log.txt").unwrap());
}

const BASE_DIR_ORIG: &str = "/dev/bus/usb";
// const BASE_DIR_REMAPPED: &str = "./fakedev/bus/usb";

macro_rules! log {
    ($($arg:tt)*) => {
        _ = writeln!(&mut *LOG.lock().unwrap(), $($arg)*);
    };
}

fn to_string(s: &CStr) -> String {
    // OsStr::from_bytes(s.to_bytes()).to_owned()
    match s.to_str() {
        Ok(s) => s.to_owned(),
        Err(e) => e.to_string(),
    }
}

fn to_cstr(b: &[c_char]) -> &CStr {
    unsafe { CStr::from_ptr(b.as_ptr()) }
}

// our directory structure will always be flat
// so we can have just one dirent per DirStream
#[derive(Clone)]
struct DirStream {
    pos: i32,
    entry: dirent,
}

trait NameSetter {
    fn set_name(&mut self, name: &OsStr);
}

impl NameSetter for dirent {
    fn set_name(&mut self, name: &OsStr) {
        for (i, j) in self.d_name.iter_mut().zip(
            name.as_bytes().iter().chain([0].iter())
        ) {
            *i = *j as c_char;
        }
    }
}

fn dirent_new(off: off_t, typ: c_uchar, name: &OsStr) -> dirent {
    let mut rng = rand::thread_rng();
    let mut entry = dirent {
        d_ino: rng.gen(),
        d_off: off,
        d_reclen: mem::size_of::<dirent>() as c_ushort,
        d_type: typ,
        d_name: [0; 256],
    };
    entry.set_name(name);

    entry
}

lazy_static! {
    static ref DIR_MAP: HashMap<PathBuf, DirStream> = {
        let mut dir_map = HashMap::new();
        if let Ok(usb_dev_path) = env::var("TERMUX_USB_DEV").map(|str| PathBuf::from(str)) {
            if let Some(usb_dev_name) = usb_dev_path.file_name() {
                let mut last_entry = dirent_new(
                    0, DT_CHR, usb_dev_name
                );
                let mut current_dir = usb_dev_path;

                while current_dir.pop() {
                    dir_map.insert(current_dir.clone(), DirStream{
                        pos: 0,
                        entry: last_entry.clone(),
                    });
                    last_entry = dirent_new(
                        0, DT_DIR, current_dir.file_name().unwrap()
                    );

                    if current_dir.as_os_str() == BASE_DIR_ORIG {
                        break;
                    }
                }
            }
        }
        dir_map
    };
}

enum HookedDir {
    Native(*mut DIR),
    Virtual(DirStream)
}

impl From<HookedDir> for *mut DIR {
    fn from(hd: HookedDir) -> Self {
        Box::into_raw(Box::new(hd)) as Self
    }
}

hook! {
    unsafe fn opendir(name: *const c_char) -> *mut DIR => tadb_opendir {
        let dir_name = to_string(CStr::from_ptr(name));

        if dir_name.starts_with(BASE_DIR_ORIG) {
            if let Some(dirstream) = DIR_MAP.get(&PathBuf::from(&dir_name)) {
                log!("[TADB] called opendir with {}, remapping to virtual DirStream", &dir_name);
                return HookedDir::Virtual(dirstream.to_owned()).into();
            }
        }

        log!("[TADB] called opendir with {}", &dir_name);
        let dir = real!(opendir)(name);
        HookedDir::Native(dir).into()
    }
}

hook! {
    unsafe fn closedir(dirp: *mut DIR) -> c_int => tadb_closedir {
        if dirp.is_null() {
            return real!(closedir)(dirp);
        }

        let hooked_dir = Box::from_raw(dirp as *mut HookedDir);
        match hooked_dir.as_ref() {
            &HookedDir::Native(dirp) => real!(closedir)(dirp),
            // nothing to do, hooked_dir along with DirStream
            // will be dropped at the end of this function
            &HookedDir::Virtual(_) => 0
        }
    }
}

hook! {
    unsafe fn readdir(dirp: *mut DIR) -> *mut dirent => tadb_readdir {
        if dirp.is_null() {
            return real!(readdir)(dirp);
        }

        let hooked_dir = &mut *(dirp as *mut HookedDir);
        match hooked_dir {
            &mut HookedDir::Native(dirp) => {
                log!("[TADB] called readdir with native DIR* {:?}", dirp);
                let result = real!(readdir)(dirp);
                if let Some(r) = result.as_ref() {
                    log!("[TADB] readdir returned dirent with d_name={}", to_string(to_cstr(&r.d_name)));
                }
                result
            }
            &mut HookedDir::Virtual(DirStream{ref mut pos, ref mut entry}) => {
                log!("[TADB] called readdir with virtual DirStream");
                match pos {
                    0 => {
                        *pos += 1;
                        log!("[TADB] readdir returned dirent with d_name={}", to_string(to_cstr(&entry.d_name)));
                        entry as *mut dirent
                    }
                    _ => null_mut()
                }
            }
        }
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

type OpenFn = unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int;
lazy_static! {
    static ref REAL_OPEN: OpenFn = unsafe{ mem::transmute(redhook::ld_preload::dlsym_next("open\0")) };
}

#[no_mangle]
pub unsafe extern "C" fn open(pathname: *const c_char, flags: c_int, mut args: ...) -> c_int {
    let name = to_string(CStr::from_ptr(pathname));
    // prevent infinite recursion when logfile is first initialized
    if name != "./tadb-log.txt" {
        log!("[TADB] called open with pathname={} flags={}", name, flags);
    }

    if name.starts_with(BASE_DIR_ORIG) { // assuming there is always only one usb device
        if let Ok(usb_fd_str) = env::var("TERMUX_USB_FD") {
            if let Ok(usb_fd) = usb_fd_str.parse::<c_int>() {
                if let Err(e) = lseek(usb_fd, 0, Whence::SeekSet) {
                    log!("[TADB] error seeking fd {}: {}", usb_fd, e);
                }
                log!("[TADB] open hook returning fd with value {}", usb_fd);
                return usb_fd;
            }
        }
    }

    let result = if (flags & O_CREAT) == 0 {
        REAL_OPEN(pathname, flags)
    } else {
        REAL_OPEN(pathname, flags, args.arg::<mode_t>())
    };

    // prevent infinite recursion when logfile is first initialized
    if name != "./tadb-log.txt" {
        log!("[TADB] open returned fd with value {}", result);
    }
    result
}
