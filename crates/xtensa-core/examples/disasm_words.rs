// Reads hex words (one per line, little-endian 24-bit) and prints disassembly.
use std::io::BufRead;
fn main() {
    for line in std::io::stdin().lock().lines() {
        let l = line.unwrap();
        let w = u32::from_str_radix(l.trim(), 16).unwrap();
        let (s, len) = xtensa_core::disasm(0x1000, w);
        println!("{}\t{}\t{}", l.trim(), len, s);
    }
}
