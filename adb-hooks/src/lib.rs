#![feature(c_variadic)]

use std::{
    env,
    ffi::{CStr, OsStr},
    mem,
    os::{
        unix::{
            ffi::OsStrExt,
            net::UnixDatagram,
        },
        raw::{c_char, c_int}, fd::{RawFd, FromRawFd}
    },
    sync::atomic::{AtomicBool, Ordering}, collections::HashMap,
    path::PathBuf, ptr::null_mut, time::Duration,
    thread,
};

use anyhow::Context;
use libc::{
    DIR, dirent, O_CREAT, mode_t,
    DT_CHR, DT_DIR,
    c_ushort, c_uchar, c_uint
};

use once_cell::sync::Lazy;

use rand::Rng;

use redhook::{
    hook, real,
};

use nix::{
    unistd::{lseek, Whence},
    sys::{stat::fstat, memfd::{memfd_create, MemFdCreateFlag}},
    fcntl::readlink
};

// TODO: maybe try to link against libusb properly
use rusb::{constants::LIBUSB_OPTION_NO_DEVICE_DISCOVERY, UsbContext};

use log::{debug, info, error};

use ctor::ctor;

const BASE_DIR_ORIG: &str = "/dev/bus/usb";

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

fn dirent_new(off: i64, typ: c_uchar, name: &OsStr) -> dirent {
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

static TERMUX_USB_FD: Lazy<Option<c_int>> = Lazy::new(|| {
    env::var("TERMUX_USB_FD").map(|usb_fd_str| {
        usb_fd_str.parse::<c_int>().ok()
    }).ok().flatten().and_then(|fd| match fd {
        -1 => None,
        i => Some(i)
    })
});

static TERMUX_USB_DEV: Lazy<Option<PathBuf>> = Lazy::new(|| {
    env::var("TERMUX_USB_DEV").ok()
        .and_then(|dev| match dev.as_str() {
            "none" => None,
            _ => Some(dev)
        }).map(|str| PathBuf::from(str))
});

struct UsbSerial {
    number: String,
    path: PathBuf,
}

fn init_libusb_device_serial() -> anyhow::Result<UsbSerial> {
    debug!("calling libusb_set_option");
    unsafe{ rusb::ffi::libusb_set_option(null_mut(), LIBUSB_OPTION_NO_DEVICE_DISCOVERY) };

    debug!("reading TERMUX_USB_FD");
    let usb_fd = TERMUX_USB_FD.context("error: missing TERMUX_USB_FD")?;

    lseek(usb_fd, 0, Whence::SeekSet)
        .with_context(|| format!("error seeking fd: {}", usb_fd))?;

    let ctx = rusb::Context::new().context("libusb_init error")?;

    debug!("opening device from {}", usb_fd);
    let usb_handle = unsafe{
        ctx.open_device_with_fd(usb_fd).context("error opening device")
    }?;

    debug!("getting device from handle");
    let usb_dev = usb_handle.device();

    debug!("requesting device descriptor");
    let usb_dev_desc = usb_dev.device_descriptor()
        .context("error getting device descriptor")?;

    let vid = usb_dev_desc.vendor_id();
    let pid = usb_dev_desc.product_id();
    let iser = usb_dev_desc.serial_number_string_index();
    debug!("device descriptor: vid={}, pid={}, iSerial={}", vid, pid, iser.unwrap_or(0));

    let timeout = Duration::from_secs(1);
    let languages = usb_handle.read_languages(timeout)
        .context("error getting supported languages for reading string descriptors")?;

    let serial_number = usb_handle.read_serial_number_string(
        languages[0], &usb_dev_desc, timeout
    ).context("error reading serial number of the device")?;

    let st = fstat(usb_fd).context("error: could not stat TERMUX_USB_FD")?;
    let dev_path_link = format!("/sys/dev/char/{}:{}", major(st.st_rdev), minor(st.st_rdev));

    let dev_path = PathBuf::from(readlink(
    &PathBuf::from(&dev_path_link))
        .context(format!("error: could not resolve symlink {}", &dev_path_link)
    )?);

    let mut dev_serial_path = PathBuf::from("/sys/bus/usb/devices");

    dev_serial_path.push(dev_path.file_name().context("error: could not get device path")?);
    dev_serial_path.push("serial");

    info!("device serial path: {}", dev_serial_path.display());

    Ok(UsbSerial{ number: serial_number, path: dev_serial_path })
}

pub const fn major(dev: u64) -> u64 {
    ((dev >> 32) & 0xffff_f000) |
    ((dev >>  8) & 0x0000_0fff)
}

pub const fn minor(dev: u64) -> u64 {
    ((dev >> 12) & 0xffff_ff00) |
    ((dev      ) & 0x0000_00ff)
}

static TERMUX_USB_SERIAL: Lazy<Option<UsbSerial>> = Lazy::new(|| {
    if let Ok(_) = env::var("TERMUX_ADB_SERVER_RUNNING") {
        match init_libusb_device_serial() {
            Ok(sn) => Some(sn),
            Err(e) => {
                error!("{}", e);
                None
            }
        }
    } else {
        env::set_var("TERMUX_ADB_SERVER_RUNNING", "true");
        None
    }
});

fn get_usb_device_serial<'a>() -> Option<&'a UsbSerial> {
    TERMUX_USB_SERIAL.as_ref()
}

static LIBUSB_INITIALIZED: AtomicBool = AtomicBool::new(false);

#[ctor]
fn libusb_device_serial_ctor() {
    env_logger::init();

    if let Ok(_) = env::var("TERMUX_ADB_SERVER_RUNNING") {
        thread::spawn(|| {
            if let Err(e) = start_socket_listener() {
                error!("socket listener error: {}", e);
            }
        });
    }

    // libusb_init hanged when called as Lazy static from opendir
    // so instead we use global constructor function which resolves the issue
    if let Some(usb_sn) = get_usb_device_serial() {
        info!("libusb device serial: {}", &usb_sn.number);
    }

    LIBUSB_INITIALIZED.store(true, Ordering::SeqCst);
}

fn start_socket_listener() -> anyhow::Result<()> {
    // "/data/data/com.termux/files/usr/tmp"
    let sock_fd: RawFd = env::var("TERMUX_ADB_SOCK_FD")?.parse()?;
    let socket = unsafe{ UnixDatagram::from_raw_fd(sock_fd) };

    info!("listening on socket fd {}", sock_fd);
    _ = socket.set_read_timeout(None);
    loop {
        let mut buf = vec![0; 64];
        match socket.recv(buf.as_mut_slice()) {
            Ok(size) => {
                let msg = String::from_utf8_lossy(&buf[0..size]);
                info!("received message (size={}): {}", size, msg);
            }
            Err(e) => {
                error!("message receive error: {}", e);
            }
        }
    }
}

static DIR_MAP: Lazy<HashMap<PathBuf, DirStream>> = Lazy::new(|| {
    let mut dir_map = HashMap::new();
    if let Some(ref usb_dev_path) = *TERMUX_USB_DEV {
        if let Some(usb_dev_name) = usb_dev_path.file_name() {
            let mut last_entry = dirent_new(
                0, DT_CHR, usb_dev_name
            );
            let mut current_dir = usb_dev_path.clone();

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
});

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
                debug!("called opendir with {}, remapping to virtual DirStream", &name_str);
                return HookedDir::Virtual(dirstream.to_owned()).into();
            }
        }

        debug!("called opendir with {}", &name_str);
        let dir = real!(opendir)(name);
        if dir.is_null() {
            return null_mut();
        }
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
                debug!("called readdir with native DIR* {:?}", dirp);
                let result = real!(readdir)(dirp);
                if let Some(r) = result.as_ref() {
                    debug!("readdir returned dirent with d_name={}", to_string(to_cstr(&r.d_name)));
                }
                result
            }
            &mut HookedDir::Virtual(DirStream{ref mut pos, ref mut entry}) => {
                debug!("called readdir with virtual DirStream");
                match pos {
                    0 => {
                        *pos += 1;
                        debug!("readdir returned dirent with d_name={}", to_string(to_cstr(&entry.d_name)));
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
        if let Some(usb_fd) = *TERMUX_USB_FD {
            // usb fd must not be closed
            if usb_fd == fd {
                return 0;
            }
        }
        real!(close)(fd)
    }
}

type OpenFn = unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int;
static REAL_OPEN: Lazy<OpenFn> = Lazy::new(|| unsafe{
    mem::transmute(redhook::ld_preload::dlsym_next("open\0"))
});

#[no_mangle]
pub unsafe extern "C" fn open(pathname: *const c_char, flags: c_int, mut args: ...) -> c_int {
    if !pathname.is_null() {
        let name = to_string(CStr::from_ptr(pathname));
        // prevent infinite recursion when logfile is first initialized

        debug!("called open with pathname={} flags={}", name, flags);

        let name_path = PathBuf::from(&name);
        if Some(&name_path) == TERMUX_USB_DEV.as_ref() {
            if let Some(usb_fd) = *TERMUX_USB_FD {
                if let Err(e) = lseek(usb_fd, 0, Whence::SeekSet) {
                    error!("error seeking fd {}: {}", usb_fd, e);
                }
                info!("open hook returning fd with value {}", usb_fd);
                return usb_fd;
            }
        }

        if LIBUSB_INITIALIZED.load(Ordering::SeqCst) {
            let usb_serial = get_usb_device_serial();
            if Some(&name_path) == usb_serial.map(|s| &s.path) {
                if let Ok(serial_fd) = memfd_create(
                    CStr::from_ptr("usb-serial\0".as_ptr() as *const c_char),
                    MemFdCreateFlag::empty())
                {
                    let wr_status = nix::unistd::write(
                        serial_fd, usb_serial.unwrap().number.as_bytes());
                    let seek_status = lseek(serial_fd, 0, Whence::SeekSet);

                    match (wr_status, seek_status) {
                        (Ok(_), Ok(_)) => {
                            info!("open hook returning fd with value {}", serial_fd);
                            return serial_fd
                        }
                        _ => ()
                    }
                }
            }
        }
    }

    let result = if (flags & O_CREAT) == 0 {
        REAL_OPEN(pathname, flags)
    } else {
        REAL_OPEN(pathname, flags, args.arg::<mode_t>() as c_uint)
    };

    debug!("open returned fd with value {}", result);

    result
}
