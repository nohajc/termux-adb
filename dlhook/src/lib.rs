use std::{fs, mem, path::PathBuf, marker::PhantomData};

// use libc::{c_char, c_int, c_void};
use procfs::process::{Process, MMapPath};

use goblin::elf::Elf;

pub use once_cell::sync::Lazy;

// #[link(name = "dl")]
// extern "C" {
//     fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
//     fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *const c_void;
// }

// const RTLD_LAZY: c_int = 0x1;
// const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;

#[derive(Clone, Copy)]
pub struct NamedFunc<FnPtr> {
    _func: PhantomData<FnPtr>,
    name: &'static str,
}

impl<FnPtr> NamedFunc<FnPtr> {
    pub fn must_coerce(&self) -> FnPtr {
        let null_addr = 0;
        unsafe{ mem::transmute_copy(&null_addr) }
    }
}

pub fn named_func<FnPtr>(_func: FnPtr, name: &'static str) -> NamedFunc<FnPtr> {
    NamedFunc{_func: PhantomData, name}
}

fn find_mapped_library(name: &str) -> (usize, usize, PathBuf) {
    let maps = Process::myself().unwrap().maps().unwrap();
    for mm in &maps {
        match mm.pathname {
            MMapPath::Path(ref p) => {
                if p.to_string_lossy().contains(&format!("/{}", name)) {
                    return (mm.address.0 as usize, (mm.address.1 - mm.address.0) as usize, p.clone())
                }
            }
            _ => ()
        }
    }
    (0, 0, PathBuf::default())
}

pub struct Lib {
    base: usize,
    raw: Vec<u8>,
}

impl Lib {
    pub fn size(&self) -> usize {
        self.raw.len()
    }
}

pub struct LibHandle<'a> {
    base: usize,
    elf: Elf<'a>,
}

unsafe impl<'a> Sync for LibHandle<'a> {}
unsafe impl<'a> Send for LibHandle<'a> {}

impl<'a> LibHandle<'a> {
    pub fn elf(&self) -> &Elf {
        &self.elf
    }
}

impl Lib {
    pub fn new(filename: &str) -> Option<Self> {
        let (base, _size, path) = find_mapped_library(filename);
        // println!("DEBUG base: {:#x}", base);
        // let data = unsafe{ slice::from_raw_parts(base as *const u8, size) };
        // let raw = data.to_owned();
        // Some(Lib{base, raw})
        fs::read(path).ok().map(|raw| {
            Lib{base, raw}
        })
    }

    pub fn handle(&self) -> Option<LibHandle> {
        let base = self.base;
        Elf::parse(&self.raw).ok().map(|elf| {
            LibHandle{base, elf}
        })
    }
}

impl<'a> LibHandle<'a> {
    pub fn sym_addr<FnPtr>(&self, nf: NamedFunc<FnPtr>) -> usize {
        for sym in self.elf.dynsyms.iter() {
            let sym_name = self.elf.dynstrtab.get_at(sym.st_name).unwrap_or("");
            if sym_name == nf.name {
                // println!("DEBUG rel_addr: {:#x}", sym.st_value);
                let addr = self.base + sym.st_value as usize;
                // println!("DEBUG abs_addr: {:#x}", addr);
                return addr;
            }
        }

        0
    }
}

#[macro_export]
macro_rules! func {
    ($lib_handle:ident, $real_fn:ident) => {{
        // ::dlhook::named_func($real_fn, concat!(stringify!($real_fn), "\0"))
        let nf = ::dlhook::named_func($real_fn, stringify!($real_fn));
        let addr = $lib_handle.sym_addr(nf);
        if false {
            // this is a type check which ensures
            // we're assigning the address of real_fn
            // to a pointer of compatible function type
            nf.must_coerce()
        } else {
            unsafe{ mem::transmute(addr) }
        }
    }};
}

#[macro_export]
macro_rules! lib_handle {
    ($lib_name:expr) => { || {
        static LIB: ::dlhook::Lazy<Option<::dlhook::Lib>> = ::dlhook::Lazy::new(|| {
            ::dlhook::Lib::new($lib_name)
        });
        LIB.as_ref().map(|lib| lib.handle()).flatten().unwrap()
    }};
}
