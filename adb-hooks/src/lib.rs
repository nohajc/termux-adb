#![feature(c_variadic)]

#[macro_use]
extern crate lazy_static;

use std::{
    env,
    ffi::{CStr, OsStr},
    fs::File,
    io::Write,
    mem,
    os::{
        unix::ffi::OsStrExt,
        raw::{c_char, c_int}
    },
    sync::Mutex, collections::HashMap,
    path::PathBuf, ptr::null_mut, time::Duration,
};

use anyhow::Context;
use libc::{
    DIR, dirent, O_CREAT, mode_t,
    DT_CHR, DT_DIR, off_t,
    c_ushort, c_uchar
};

use rand::Rng;

use redhook::{
    hook, real,
};

use nix::{unistd::{lseek, Whence}, sys::stat::{fstat, major, minor}};
use rusb::{constants::LIBUSB_OPTION_NO_DEVICE_DISCOVERY, UsbContext};

use ctor::ctor;

lazy_static! {
    static ref LOG: Mutex<File> = Mutex::new(File::create("./tadb-log.txt").unwrap());
}

const BASE_DIR_ORIG: &str = "/dev/bus/usb";

macro_rules! log {
    ($($arg:tt)*) => {
        _ = writeln!(&mut *LOG.lock().unwrap(), $($arg)*);
    };
}

fn to_string(s: &CStr) -> String {
    s.to_string_lossy().into_owned()
}

fn to_os_str(s: &CStr) -> &OsStr {
    OsStr::from_bytes(s.to_bytes())
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
    static ref TERMUX_USB_FD: Option<c_int> = env::var("TERMUX_USB_FD")
        .map(|usb_fd_str| usb_fd_str.parse::<c_int>().ok()).ok().flatten();
}

fn init_libusb_device_serial() -> anyhow::Result<String> {
    eprintln!("[TADB] calling libusb_set_option");
    unsafe{ rusb::ffi::libusb_set_option(null_mut(), LIBUSB_OPTION_NO_DEVICE_DISCOVERY) };

    eprintln!("[TADB] reading TERMUX_USB_FD");
    let usb_fd = TERMUX_USB_FD.context("error: missing TERMUX_USB_FD")?;

    lseek(usb_fd, 0, Whence::SeekSet)
        .with_context(|| format!("error seeking fd: {}", usb_fd))?;

    let ctx = rusb::Context::new().context("libusb_init error")?;

    eprintln!("[TADB] opening device from {}", usb_fd);
    let usb_handle = unsafe{
        ctx.open_device_with_fd(usb_fd).context("error opening device")
    }?;

    eprintln!("[TADB] getting device from handle");
    let usb_dev = usb_handle.device();

    eprintln!("[TADB] requesting device descriptor");
    let usb_dev_desc = usb_dev.device_descriptor()
        .context("error getting device descriptor")?;

    let vid = usb_dev_desc.vendor_id();
    let pid = usb_dev_desc.product_id();
    let iser = usb_dev_desc.serial_number_string_index();
    eprintln!("[TADB] device descriptor: vid={}, pid={}, iSerial={}", vid, pid, iser.unwrap_or(0));

    let timeout = Duration::from_secs(1);
    let languages = usb_handle.read_languages(timeout)
        .context("error getting supported languages for reading string descriptors")?;

    let serial_number = usb_handle.read_serial_number_string(
        languages[0], &usb_dev_desc, timeout
    ).context("error reading serial number of the device")?;

    // let ports = usb_dev.port_numbers().context("error getting usb device ports")?;
    // let bus_num = usb_dev.bus_number();

    // let mut dev_path = format!("/sys/bus/usb/devices/{}-{}", bus_num, ports[0]);
    // for i in 1..ports.len() {
    //     dev_path += &format!(".{}", ports[i]);
    // }
    // dev_path += "/serial";

    let st = fstat(usb_fd).context("error: could not stat TERMUX_USB_FD")?;
    // "/sys/dev/char/%d:%d"
    let dev_path_link = format!("/sys/dev/char/{}:{}", major(st.st_rdev), minor(st.st_rdev));
    eprintln!("[TADB] device serial path link: {}", dev_path_link);

    Ok(serial_number)
}

lazy_static! {
    static ref TERMUX_USB_SERIAL: Option<String> = {
        match init_libusb_device_serial() {
            Ok(sn) => Some(sn),
            Err(e) => {
                eprintln!("[TADB] {}", e);
                None
            }
        }
    };
}

fn get_usb_device_serial() -> &'static str {
    TERMUX_USB_SERIAL.as_ref().map(|sn| sn.as_str()).unwrap_or("(no serial number)")
}

#[ctor]
fn libusb_device_serial_ctor() {
    // libusb_init hanged when called as lazy_static from opendir
    // so instead we use global constructor function which resolves the issue
    eprintln!("[TADB] libusb device serial: {}", get_usb_device_serial());
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
        if name.is_null() {
            return real!(opendir)(name);
        }

        let name_cstr = CStr::from_ptr(name);
        let name_str = to_string(name_cstr);

        if name_str.starts_with(BASE_DIR_ORIG) {
            let name_osstr = to_os_str(name_cstr);
            if let Some(dirstream) = DIR_MAP.get(&PathBuf::from(name_osstr)) {
                log!("[TADB] called opendir with {}, remapping to virtual DirStream", &name_str);
                return HookedDir::Virtual(dirstream.to_owned()).into();
            }
        }

        log!("[TADB] called opendir with {}", &name_str);
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
        if let Some(usb_fd) = TERMUX_USB_FD.clone() {
            // usb fd must not be closed
            if usb_fd == fd {
                return 0;
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
    let name = if !pathname.is_null() {
        let name = to_string(CStr::from_ptr(pathname));
        // prevent infinite recursion when logfile is first initialized
        if name != "./tadb-log.txt" {
            log!("[TADB] called open with pathname={} flags={}", name, flags);
        }

        if name.starts_with(BASE_DIR_ORIG) { // assuming there is always only one usb device
            if let Some(usb_fd) = TERMUX_USB_FD.clone() {
                if let Err(e) = lseek(usb_fd, 0, Whence::SeekSet) {
                    log!("[TADB] error seeking fd {}: {}", usb_fd, e);
                }
                log!("[TADB] open hook returning fd with value {}", usb_fd);
                return usb_fd;
            }
        }
        name
    } else {
        "".to_owned()
    };

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
