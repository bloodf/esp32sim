// Raw interpreter speed: a counting loop in RAM, no peripherals.
use xtensa_core::{Bus, Cpu, MemFault, StepResult};
struct Ram(Vec<u8>);
impl Bus for Ram {
    fn read(&mut self, a: u32, size: u8, _c: u64) -> Result<u32, MemFault> {
        let o = (a - 0x4008_0000) as usize;
        let m = &self.0;
        Ok(match size { 1 => m[o] as u32, 2 => u16::from_le_bytes([m[o], m[o + 1]]) as u32, _ => u32::from_le_bytes([m[o], m[o + 1], m[o + 2], m[o + 3]]) })
    }
    fn write(&mut self, a: u32, size: u8, v: u32, _c: u64) -> Result<(), MemFault> {
        let o = (a - 0x4008_0000) as usize;
        match size { 1 => self.0[o] = v as u8, 2 => self.0[o..o + 2].copy_from_slice(&(v as u16).to_le_bytes()), _ => self.0[o..o + 4].copy_from_slice(&v.to_le_bytes()) }
        Ok(())
    }
    fn fetch(&mut self, a: u32) -> Result<u32, MemFault> {
        let o = (a - 0x4008_0000) as usize;
        Ok(u32::from_le_bytes([self.0[o], self.0[o + 1], self.0[o + 2], self.0[o + 3]]))
    }
}
fn main() {
    let mut ram = Ram(vec![0; 0x10000]);
    // loop:  addi a2, a2, 1 ; l32i a3, a1, 0 ; add a3, a3, a2 ; s32i a3, a1, 0 ; bnez a4? -> j loop
    let prog: Vec<u8> = vec![
        0x22, 0xC2, 0x01, // addi a2, a2, 1
        0x32, 0x21, 0x00, // l32i a3, a1, 0
        0x2A, 0x33,       // add.n a3, a3, a2
        0x32, 0x61, 0x00, // s32i a3, a1, 0
        0x06, 0xFD, 0xFF, // j -> start (offset -12? computed below)
    ];
    ram.0[..prog.len()].copy_from_slice(&prog);
    // fix J: target = pc + 4 + off ; pc = 11 -> want 0 => off = -15
    let off: i32 = -15;
    let w = ((off as u32 & 0x3FFFF) << 6) | 0x06;
    ram.0[11] = w as u8; ram.0[12] = (w >> 8) as u8; ram.0[13] = (w >> 16) as u8;
    let mut cpu = Cpu::new(0);
    cpu.pc = 0x4008_0000;
    cpu.sr[230] = 0x00040000; // PS: WOE, intlevel 0
    cpu.set_ar(1, 0x4008_8000);
    let t = std::time::Instant::now();
    let n = 300_000_000u64;
    for _ in 0..n {
        if cpu.step(&mut ram) != StepResult::Ok { panic!("stop at {:x}", cpu.pc); }
    }
    let el = t.elapsed().as_secs_f64();
    println!("{} instructions in {:.2}s = {:.1} MIPS (a2={})", n, el, n as f64 / el / 1e6, cpu.ar(2));
}
