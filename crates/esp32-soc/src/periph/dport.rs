//! DPORT: system registers, APP CPU control, cache control, interrupt matrix.

use super::{src, Irq, RegFile};

pub const APPCPU_CTRL_A: u32 = 0x2C;
pub const APPCPU_CTRL_B: u32 = 0x30;
pub const APPCPU_CTRL_C: u32 = 0x34;
pub const APPCPU_CTRL_D: u32 = 0x38;
pub const CPU_PER_CONF: u32 = 0x3C;
pub const PRO_CACHE_CTRL: u32 = 0x40;
pub const APP_CACHE_CTRL: u32 = 0x58;
pub const FROM_CPU_0: u32 = 0xDC;
pub const INTR_STATUS_PRO: u32 = 0xEC;
pub const INTR_STATUS_APP: u32 = 0xF8;
pub const INTR_MAP_PRO: u32 = 0x104;
pub const INTR_MAP_APP: u32 = 0x218;
pub const DATE: u32 = 0xFFC;

pub struct Dport {
    pub regs: RegFile,
    pub appcpu_boot_addr: u32,
    /// set by the ROM HLE once the app CPU may start
    pub appcpu_released: bool,
    /// MMU tables: [cpu][entry]
    pub mmu: [Vec<u32>; 2],
    pub mmu_dirty: bool,
}

impl Dport {
    pub fn new() -> Dport {
        let mut regs = RegFile::default();
        regs.write(APPCPU_CTRL_A, 1);
        regs.write(PRO_CACHE_CTRL, 1 << 5); // flush done
        regs.write(APP_CACHE_CTRL, 1 << 5);
        regs.write(DATE, 0x1605190);
        Dport {
            regs,
            appcpu_boot_addr: 0,
            appcpu_released: false,
            mmu: [vec![0x100; 2048], vec![0x100; 2048]],
            mmu_dirty: true,
        }
    }

    /// Whether DPORT lets the APP CPU run (clock on, not in reset, not stalled).
    pub fn appcpu_running(&self) -> bool {
        self.regs.read(APPCPU_CTRL_B) & 1 != 0 && self.regs.read(APPCPU_CTRL_A) & 1 == 0 && self.regs.read(APPCPU_CTRL_C) & 1 == 0
    }

    pub fn cpu_period_sel(&self) -> u32 {
        self.regs.read(CPU_PER_CONF) & 3
    }

    pub fn read(&mut self, off: u32, irq: &Irq) -> u32 {
        match off {
            INTR_STATUS_PRO | INTR_STATUS_APP => (irq.sources & 0xFFFF_FFFF) as u32,
            o if o == INTR_STATUS_PRO + 4 || o == INTR_STATUS_APP + 4 => ((irq.sources >> 32) & 0xFFFF_FFFF) as u32,
            o if o == INTR_STATUS_PRO + 8 || o == INTR_STATUS_APP + 8 => ((irq.sources >> 64) & 0x1F) as u32,
            o if (INTR_MAP_PRO..INTR_MAP_PRO + 4 * src::NUM as u32).contains(&o) => irq.map[0][((o - INTR_MAP_PRO) / 4) as usize] as u32,
            o if (INTR_MAP_APP..INTR_MAP_APP + 4 * src::NUM as u32).contains(&o) => irq.map[1][((o - INTR_MAP_APP) / 4) as usize] as u32,
            PRO_CACHE_CTRL | APP_CACHE_CTRL => self.regs.read(off) | (1 << 5),
            // PRO/APP_DCACHE_DBUG0: cache state = 1 (idle)
            0x3F0 | 0x418 => 1 << 7,
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, val: u32, irq: &mut Irq) {
        match off {
            o if (FROM_CPU_0..FROM_CPU_0 + 16).contains(&o) => {
                let n = ((o - FROM_CPU_0) / 4) as usize;
                irq.set(src::FROM_CPU_0 + n, val & 1 != 0);
                self.regs.write(off, val & 1);
            }
            o if (INTR_MAP_PRO..INTR_MAP_PRO + 4 * src::NUM as u32).contains(&o) => {
                irq.map[0][((o - INTR_MAP_PRO) / 4) as usize] = (val & 0x1F) as u8;
                irq.dirty = true;
            }
            o if (INTR_MAP_APP..INTR_MAP_APP + 4 * src::NUM as u32).contains(&o) => {
                irq.map[1][((o - INTR_MAP_APP) / 4) as usize] = (val & 0x1F) as u8;
                irq.dirty = true;
            }
            _ => self.regs.write(off, val),
        }
    }

    /// Access to the MMU table region (0x3FF10000..0x3FF14000).
    pub fn mmu_read(&self, off: u32) -> u32 {
        let cpu = ((off >> 13) & 1) as usize;
        let idx = ((off & 0x1FFF) >> 2) as usize;
        self.mmu[cpu][idx]
    }
    pub fn mmu_write(&mut self, off: u32, val: u32) {
        let cpu = ((off >> 13) & 1) as usize;
        let idx = ((off & 0x1FFF) >> 2) as usize;
        if self.mmu[cpu][idx] != val {
            self.mmu[cpu][idx] = val;
            self.mmu_dirty = true;
        }
    }

    /// Translate a flash-cache virtual address for `cpu` into a flash offset.
    #[inline]
    pub fn translate(&self, cpu: usize, vaddr: u32) -> Option<u32> {
        let idx = if (0x3F40_0000..0x3F80_0000).contains(&vaddr) {
            (vaddr - 0x3F40_0000) >> 16
        } else if (0x4000_0000..0x40C0_0000).contains(&vaddr) {
            64 + ((vaddr - 0x4000_0000) >> 16)
        } else {
            return None;
        };
        let e = self.mmu[cpu][idx as usize];
        if e & 0x100 != 0 {
            return None;
        }
        Some(((e & 0xFF) << 16) | (vaddr & 0xFFFF))
    }
}
