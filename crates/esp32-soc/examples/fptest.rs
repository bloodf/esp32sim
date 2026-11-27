// Verifies the FPU division/sqrt helper instructions by running the firmware's own
// __divsf3 / sqrtf code on the emulated CPU and comparing with IEEE results.
use esp32_soc::flash::build_flash;
use esp32_soc::{Machine, MachineConfig};
use xtensa_core::cpu::{PS, PS_WOE, CPENABLE};
use xtensa_core::StepResult;

fn call(m: &mut Machine, f: u32, a: u32, b: u32) -> u32 {
    let c = &mut m.cpus[0];
    c.sr[72] = 0; c.sr[73] = 1;
    c.sr[PS] = PS_WOE | (1 << 16);
    c.sr[CPENABLE] = 1;
    c.set_ar(1, 0x3FFE_3000);
    c.set_ar(4, (1 << 30) | 0x0000_0010); // return to 0x40000010 (HLE region)
    c.set_ar(5, 0x3FFE_3000);
    c.set_ar(6, a);
    c.set_ar(7, b);
    c.pc = f;
    for _ in 0..1000 {
        match c.step(&mut m.sys) {
            StepResult::Hle => return c.ar(6),
            StepResult::Ok => {}
            r => panic!("unexpected {:?}", r),
        }
    }
    panic!("did not return");
}

fn main() {
    let fw = std::fs::read("firmware-tests/ESP32_GENERIC-20240602-v1.23.0.bin").unwrap();
    let flash = build_flash(&fw, 4 << 20, None, None).unwrap();
    let mut m = Machine::new(flash, MachineConfig::default());
    m.boot().unwrap();
    let (div, sqrt) = (0x401fbd70u32, 0x401fbfccu32);
    let mut rng = 0x1234567u64;
    let mut next = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng as u32 };
    let specials = [0.0f32, -0.0, 1.0, -1.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, f32::MIN_POSITIVE, 1e-40, -3e-39, f32::MAX, 3.0, 0.1, 7.5, 1e30, 1e-30];
    let (mut bad_div, mut bad_sqrt, mut n) = (0, 0, 0);
    let same = |x: f32, y: f32| (x.is_nan() && y.is_nan()) || x.to_bits() == y.to_bits();
    for i in 0..200000 {
        let (a, b) = if i < specials.len() * specials.len() {
            (specials[i / specials.len()], specials[i % specials.len()])
        } else if i % 3 == 0 {
            (f32::from_bits(next()), f32::from_bits(next()))
        } else {
            ((next() as f32 / 1e6) - 2000.0, (next() as f32 / 1e7) - 200.0)
        };
        n += 1;
        let r = f32::from_bits(call(&mut m, div, a.to_bits(), b.to_bits()));
        if !same(r, a / b) { if bad_div < 10 { println!("div {:e} / {:e} = {:e} expected {:e}", a, b, r, a / b); } bad_div += 1; }
        let s = f32::from_bits(call(&mut m, sqrt, a.to_bits(), 0));
        if !same(s, a.sqrt()) { if bad_sqrt < 10 { println!("sqrt {:e} = {:e} expected {:e}", a, s, a.sqrt()); } bad_sqrt += 1; }
    }
    println!("{} cases: {} division mismatches, {} sqrt mismatches", n, bad_div, bad_sqrt);
}
