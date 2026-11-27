//! Flash layout handling: partition tables, firmware format detection, app
//! image parsing, OTA slot selection and ELF conversion.
use esp32_soc::elf::{build_app_image, elf_to_image, Symbols};
use esp32_soc::flash::*;

fn fixture_elf() -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../esp32sim/tests/fixtures/debug-demo/blink.elf");
    std::fs::read(p).expect("debug-demo fixture")
}

fn tiny_app() -> Vec<u8> {
    build_app_image(0x4008_0400, &[(0x4008_0400, vec![0x36, 0x41, 0x00, 0x06, 0xff, 0xff]), (0x3FFB_0000, vec![1, 2, 3, 4])])
}

#[test]
fn crc32_matches_the_standard_check_value() {
    assert_eq!(crc32_le(0, b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32_le(0, b""), 0);
    // chaining: crc of a split buffer equals crc of the whole
    let whole = crc32_le(0, b"hello world");
    assert_eq!(crc32_le(crc32_le(0, b"hello "), b"world"), whole);
}

#[test]
fn partition_table_roundtrip() {
    let parts = default_partitions();
    let bin = build_partition_table(&parts);
    assert_eq!(bin.len(), 0xC00);
    let back = parse_partitions(&bin);
    assert_eq!(back.len(), parts.len());
    for (a, b) in parts.iter().zip(back.iter()) {
        assert_eq!((a.label.as_str(), a.ptype, a.subtype, a.offset, a.size), (b.label.as_str(), b.ptype, b.subtype, b.offset, b.size));
    }
    // MD5 marker entry follows the partitions
    let md5_at = parts.len() * 32;
    assert_eq!(&bin[md5_at..md5_at + 2], &[0xEB, 0xEB]);
    assert!(format!("{}", back[2]).contains("app0"));
}

#[test]
fn default_layout_fits_4mb_without_overlaps() {
    let mut parts = default_partitions();
    parts.sort_by_key(|p| p.offset);
    for w in parts.windows(2) {
        assert!(w[0].offset + w[0].size <= w[1].offset, "{} overlaps {}", w[0].label, w[1].label);
    }
    let last = parts.last().unwrap();
    assert!(last.offset + last.size <= 4 << 20);
    assert!(parts.iter().all(|p| p.offset % 0x1000 == 0));
    assert!(parts.iter().filter(|p| p.ptype == 0).all(|p| p.offset % 0x10000 == 0), "apps are 64K aligned");
}

#[test]
fn firmware_kinds_are_detected() {
    assert_eq!(detect_kind(&fixture_elf()).unwrap(), FirmwareKind::Elf);
    assert_eq!(detect_kind(&tiny_app()).unwrap(), FirmwareKind::AppImage);
    let mut merged = vec![0xFFu8; 0x20000];
    merged[0x1000] = 0xE9;
    merged[0x8000] = 0xAA;
    merged[0x8001] = 0x50;
    assert_eq!(detect_kind(&merged).unwrap(), FirmwareKind::MergedFrom0);
    let from1000 = merged[0x1000..].to_vec();
    assert_eq!(detect_kind(&from1000).unwrap(), FirmwareKind::MergedFrom0x1000);
    assert!(detect_kind(b"not firmware").is_err());
    assert!(detect_kind(&[]).is_err());
}

#[test]
fn app_image_is_placed_in_the_first_app_partition() {
    let flash = build_flash(&tiny_app(), 4 << 20, None, None).unwrap();
    assert_eq!(flash.size(), 4 << 20);
    assert_eq!(flash.partitions().len(), default_partitions().len());
    let boot = flash.boot_partition().unwrap();
    assert_eq!(boot.label, "app0");
    assert_eq!(flash.byte(boot.offset), 0xE9);
    let img = parse_app_image(&flash, boot.offset).unwrap();
    assert_eq!(img.entry, 0x4008_0400);
    assert_eq!(img.segments.len(), 2);
    assert_eq!(img.segments[1].load_addr, 0x3FFB_0000);
    assert_eq!(img.segments[1].len, 4);
    assert!(!flash.dirty);
}

#[test]
fn image_checksum_and_padding() {
    let img = tiny_app();
    assert_eq!(img.len() % 16, 0, "padded to 16 bytes");
    let mut ck = 0xEFu8;
    let mut p = 24;
    for _ in 0..img[1] {
        let len = u32::from_le_bytes(img[p + 4..p + 8].try_into().unwrap()) as usize;
        img[p + 8..p + 8 + len].iter().for_each(|b| ck ^= b);
        p += 8 + len;
    }
    assert_eq!(*img.last().unwrap(), ck);
}

#[test]
fn flash_mapped_segments_keep_64k_alignment() {
    let img = build_app_image(
        0x4008_0000,
        &[(0x3F40_0020, vec![7; 100]), (0x400D_0020, vec![9; 64]), (0x4008_0000, vec![1; 8])],
    );
    let flash = build_flash(&img, 4 << 20, None, None).unwrap();
    let app = parse_app_image(&flash, 0x10000).unwrap();
    for s in app.segments.iter().filter(|s| s.load_addr >= 0x3F40_0000 && s.load_addr < 0x3F80_0000 || s.load_addr >= 0x400C_2000) {
        assert_eq!(s.flash_off % 0x10000, s.load_addr % 0x10000, "segment 0x{:08x}", s.load_addr);
    }
}

#[test]
fn app_descriptor_is_read() {
    let mut desc = vec![0u8; 256];
    desc[0..4].copy_from_slice(&0xABCD_5432u32.to_le_bytes());
    desc[16..21].copy_from_slice(b"1.2.3");
    desc[48..55].copy_from_slice(b"my-proj");
    desc[112..118].copy_from_slice(b"v5.1.2");
    let img = build_app_image(0x4008_0000, &[(0x3F40_0020, desc), (0x4008_0000, vec![0; 4])]);
    let flash = build_flash(&img, 4 << 20, None, None).unwrap();
    let app = parse_app_image(&flash, 0x10000).unwrap();
    assert_eq!(app.project_name, "my-proj");
    assert_eq!(app.version, "1.2.3");
    assert_eq!(app.idf_version, "v5.1.2");
}

#[test]
fn otadata_selects_the_ota_slot() {
    let mut flash = build_flash(&tiny_app(), 4 << 20, None, None).unwrap();
    assert_eq!(flash.boot_partition().unwrap().label, "app0");
    // otadata entry: seq 2 -> slot (2-1) % 2 = app1
    let otadata = flash.partitions().into_iter().find(|p| p.ptype == 1 && p.subtype == 0).unwrap();
    let seq = 2u32.to_le_bytes();
    flash.write_at(otadata.offset, &seq);
    flash.write_at(otadata.offset + 28, &crc32_le(0xFFFF_FFFF, &seq).to_le_bytes());
    assert_eq!(flash.boot_partition().unwrap().label, "app1");
    // a corrupt CRC is ignored
    flash.write_at(otadata.offset + 28, &0u32.to_le_bytes());
    assert_eq!(flash.boot_partition().unwrap().label, "app0");
}

#[test]
fn merged_image_is_written_verbatim() {
    let mut merged = vec![0xFFu8; 0x20000];
    merged[0x1000] = 0xE9;
    merged[0x8000..0x8C00].copy_from_slice(&build_partition_table(&default_partitions()));
    let app = tiny_app();
    merged[0x10000..0x10000 + app.len()].copy_from_slice(&app);
    let flash = build_flash(&merged, 4 << 20, None, None).unwrap();
    assert_eq!(&flash.data[..merged.len()], &merged[..]);
    assert_eq!(parse_app_image(&flash, 0x10000).unwrap().entry, 0x4008_0400);
}

#[test]
fn custom_partition_table_is_used() {
    let parts = vec![Partition { label: "factory".into(), ptype: 0, subtype: 0, offset: 0x20000, size: 0x100000, flags: 0 }];
    let pt = build_partition_table(&parts);
    let flash = build_flash(&tiny_app(), 4 << 20, Some(&pt), None).unwrap();
    let boot = flash.boot_partition().unwrap();
    assert_eq!((boot.label.as_str(), boot.offset), ("factory", 0x20000));
    assert_eq!(flash.byte(0x20000), 0xE9);
}

#[test]
fn nor_flash_program_and_erase_semantics() {
    let mut f = Flash::new(0x2000);
    assert_eq!(f.byte(0), 0xFF);
    f.program(0, &[0x0F, 0xF0]);
    f.program(0, &[0xF3, 0xFF]); // can only clear bits
    assert_eq!((f.byte(0), f.byte(1)), (0x03, 0xF0));
    assert!(f.dirty);
    f.erase(0, 0x1000);
    assert_eq!((f.byte(0), f.byte(1)), (0xFF, 0xFF));
    // out-of-range reads return erased bytes, writes are clipped
    assert_eq!(f.u32_at(0x1FFE), 0xFFFF_FFFF);
    f.write_at(0x1FFF, &[1, 2, 3]);
    assert_eq!(f.byte(0x1FFF), 1);
}

#[test]
fn elf_is_converted_to_a_bootable_image() {
    let elf = fixture_elf();
    let img = elf_to_image(&elf).unwrap();
    assert_eq!(img[0], 0xE9);
    let flash = build_flash(&elf, 4 << 20, None, None).unwrap();
    let app = parse_app_image(&flash, flash.boot_partition().unwrap().offset).unwrap();
    let sym = Symbols::from_elf(&elf).unwrap();
    assert_eq!(Some(app.entry), sym.by_name.get("app_main").copied().or(Some(app.entry)));
    assert!(app.segments.iter().any(|s| (0x4008_0000..0x400A_0000).contains(&s.load_addr)));
    assert!(elf_to_image(b"\x7fELF garbage").is_err());
}

#[test]
fn symbols_lookup_and_describe() {
    let sym = Symbols::from_elf(&fixture_elf()).unwrap();
    let main = *sym.by_name.get("app_main").expect("app_main symbol");
    let (name, off) = sym.lookup(main + 3).unwrap();
    assert_eq!((name.as_str(), off), ("app_main", 3));
    assert!(sym.describe(main).contains("app_main"));
    let step = *sym.by_name.get("step").unwrap();
    assert_eq!(sym.lookup(step).unwrap().0, "step");
}
