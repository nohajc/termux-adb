use std::{fs, mem, path::PathBuf, marker::PhantomData, ffi::CString};

use libc::{c_void, dlopen, dlsym, RTLD_LAZY};
use procfs::process::{Process, MMapPath};

use goblin::elf::Elf;

use atomic::{Atomic, Ordering};

pub use once_cell::sync::Lazy;

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

type LazyClosure<'a, T> = Lazy<T, Box<dyn FnOnce() -> T + Send + 'a>>;

struct MemMapLib {
    base: usize,
    raw: Vec<u8>,
}

struct MemMapLibHandle<'a> {
    base: usize,
    elf: Option<Elf<'a>>,
}

#[derive(Clone, Copy)]
enum ImplType {
    Detect,
    MemMap,
    DynLink,
}

pub struct Lib {
    filename: &'static str,
    mm_lib: LazyClosure<'static, MemMapLib>,
}

pub struct LibHandle<'a> {
    impl_type: Atomic<ImplType>,
    mm_hnd: LazyClosure<'a, MemMapLibHandle<'a>>,
    dl_hnd: *mut c_void,
}

unsafe impl<'a> Sync for LibHandle<'a> {}
unsafe impl<'a> Send for LibHandle<'a> {}

impl Lib {
    pub fn new(filename: &'static str) -> Option<Self> {
        // println!("DEBUG base: {:#x}", base);
        // let data = unsafe{ slice::from_raw_parts(base as *const u8, size) };
        // let raw = data.to_owned();
        // Some(Lib{base, raw})

        let mm_lib: LazyClosure<MemMapLib> = Lazy::new(Box::new(|| {
            let (base, _size, path) = find_mapped_library(filename);
            let raw = match fs::read(path).ok() {
                Some(data) => data,
                None => vec![],
            };
            MemMapLib{base, raw}
        }));

        Some(Lib{filename, mm_lib})
        // fs::read(path).ok().map(|raw| {
        //     Lib{base, raw}
        // })
    }

    pub fn handle(&self) -> Option<LibHandle> {
        let mm_hnd: LazyClosure<MemMapLibHandle> = Lazy::new(Box::new(|| {
            let base = self.mm_lib.base;
            let elf = Elf::parse(&self.mm_lib.raw).ok();
            MemMapLibHandle{base, elf}
        }));

        let fname = CString::new(self.filename).unwrap();
        let dl_hnd = unsafe{ dlopen(fname.as_ptr(), RTLD_LAZY) };

        Some(LibHandle{impl_type: Atomic::new(ImplType::Detect), mm_hnd, dl_hnd})
        // Elf::parse(&self.raw).ok().map(|elf| {
        //     LibHandle{base, elf}
        // })
    }
}

impl<'a> LibHandle<'a> {
    fn mm_sym_addr<FnPtr: Copy>(&self, nf: NamedFunc<FnPtr>) -> usize {
        self.mm_hnd.elf.as_ref().map_or_else(|| 0, |elf| {
            for sym in elf.dynsyms.iter() {
                let sym_name = elf.dynstrtab.get_at(sym.st_name).unwrap_or("");
                if sym_name == nf.name {
                    // println!("DEBUG rel_addr: {:#x}", sym.st_value);
                    let addr = self.mm_hnd.base + sym.st_value as usize;
                    // println!("DEBUG abs_addr: {:#x}", addr);
                    return addr;
                }
            }
            0
        })
    }

    fn dl_sym_addr<FnPtr: Copy>(&self, nf: NamedFunc<FnPtr>) -> usize {
        let sym = CString::new(nf.name).unwrap();
        unsafe{ dlsym(self.dl_hnd, sym.as_ptr()) as usize }
        // unsafe{ dlsym(RTLD_NEXT, sym.as_ptr()) as usize }
    }

    pub fn sym_addr<FnPtr: Copy>(&self, nf: NamedFunc<FnPtr>, equals_hook: fn(usize) -> bool) -> usize {
        match self.impl_type.load(Ordering::SeqCst) {
            ImplType::Detect => {
                let sym_addr = self.dl_sym_addr(nf);
                if !equals_hook(sym_addr) {
                    println!("DEBUG: dlsym works!");
                    self.impl_type.store(ImplType::DynLink, Ordering::SeqCst);
                    sym_addr
                } else {
                    println!("DEBUG: dlsym doesn't work, using fallback impl!");
                    self.impl_type.store(ImplType::MemMap, Ordering::SeqCst);
                    self.mm_sym_addr(nf)
                }
            }
            ImplType::DynLink => {
                self.dl_sym_addr(nf)
            }
            ImplType::MemMap => {
                self.mm_sym_addr(nf)
            }
        }
    }
}

#[macro_export]
macro_rules! func {
    ($lib_handle:ident, $real_fn:ident) => {{
        let nf = ::dlhook::named_func($real_fn, stringify!($real_fn));
        let addr = $lib_handle.sym_addr(nf, |sym| {
            sym as *const usize == $real_fn as *const usize
        });
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
    ($lib_name:expr) => {{
        static LIB: ::dlhook::Lazy<Option<::dlhook::Lib>> = ::dlhook::Lazy::new(|| {
            ::dlhook::Lib::new($lib_name)
        });
        LIB.as_ref().map(|lib| lib.handle()).flatten().unwrap()
    }};
}
