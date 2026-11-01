//! The C runtime's side: the functions this host calls (runtime/win/host_win.c,
//! runtime/win/plat_win.c, runtime/web/main_web.c) and the shared memory the
//! runtime publishes (runtime/web/host_abi.h).
//!
//! Every structure the runtime shares lives in its address space at a known
//! pointer; the 32-bit addresses inside them (the page's "wasm addresses")
//! count from the guest memory base (`mem_base`).

use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};


extern "C" {
    fn nfsu2_game_main(argc: c_int, argv: *mut *mut c_char) -> c_int;
    fn nfsu2_win_selftest() -> c_int;
    fn mem_fault_handler_install();   // runtime/mem.c: a crash in the game reports its guest state in the log
    fn nfsu2_host_lookup(kind: c_int, size: *mut u32, ms: u32) -> *mut c_void;
    fn nfsu2_host_mem_base() -> *mut u8;
    fn nfsu2_now_ms() -> f64;
    fn nfsu2_wait32(p: *const c_void, v: i32, ns: i64) -> c_int;
    fn nfsu2_notify(p: *const c_void, count: u32) -> u32;
    fn nfsu2_host_log_to(path: *const c_char) -> c_int;
    fn nfsu2_host_log(line: *const c_char);
    fn setenv(name: *const c_char, value: *const c_char, overwrite: c_int) -> c_int;   // runtime/win/plat_win.c
}

/// Sets an environment variable for this host and for the runtime: the C runtime library keeps its own copy of
/// the environment (getenv), which SetEnvironmentVariable (Rust's set_var) does not change.
pub fn set_env(k: &str, v: &str) {
    std::env::set_var(k, v);
    if let (Ok(k), Ok(v)) = (std::ffi::CString::new(k), std::ffi::CString::new(v)) { unsafe { setenv(k.as_ptr(), v.as_ptr(), 1); } }
}

/// A line for the log (the runtime's stderr: the console, or the log file once `log_to` sent it there).
pub fn log(s: &str) {
    if let Ok(c) = std::ffi::CString::new(s.replace('\0', "")) { unsafe { nfsu2_host_log(c.as_ptr()) } }
}
/// From now on the log goes to this file (the runtime's stderr included).
pub fn log_to(path: &str) -> bool {
    std::ffi::CString::new(path).map(|c| unsafe { nfsu2_host_log_to(c.as_ptr()) } == 0).unwrap_or(false)
}

/// runtime/web/host_abi.h
pub const KIND_GPU: i32 = 1;
pub const KIND_AUDIO: i32 = 2;
pub const KIND_INPUT: i32 = 3;
pub const KIND_VSYNC: i32 = 6;

/// Runs the game (main_web.c's main) on the calling thread; returns its exit code.
pub fn game_main(args: &[String]) -> i32 {
    let c: Vec<std::ffi::CString> = args.iter().map(|a| std::ffi::CString::new(a.as_str()).unwrap_or_default()).collect();
    let mut p: Vec<*mut c_char> = c.iter().map(|s| s.as_ptr() as *mut c_char).collect();
    p.push(std::ptr::null_mut());
    unsafe { nfsu2_game_main(c.len() as c_int, p.as_mut_ptr()) }
}

/// Reports access violations with the guest's state (the log's last lines after a crash).
pub fn fault_handler() { unsafe { mem_fault_handler_install() } }

/// The renderer's self test without the game (runtime/win/selftest_win.c); 0 = passed.
pub fn selftest() -> i32 { unsafe { nfsu2_win_selftest() } }

/// The structure the runtime registered for `kind`, waiting at most `ms` for it.
pub fn lookup(kind: i32, ms: u32) -> Option<(*mut u8, u32)> {
    let mut size = 0u32;
    let p = unsafe { nfsu2_host_lookup(kind, &mut size, ms) };
    if p.is_null() { None } else { Some((p as *mut u8, size)) }
}

/// Milliseconds on the runtime's clock (the page's performance.timeOrigin + now()).
pub fn now_ms() -> f64 { unsafe { nfsu2_now_ms() } }

/// The base the shared structures' 32-bit addresses count from.
pub fn mem_base() -> *mut u8 { unsafe { nfsu2_host_mem_base() } }

/// wasm's memory.atomic.wait32 on a shared word: wait while it is `v`, at most `ms` (< 0: forever).
pub fn wait32(a: &AtomicI32, v: i32, ms: f64) {
    unsafe { nfsu2_wait32(a as *const AtomicI32 as *const c_void, v, if ms < 0.0 { -1 } else { (ms * 1e6) as i64 }); }
}
pub fn notify(a: &AtomicI32) { unsafe { nfsu2_notify(a as *const AtomicI32 as *const c_void, u32::MAX); } }

/// A view of shared memory at a host pointer: word access by byte offset.
#[derive(Clone, Copy)]
pub struct Shm(pub *mut u8);
unsafe impl Send for Shm {}
unsafe impl Sync for Shm {}
#[allow(dead_code)]
impl Shm {
    pub fn at(self, off: usize) -> Shm { Shm(unsafe { self.0.add(off) }) }
    pub fn u32(self, off: usize) -> u32 { unsafe { std::ptr::read_volatile(self.0.add(off) as *const u32) } }
    pub fn i32(self, off: usize) -> i32 { self.u32(off) as i32 }
    pub fn f32(self, off: usize) -> f32 { f32::from_bits(self.u32(off)) }
    pub fn f64(self, off: usize) -> f64 { unsafe { std::ptr::read_volatile(self.0.add(off) as *const f64) } }
    pub fn set_u32(self, off: usize, v: u32) { unsafe { std::ptr::write_volatile(self.0.add(off) as *mut u32, v) } }
    pub fn set_f64(self, off: usize, v: f64) { unsafe { std::ptr::write_volatile(self.0.add(off) as *mut f64, v) } }
    pub fn set_u8(self, off: usize, v: u8) { unsafe { std::ptr::write_volatile(self.0.add(off), v) } }
    pub fn atomic(self, off: usize) -> &'static AtomicI32 { unsafe { &*(self.0.add(off) as *const AtomicI32) } }
    pub fn atomic_u(self, off: usize) -> &'static AtomicU32 { unsafe { &*(self.0.add(off) as *const AtomicU32) } }
    pub fn load(self, off: usize) -> i32 { self.atomic(off).load(Ordering::SeqCst) }
    pub fn store(self, off: usize, v: i32) { self.atomic(off).store(v, Ordering::SeqCst) }
    pub fn add(self, off: usize, v: i32) -> i32 { self.atomic(off).fetch_add(v, Ordering::SeqCst) }
    pub fn cas(self, off: usize, old: i32, new: i32) -> i32 {
        match self.atomic(off).compare_exchange(old, new, Ordering::SeqCst, Ordering::SeqCst) { Ok(v) | Err(v) => v }
    }
    /// `n` bytes at `off` (they stay the runtime's: read them before handing the record back)
    pub fn bytes(self, off: usize, n: usize) -> &'static [u8] { unsafe { std::slice::from_raw_parts(self.0.add(off), n) } }
    pub fn bytes_mut(self, off: usize, n: usize) -> &'static mut [u8] { unsafe { std::slice::from_raw_parts_mut(self.0.add(off), n) } }
}

/// Guest memory: the 32-bit addresses in the shared structures.
pub fn mem() -> Shm { Shm(mem_base()) }
