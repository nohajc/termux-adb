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
    sync::{atomic::{AtomicBool, Ordering}, Mutex},
    collections::HashMap,
    path::{Path, PathBuf},
    ptr::null_mut,
    time::Duration,
    thread,
};

use anyhow::Context;

use define_hook::define_hook;
use libc::{
    DIR, dirent, O_CREAT, mode_t,
    DT_CHR, DT_DIR, openat, AT_FDCWD,
    c_ushort, c_uchar, c_uint
};

use rand::Rng;

use dlhook::*;

use nix::{
    unistd::{lseek, Whence},
    sys::{stat::fstat, memfd::{memfd_create, MemFdCreateFlag}},
    fcntl::readlink
};

// TODO: maybe try to link against libusb properly
use rusb::{constants::LIBUSB_OPTION_NO_DEVICE_DISCOVERY, UsbContext};

use log::{debug, info, error};

use ctor::ctor;
use sendfd::RecvWithFd;

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

static TERMUX_USB_FD: Lazy<Mutex<Option<c_int>>> = Lazy::new(|| Mutex::new({
    env::var("TERMUX_USB_FD").map(|usb_fd_str| {
        usb_fd_str.parse::<c_int>().ok()
    }).ok().flatten().and_then(|fd| match fd {
        -1 => None,
        i => Some(i)
    })
}));

fn get_termux_fd() -> Option<c_int> {
    *TERMUX_USB_FD.lock().unwrap()
}

static TERMUX_USB_DEV: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new({
    env::var("TERMUX_USB_DEV").ok()
        .and_then(|dev| match dev.as_str() {
            "none" => None,
            _ => Some(dev)
        }).map(|str| PathBuf::from(str))
}));

#[derive(Clone)]
struct UsbSerial {
    number: String,
    path: PathBuf,
}

fn init_libusb_device_serial(usb_fd: c_int) -> anyhow::Result<UsbSerial> {
    debug!("calling libusb_set_option");
    unsafe{ rusb::ffi::libusb_set_option(null_mut(), LIBUSB_OPTION_NO_DEVICE_DISCOVERY) };

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

fn print_err_and_convert<T>(r: anyhow::Result<T>) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            error!("{}", e);
            None
        }
    }
}

static TERMUX_USB_SERIAL: Lazy<Mutex<Option<UsbSerial>>> = Lazy::new(|| Mutex::new({
    if let Ok(_) = env::var("TERMUX_ADB_SERVER_RUNNING") {
        debug!("reading TERMUX_USB_FD");
        print_err_and_convert(get_termux_fd().context("error: missing TERMUX_USB_FD")).and_then(|usb_fd| {
            print_err_and_convert(init_libusb_device_serial(usb_fd))
        })
    } else {
        env::set_var("TERMUX_ADB_SERVER_RUNNING", "true");
        None
    }
}));

fn get_usb_device_serial() -> Option<UsbSerial> {
    TERMUX_USB_SERIAL.lock().unwrap().clone()
}

static LIBADBHOOKS_INITIALIZED: AtomicBool = AtomicBool::new(false);

static LIBC_FILE: Lazy<Lib> = Lazy::new(|| Lib::new("libc.so").unwrap());
static LIBC: Lazy<LibHandle> = Lazy::new(|| LIBC_FILE.handle().unwrap());
// static LIBC: Lazy<LibHandle> = Lazy::new(lib_handle!("libc.so"));

#[ctor]
fn libadbhooks_ctor() {
    env_logger::init();

    debug!("libc loaded size: {}", LIBC_FILE.size());
    debug!("libc ELF loaded: {}", LIBC.elf().is_lib);

    // resolve the address of libc close to prevent deadlock
    let _real_close = *REAL_CLOSE;

    // debug!("opendir hook address: {:?}", opendir as *const usize);
    // debug!("opendir calculated address: {:?}", *REAL_OPENDIR as *const usize);

    // let dlsym_opendir: OpenDirFn = unsafe{
    //     mem::transmute(redhook::ld_preload::dlsym_next("opendir\0"))
    // };
    // debug!("opendir dlsym address: {:?}", dlsym_opendir as *const usize);

    // libusb_init hanged when called as Lazy static from opendir
    // so instead we use global constructor function which resolves the issue
    if let Some(usb_sn) = get_usb_device_serial() {
        info!("libusb device serial: {}", &usb_sn.number);
    }

    if let Ok(_) = env::var("TERMUX_ADB_SERVER_RUNNING") {
        thread::spawn(|| {
            if let Err(e) = start_socket_listener() {
                error!("socket listener error: {}", e);
            }
        });
    }

    LIBADBHOOKS_INITIALIZED.store(true, Ordering::SeqCst);
}

fn start_socket_listener() -> anyhow::Result<()> {
    let sock_fd: RawFd = env::var("TERMUX_ADB_SOCK_FD")?.parse()?;
    let socket = unsafe{ UnixDatagram::from_raw_fd(sock_fd) };

    info!("listening on socket fd {}", sock_fd);
    _ = socket.set_read_timeout(None);
    loop {
        let mut buf = vec![0; 256];
        let mut fds = vec![0; 1];
        match socket.recv_with_fd(buf.as_mut_slice(), fds.as_mut_slice()) {
            Ok((_, 0)) => {
                error!("received message without usb fd");
            }
            Ok((size, _)) => {
                let usb_dev_path = PathBuf::from(String::from_utf8_lossy(&buf[0..size]).as_ref());
                let usb_fd = fds[0];
                // use the received info as TERMUX_USB_DEV and TERMUX_USB_FD
                info!("received message (size={}) with fd={}: {}", size, usb_fd, usb_dev_path.display());

                update_dir_map(&mut DIR_MAP.lock().unwrap(), &usb_dev_path);
                *TERMUX_USB_DEV.lock().unwrap() = Some(usb_dev_path);
                *TERMUX_USB_FD.lock().unwrap() = Some(usb_fd);
                *TERMUX_USB_SERIAL.lock().unwrap() = print_err_and_convert(init_libusb_device_serial(usb_fd));
            }
            Err(e) => {
                error!("message receive error: {}", e);
            }
        }
    }
}

fn update_dir_map(dir_map: &mut HashMap<PathBuf, DirStream>, usb_dev_path: &Path) {
    dir_map.clear();

    if let Some(usb_dev_name) = usb_dev_path.file_name() {
        let mut last_entry = dirent_new(
            0, DT_CHR, usb_dev_name
        );
        let mut current_dir = usb_dev_path.to_owned();

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

static DIR_MAP: Lazy<Mutex<HashMap<PathBuf, DirStream>>> = Lazy::new(|| Mutex::new({
    let mut dir_map = HashMap::new();
    if let Some(ref usb_dev_path) = *TERMUX_USB_DEV.lock().unwrap() {
        update_dir_map(&mut dir_map, usb_dev_path);
    }

    dir_map
}));

enum HookedDir {
    Native(*mut DIR),
    Virtual(DirStream)
}

impl From<HookedDir> for *mut DIR {
    fn from(hd: HookedDir) -> Self {
        Box::into_raw(Box::new(hd)) as Self
    }
}

type OpenDirFn = unsafe extern "C" fn(*const u8) -> *mut DIR;
static REAL_OPENDIR: Lazy<OpenDirFn> = Lazy::new(|| func!(LIBC, opendir));
// static REAL_OPENDIR: Lazy<OpenDirFn> = Lazy::new(|| unsafe{
//     mem::transmute(redhook::ld_preload::dlsym_next("opendir\0"))
// });

#[no_mangle]
unsafe extern "C" fn opendir(name: *const c_char) -> *mut DIR {
    if name.is_null() {
        return REAL_OPENDIR(name);
    }

    let name_cstr = CStr::from_ptr(name);
    let name_str = to_string(name_cstr);

    if name_str.starts_with(BASE_DIR_ORIG) {
        let name_osstr = to_os_str(name_cstr);
        if let Some(dirstream) = DIR_MAP.lock().unwrap().get(&PathBuf::from(name_osstr)) {
            debug!("called opendir with {}, remapping to virtual DirStream", &name_str);
            return HookedDir::Virtual(dirstream.to_owned()).into();
        }
    }

    debug!("called opendir with {}", &name_str);
    let dir = REAL_OPENDIR(name);
    if dir.is_null() {
        return null_mut();
    }
    HookedDir::Native(dir).into()
}

type CloseDirFn = unsafe extern "C" fn(*mut DIR) -> c_int;
static REAL_CLOSEDIR: Lazy<CloseDirFn> = Lazy::new(|| func!(LIBC, closedir));

#[no_mangle]
unsafe extern "C" fn closedir(dirp: *mut DIR) -> c_int {
    if dirp.is_null() {
        return REAL_CLOSEDIR(dirp);
    }

    let hooked_dir = Box::from_raw(dirp as *mut HookedDir);
    match hooked_dir.as_ref() {
        &HookedDir::Native(dirp) => REAL_CLOSEDIR(dirp),
        // nothing to do, hooked_dir along with DirStream
        // will be dropped at the end of this function
        &HookedDir::Virtual(_) => 0
    }
}

type ReadDirFn = unsafe extern "C" fn(*mut DIR) -> *mut dirent;
static REAL_READDIR: Lazy<ReadDirFn> = Lazy::new(|| func!(LIBC, readdir));

#[no_mangle]
unsafe extern "C" fn readdir(dirp: *mut DIR) -> *mut dirent {
    if dirp.is_null() {
        return REAL_READDIR(dirp);
    }

    let hooked_dir = &mut *(dirp as *mut HookedDir);
    match hooked_dir {
        &mut HookedDir::Native(dirp) => {
            debug!("called readdir with native DIR* {:?}", dirp);
            let result = REAL_READDIR(dirp);
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

type CloseFn = unsafe extern "C" fn(c_int) -> c_int;
static REAL_CLOSE: Lazy<CloseFn> = Lazy::new(|| func!(LIBC, close));

static DELAYED_CLOSE_FDS: Mutex<Vec<c_int>> = Mutex::new(vec![]);
static DELAYED_FDS_PROCESSED: AtomicBool = AtomicBool::new(false);

#[no_mangle]
unsafe extern "C" fn close(fd: c_int) -> c_int {
    if !LIBADBHOOKS_INITIALIZED.load(Ordering::SeqCst) {
        let mut delayed_fds = DELAYED_CLOSE_FDS.lock().unwrap();
        delayed_fds.push(fd);
        return 0;
    }

    if !DELAYED_FDS_PROCESSED.load(Ordering::SeqCst) {
        let mut delayed_fds = DELAYED_CLOSE_FDS.lock().unwrap();
        for dfd in &*delayed_fds {
            REAL_CLOSE(*dfd);
        }
        delayed_fds.clear();
        DELAYED_FDS_PROCESSED.store(true, Ordering::SeqCst);
    }

    if let Some(usb_fd) = get_termux_fd() {
        // usb fd must not be closed
        if usb_fd == fd {
            return 0;
        }
    }
    REAL_CLOSE(fd)
}

// type OpenFn = unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int;
// static REAL_OPEN: Lazy<OpenFn> = Lazy::new(|| func!(LIBC, open));
// static REAL_OPEN: Lazy<OpenFn> = Lazy::new(|| unsafe{
//     mem::transmute(redhook::ld_preload::dlsym_next("open\0"))
// });

#[no_mangle]
pub unsafe extern "C" fn open(pathname: *const c_char, flags: c_int, mut args: ...) -> c_int {
    if !pathname.is_null() {
        let name = to_string(CStr::from_ptr(pathname));

        debug!("called open with pathname={} flags={}", name, flags);

        let name_path = PathBuf::from(&name);
        {
            if Some(&name_path) == TERMUX_USB_DEV.lock().unwrap().as_ref() {
                if let Some(usb_fd) = get_termux_fd() {
                    if let Err(e) = lseek(usb_fd, 0, Whence::SeekSet) {
                        error!("error seeking fd {}: {}", usb_fd, e);
                    }
                    info!("open hook returning fd with value {}", usb_fd);
                    return usb_fd;
                }
            }
        }

        if LIBADBHOOKS_INITIALIZED.load(Ordering::SeqCst) {
            let usb_serial = get_usb_device_serial();
            if Some(&name_path) == usb_serial.as_ref().map(|s| &s.path) {
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
        openat(AT_FDCWD, pathname, flags)
    } else {
        openat(AT_FDCWD, pathname, flags, args.arg::<mode_t>() as c_uint)
    };

    debug!("open returned fd with value {}", result);

    result
}
