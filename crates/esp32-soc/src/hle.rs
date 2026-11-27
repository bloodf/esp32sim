//! ROM functions that need to call back into the firmware.
//!
//! These are written as ordinary Rust functions running inside a stackful
//! coroutine.  Whenever they need to call a firmware function (a newlib
//! syscall, a comparator, ...) they *yield* a [`GuestCall`]; the machine runs
//! the guest function on the emulated CPU and resumes the coroutine with its
//! return value.

use crate::sys::Sys;
use corosensei::{Coroutine, Yielder};
use xtensa_core::Cpu;

/// Guest trampolines: at TRAMP_ADDR + 8*(n-1) there is `callx(4n) a(4n+3)`,
/// matching the call increment the firmware used to call the ROM function
/// (so the caller's frame is spilled with the layout it reserved space for).
pub const TRAMP_ADDR: u32 = 0x4007_FF00;
/// Return point of the first trampoline (others at +8, +16).
pub const TRAMP_RET: u32 = TRAMP_ADDR + 3;
/// Guest stub that loads target/args from a memory block and calls the target.
pub const STUB_ADDR: u32 = 0x4007_FF20;

pub fn is_tramp_ret(pc: u32) -> bool {
    (pc.wrapping_sub(TRAMP_RET) & !0x18) == 0 && pc <= TRAMP_RET + 16
}
/// Per-core argument blocks (inside the ROM's reserved DRAM area).
pub const ARGBLOCK: [u32; 2] = [0x3FFE_0000, 0x3FFE_0020];

/// Chain of nested call12 functions used to spill all register windows.
pub const SPILL_ADDR: u32 = 0x4007_FF60;

pub fn spill_code() -> Vec<u8> {
    // five functions 16 bytes apart: entry a1,48 ; call12 next ; retw.n  (last: entry ; retw.n)
    // A function that uses call12 needs a 48-byte frame: the window-overflow
    // handler stores its a4..a11 in the 32 bytes below its caller's base save area.
    let mut v = Vec::new();
    for k in 0..5 {
        let mut f = vec![0x36, 0x61, 0x00];
        if k < 4 {
            f.extend_from_slice(&[0xF5, 0x00, 0x00]); // call12 +16
        }
        f.extend_from_slice(&[0x1D, 0xF0]);
        f.resize(16, 0);
        v.extend_from_slice(&f);
    }
    v
}

/// Machine code placed at TRAMP_ADDR / STUB_ADDR.
pub fn trampoline_code() -> (Vec<u8>, Vec<u8>) {
    let tramp = vec![
        0xD0, 0x07, 0x00, 0, 0, 0, 0, 0, // callx4 a7
        0xE0, 0x0B, 0x00, 0, 0, 0, 0, 0, // callx8 a11
        0xF0, 0x0F, 0x00, // callx12 a15
    ];
    let stub = vec![
        0x36, 0x41, 0x00, // entry a1, 32
        0x82, 0x22, 0x00, // l32i a8, a2, 0
        0xA2, 0x22, 0x01, // l32i a10, a2, 4
        0xB2, 0x22, 0x02, // l32i a11, a2, 8
        0xC2, 0x22, 0x03, // l32i a12, a2, 12
        0xD2, 0x22, 0x04, // l32i a13, a2, 16
        0xE2, 0x22, 0x05, // l32i a14, a2, 20
        0xF2, 0x22, 0x06, // l32i a15, a2, 24
        0xE0, 0x08, 0x00, // callx8 a8
        0x2D, 0x0A, // mov.n a2, a10
        0x1D, 0xF0, // retw.n
    ];
    (tramp, stub)
}

#[derive(Debug, Clone)]
pub struct GuestCall {
    pub func: u32,
    pub args: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HleRet {
    pub ret: Option<u32>,
    pub ret_hi: Option<u32>,
}

/// Value passed when resuming a coroutine: the guest call's result plus fresh
/// pointers to the CPU and system (the machine may have moved in memory).
#[derive(Clone, Copy)]
pub struct ResumeIn {
    pub result: u32,
    pub cpu: *mut Cpu,
    pub sys: *mut Sys,
}

pub type HleCoroutine = Coroutine<ResumeIn, GuestCall, HleRet>;

/// Environment handed to coroutine HLE functions.
pub struct Env<'a> {
    cpu: std::cell::Cell<*mut Cpu>,
    sys: std::cell::Cell<*mut Sys>,
    y: &'a Yielder<ResumeIn, GuestCall>,
    pub args: [u32; 8],
    pub core: usize,
}

impl<'a> Env<'a> {
    #[allow(clippy::mut_from_ref)]
    pub fn sys(&self) -> &mut Sys {
        // SAFETY: the coroutine only runs while the machine is suspended inside
        // `resume`, so no other reference to Sys is active.
        unsafe { &mut *self.sys.get() }
    }
    #[allow(clippy::mut_from_ref)]
    pub fn cpu(&self) -> &mut Cpu {
        unsafe { &mut *self.cpu.get() }
    }
    /// Call a firmware function (windowed ABI, up to 6 args) and return a2.
    pub fn call(&self, func: u32, args: &[u32]) -> u32 {
        let r = self.y.suspend(GuestCall { func, args: args.to_vec() });
        self.cpu.set(r.cpu);
        self.sys.set(r.sys);
        r.result
    }
    /// Call newlib syscall-table entry `idx`.
    pub fn syscall(&self, idx: u32, args: &[u32]) -> u32 {
        let ptr_addr = if self.core == 0 { 0x3FFA_E024 } else { 0x3FFA_E020 };
        let mut table = self.sys().peek32(ptr_addr);
        if table == 0 {
            table = self.sys().peek32(0x3FFA_E024);
        }
        let f = self.sys().peek32(table + 4 * idx);
        if f == 0 {
            return u32::MAX;
        }
        self.call(f, args)
    }
    pub fn r8(&self, a: u32) -> u8 {
        self.sys().peek8(a)
    }
    pub fn r16(&self, a: u32) -> u16 {
        self.sys().peek8(a) as u16 | ((self.sys().peek8(a + 1) as u16) << 8)
    }
    pub fn r32(&self, a: u32) -> u32 {
        self.sys().peek32(a)
    }
    pub fn w8(&self, a: u32, v: u8) {
        self.sys().poke8(a, v)
    }
    pub fn w16(&self, a: u32, v: u16) {
        self.sys().poke8(a, v as u8);
        self.sys().poke8(a + 1, (v >> 8) as u8);
    }
    pub fn w32(&self, a: u32, v: u32) {
        self.sys().poke32(a, v)
    }
}

/// newlib syscall table indices (struct syscall_stub_table in ROM).
pub mod sc {
    pub const GETREENT: u32 = 0;
    pub const MALLOC_R: u32 = 1;
    pub const FREE_R: u32 = 2;
    pub const CLOSE_R: u32 = 19;
    pub const OPEN_R: u32 = 20;
    pub const WRITE_R: u32 = 21;
    pub const LSEEK_R: u32 = 22;
    pub const READ_R: u32 = 23;
    pub const LOCK_INIT_RECURSIVE: u32 = 25;
    pub const LOCK_ACQUIRE_RECURSIVE: u32 = 29;
    pub const LOCK_RELEASE_RECURSIVE: u32 = 33;
}

// newlib FILE layout (ESP-IDF configuration)
pub mod file {
    pub const P: u32 = 0;
    pub const R: u32 = 4;
    pub const W: u32 = 8;
    pub const FLAGS: u32 = 12;
    pub const FILE: u32 = 14;
    pub const BF_BASE: u32 = 16;
    pub const BF_SIZE: u32 = 20;
    pub const LBFSIZE: u32 = 24;
    pub const DATA: u32 = 28;
    pub const COOKIE: u32 = 32;
    pub const READ: u32 = 36;
    pub const WRITE: u32 = 40;
    pub const SEEK: u32 = 44;
    pub const CLOSE: u32 = 48;
    pub const OFFSET: u32 = 84;
    pub const LOCK: u32 = 88;
    pub const FLAGS2: u32 = 100;
    pub const SIZE: u32 = 104;
    pub const SOFF: u16 = 0x1000;
    pub const SLBF: u16 = 0x0001;
    pub const SNBF: u16 = 0x0002;
    pub const SWR: u16 = 0x0008;
    pub const SERR: u16 = 0x0040;
    pub const SAPP: u16 = 0x0100;
}

/// Names of ROM functions implemented as coroutines.
pub fn is_coroutine_fn(name: &str) -> bool {
    matches!(name, "qsort" | "setjmp" | "strdup" | "_strdup_r" | "strndup" | "_strndup_r" | "__sfp" | "__sread" | "__swrite" | "__sseek" | "__sclose" | "__sinit" | "_fwalk" | "_fwalk_reent" | "_fflush_r")
}

/// Registry of FILE objects allocated by the emulated `__sfp`.
pub static FILES: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

fn run_named(name: &str, env: &mut Env) -> HleRet {
    let a = env.args;
    match name {
        "qsort" => {
            let (base, n, size, compar) = (a[0], a[1], a[2], a[3]);
            if n >= 2 && size > 0 {
                let data = env.sys().read_bytes(base, (n * size) as usize);
                let mut idx: Vec<u32> = (0..n).collect();
                // merge sort keeps the number of comparator calls low
                idx.sort_by(|&x, &y| {
                    let r = env.call(compar, &[base + x * size, base + y * size]) as i32;
                    r.cmp(&0)
                });
                let mut out = Vec::with_capacity(data.len());
                for &k in &idx {
                    out.extend_from_slice(&data[(k * size) as usize..((k + 1) * size) as usize]);
                }
                env.sys().write_bytes(base, &out);
            }
            HleRet::default()
        }
        "__sfp" => {
            let d = a[0];
            // reuse a free slot if possible
            let free = {
                let files = FILES.lock().unwrap();
                files.iter().copied().find(|&f| env.r16(f + file::FLAGS) == 0)
            };
            let fp = match free {
                Some(f) => f,
                None => {
                    let f = env.syscall(sc::MALLOC_R, &[d, file::SIZE]);
                    if f == 0 || f == u32::MAX {
                        return HleRet { ret: Some(0), ret_hi: None };
                    }
                    FILES.lock().unwrap().push(f);
                    f
                }
            };
            // a reused slot keeps its lock (newlib only re-initialises it)
            let old_lock = if free.is_some() { env.r32(fp + file::LOCK) } else { 0 };
            for i in 0..file::SIZE {
                env.w8(fp + i, 0);
            }
            env.w16(fp + file::FILE, 0xFFFF);
            env.w16(fp + file::FLAGS, 1);
            env.w32(fp + file::DATA, d);
            env.w32(fp + file::LOCK, old_lock);
            // __lock_init_recursive(fp->_lock): the firmware's lock implementation
            // creates the mutex that stdio functions acquire and release
            env.syscall(sc::LOCK_INIT_RECURSIVE, &[fp + file::LOCK]);
            HleRet { ret: Some(fp), ret_hi: None }
        }
        "__sread" => {
            let (r, fp, buf, n) = (a[0], a[1], a[2], a[3]);
            let fd = env.r16(fp + file::FILE) as i16 as i32 as u32;
            let ret = env.syscall(sc::READ_R, &[r, fd, buf, n]);
            if (ret as i32) >= 0 {
                let off = env.r32(fp + file::OFFSET);
                env.w32(fp + file::OFFSET, off.wrapping_add(ret));
            } else {
                let fl = env.r16(fp + file::FLAGS);
                env.w16(fp + file::FLAGS, fl & !file::SOFF);
            }
            HleRet { ret: Some(ret), ret_hi: None }
        }
        "__swrite" => {
            let (r, fp, buf, n) = (a[0], a[1], a[2], a[3]);
            let fd = env.r16(fp + file::FILE) as i16 as i32 as u32;
            let fl = env.r16(fp + file::FLAGS);
            if fl & file::SAPP != 0 {
                env.syscall(sc::LSEEK_R, &[r, fd, 0, 2]);
            }
            env.w16(fp + file::FLAGS, fl & !file::SOFF);
            let ret = env.syscall(sc::WRITE_R, &[r, fd, buf, n]);
            HleRet { ret: Some(ret), ret_hi: None }
        }
        "__sseek" => {
            let (r, fp, off, whence) = (a[0], a[1], a[2], a[3]);
            let fd = env.r16(fp + file::FILE) as i16 as i32 as u32;
            let ret = env.syscall(sc::LSEEK_R, &[r, fd, off, whence]);
            let fl = env.r16(fp + file::FLAGS);
            if ret == u32::MAX {
                env.w16(fp + file::FLAGS, fl & !file::SOFF);
            } else {
                env.w16(fp + file::FLAGS, fl | file::SOFF);
                env.w32(fp + file::OFFSET, ret);
            }
            HleRet { ret: Some(ret), ret_hi: None }
        }
        "__sclose" => {
            let (r, fp) = (a[0], a[1]);
            let fd = env.r16(fp + file::FILE) as i16 as i32 as u32;
            let ret = env.syscall(sc::CLOSE_R, &[r, fd]);
            HleRet { ret: Some(ret), ret_hi: None }
        }
        "__sinit" => HleRet::default(),
        "_fflush_r" => {
            // write out a FILE's buffer through its _write hook (newlib __sflush_r)
            let (r, fp) = (a[0], a[1]);
            let files: Vec<u32> = if fp == 0 { FILES.lock().unwrap().clone() } else { vec![fp] };
            let mut ret = 0u32;
            for fp in files {
                let flags = env.r16(fp + file::FLAGS);
                if flags == 0 || flags & file::SWR == 0 {
                    continue;
                }
                let base = env.r32(fp + file::BF_BASE);
                if base == 0 {
                    continue;
                }
                let mut n = env.r32(fp + file::P).wrapping_sub(base) as i32;
                env.w32(fp + file::P, base);
                let w = if flags & (file::SLBF | file::SNBF) != 0 { 0 } else { env.r32(fp + file::BF_SIZE) };
                env.w32(fp + file::W, w);
                let (write, cookie) = (env.r32(fp + file::WRITE), env.r32(fp + file::COOKIE));
                let mut off = base;
                while n > 0 {
                    let t = env.call(write, &[r, cookie, off, n as u32]) as i32;
                    if t <= 0 {
                        env.w16(fp + file::FLAGS, flags | file::SERR);
                        ret = u32::MAX;
                        break;
                    }
                    off = off.wrapping_add(t as u32);
                    n -= t;
                }
            }
            HleRet { ret: Some(ret), ret_hi: None }
        }
        "strdup" | "_strdup_r" | "strndup" | "_strndup_r" => {
            let reent = name.starts_with('_');
            let src = if reent { a[1] } else { a[0] };
            let lim = if name.contains("ndup") { (if reent { a[2] } else { a[1] }) as usize } else { 1 << 20 };
            let mut v = env.sys().read_cstr(src, lim);
            v.push(0);
            let r = if reent { a[0] } else { env.syscall(sc::GETREENT, &[]) };
            let p = env.syscall(sc::MALLOC_R, &[r, v.len() as u32]);
            if p != 0 && p != u32::MAX {
                env.sys().write_bytes(p, &v);
                HleRet { ret: Some(p), ret_hi: None }
            } else {
                HleRet { ret: Some(0), ret_hi: None }
            }
        }
        "setjmp" => {
            // spill every live register window so the caller's frames are in memory
            let cpu = env.cpu();
            let sp = cpu.ar(1);
            let callinc = (cpu.sr[xtensa_core::cpu::PS] >> xtensa_core::cpu::PS_CALLINC_SHIFT) & 3;
            let ra = cpu.ar((callinc * 4) as u8);
            // snapshot the caller's live registers (a0..a(4n-1)) like newlib's setjmp
            let nregs = (callinc.max(1) * 4) as u8;
            let regs: Vec<u32> = (0..nregs).map(|i| cpu.ar(i)).collect();
            // spill older frames so that they live in memory when longjmp
            // invalidates the register windows
            env.call(SPILL_ADDR, &[]);
            let jb = a[0];
            env.w32(jb, 0x534A_4D50);
            env.w32(jb + 4, sp);
            env.w32(jb + 8, callinc);
            env.w32(jb + 12, ra);
            for (i, r) in regs.iter().enumerate() {
                env.w32(jb + 16 + 4 * i as u32, *r);
            }
            if std::env::var("DEBUG_SJLJ").is_ok() {
                eprintln!("setjmp jb={:08x} sp={:08x} n={} ra={:08x}", jb, sp, callinc, ra);
            }
            let _ = env.args;
            HleRet { ret: Some(0), ret_hi: None }
        }
        "_fwalk" | "_fwalk_reent" => {
            // int _fwalk_reent(struct _reent *ptr, int (*function)(struct _reent *, FILE *))
            let files = FILES.lock().unwrap().clone();
            let mut ret = 0u32;
            for f in files {
                if env.r16(f + file::FLAGS) > 1 {
                    ret |= if name == "_fwalk" { env.call(a[1], &[f]) } else { env.call(a[1], &[a[0], f]) };
                }
            }
            HleRet { ret: Some(ret), ret_hi: None }
        }
        _ => HleRet { ret: Some(0), ret_hi: None },
    }
}

/// Create a coroutine that emulates ROM function `name` for `cpu` (args are read now).
/// Resume it with a [`ResumeIn`] carrying the current CPU/system pointers.
pub fn start(name: &str, cpu: &mut Cpu, sys: &mut Sys) -> HleCoroutine {
    let mut args = [0u32; 8];
    for (i, a) in args.iter_mut().enumerate() {
        *a = cpu.hle_arg(sys, i as u32);
    }
    let core = cpu.core_id as usize;
    let name = name.to_string();
    Coroutine::new(move |y: &Yielder<ResumeIn, GuestCall>, first: ResumeIn| {
        let mut env = Env { cpu: std::cell::Cell::new(first.cpu), sys: std::cell::Cell::new(first.sys), y, args, core };
        run_named(&name, &mut env)
    })
}
