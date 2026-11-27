//! Whole-chip tests: tiny firmwares (assembled in the test, no toolchain
//! needed) boot on the emulated ESP32 and talk to a mock circuit that
//! records every pin / bus event.
mod common;
use common::*;
use esp32_soc::RunState;

#[test]
fn boots_app_image_and_prints_on_uart0() {
    let (mut m, log) = boot(
        "li a2, UART0
         movi a3, 'H'
         s32i a3, a2, 0
         movi a3, 'i'
         s32i a3, a2, 0
         movi a3, 10
         s32i a3, a2, 0",
    );
    assert_eq!(m.run_for(ms(2)), RunState::Running);
    assert_eq!(log.lock().unwrap().uart0(), "Hi\n");
    assert!(m.messages.iter().any(|s| s.contains("boot: app")), "{:?}", m.messages);
    assert!(m.total_instructions > 0);
    assert!(m.now_ns() >= ms(2));
}

#[test]
fn uart_receive_and_echo_uppercase() {
    let (mut m, log) = boot_with(
        "       li a2, UART0
         poll:  l32i a3, a2, 0x1c        ; STATUS: rx count in bits 0..7
                extui a3, a3, 0, 8
                beqz a3, poll
                l32i a4, a2, 0           ; pop a byte
                addi a4, a4, -32         ; to upper case
                s32i a4, a2, 0
                j poll",
        |io| io.uart_in[0].extend(b"esp"),
    );
    m.run_for(ms(20));
    assert_eq!(log.lock().unwrap().uart0(), "ESP");
}

#[test]
fn gpio_outputs_reach_the_circuit() {
    let (mut m, log) = boot(
        "li a2, GPIO
         movi a3, 0x24             ; GPIO2 and GPIO5
         s32i a3, a2, 0x24         ; ENABLE_W1TS
         s32i a3, a2, 0x08         ; OUT_W1TS
         movi a4, 4
         s32i a4, a2, 0x0c         ; OUT_W1TC: GPIO2 low again
         movi a5, 2                ; GPIO33 = bit 1 of the high bank
         s32i a5, a2, 0x30         ; ENABLE1_W1TS
         s32i a5, a2, 0x14         ; OUT1_W1TS",
    );
    m.run_for(ms(1));
    let d = log.lock().unwrap().last_drive();
    assert!(d.is_output(2) && d.is_output(5) && d.is_output(33));
    assert!(!d.level(2));
    assert!(d.level(5));
    assert!(d.level(33));
    assert!(!d.is_output(4));
}

#[test]
fn gpio_inputs_are_sampled_from_the_circuit() {
    let (mut m, _log) = boot_with(
        "li a2, GPIO
         li a5, RESULTS
         l32i a3, a2, 0x3c         ; GPIO_IN
         s32i a3, a5, 0
         l32i a4, a2, 0x40         ; GPIO_IN1
         s32i a4, a5, 4",
        |io| {
            io.inputs.insert(4, true);
            io.inputs.insert(15, false);
            io.inputs.insert(34, true);
        },
    );
    m.run_for(ms(1));
    let in0 = m.sys.peek32(RESULTS);
    let in1 = m.sys.peek32(RESULTS + 4);
    assert_ne!(in0 & (1 << 4), 0, "GPIO4 high: {:08x}", in0);
    assert_eq!(in0 & (1 << 15), 0, "GPIO15 low");
    assert_ne!(in1 & (1 << 2), 0, "GPIO34 high: {:08x}", in1);
}

/// Program I2C0 command list + FIFO, start, and wait for completion.
const I2C_WRITE: &str = "
        li a2, 0x3ff53000          ; I2C0
        movi a3, 0x113             ; CTR: ms_mode | sda/scl force out | clk_en
        s32i a3, a2, 0x04
        movi a3, 0x78              ; address 0x3c, write
        s32i a3, a2, 0x1c
        movi a3, 0x00
        s32i a3, a2, 0x1c
        movi a3, 0xaf
        s32i a3, a2, 0x1c
        movi a3, 0                 ; RSTART
        s32i a3, a2, 0x58
        li a3, 0x0903              ; WRITE 3 bytes, ack check
        s32i a3, a2, 0x5c
        li a3, 0x1800              ; STOP
        s32i a3, a2, 0x60
        movi a3, 0x133             ; CTR | trans_start
        s32i a3, a2, 0x04
 i2c_w: l32i a4, a2, 0x20          ; INT_RAW
        li a6, 0x480               ; TRANS_COMPLETE | ACK_ERR
        bnone a4, a6, i2c_w
        li a5, RESULTS
        s32i a4, a5, 0
";

#[test]
fn i2c_master_write_transaction() {
    let (mut m, log) = boot_with(I2C_WRITE, |io| {
        io.i2c_devices.insert(0x3C);
    });
    m.run_for(ms(5));
    let l = log.lock().unwrap();
    assert_eq!(l.i2c, vec!["S 3cw", "W 00", "W af", "P"]);
    assert_eq!(m.sys.peek32(RESULTS) & (1 << 10), 0, "no ACK error");
}

#[test]
fn i2c_missing_device_reports_ack_error() {
    let (mut m, log) = boot(I2C_WRITE);
    m.run_for(ms(5));
    assert_eq!(log.lock().unwrap().i2c.first().map(|s| s.as_str()), Some("S 3cw"));
    assert_ne!(m.sys.peek32(RESULTS) & (1 << 10), 0, "ACK error raised: {:08x}", m.sys.peek32(RESULTS));
}

#[test]
fn i2c_master_read_transaction() {
    let (mut m, log) = boot_with(
        "       li a2, 0x3ff53000
                movi a3, 0x113
                s32i a3, a2, 0x04
                movi a3, 0x91              ; address 0x48, read
                s32i a3, a2, 0x1c
                movi a3, 0                 ; RSTART
                s32i a3, a2, 0x58
                li a3, 0x0901              ; WRITE 1 byte (address), ack check
                s32i a3, a2, 0x5c
                li a3, 0x1001              ; READ 1 byte, ACK
                s32i a3, a2, 0x60
                li a3, 0x1401              ; READ 1 byte, NACK
                s32i a3, a2, 0x64
                li a3, 0x1800              ; STOP
                s32i a3, a2, 0x68
                movi a3, 0x133
                s32i a3, a2, 0x04
         wait:  l32i a4, a2, 0x20
                bbci a4, 7, wait
                li a5, RESULTS
                l32i a6, a2, 0x1c          ; FIFO data
                s32i a6, a5, 0
                l32i a6, a2, 0x1c
                s32i a6, a5, 4",
        |io| {
            io.i2c_devices.insert(0x48);
            io.i2c_read_data.extend([0x12, 0x34]);
        },
    );
    m.run_for(ms(5));
    assert_eq!(log.lock().unwrap().i2c, vec!["S 48r", "R 12", "R 34 nack", "P"]);
    assert_eq!(m.sys.peek32(RESULTS) & 0xFF, 0x12);
    assert_eq!(m.sys.peek32(RESULTS + 4) & 0xFF, 0x34);
}

#[test]
fn spi_full_duplex_transfer() {
    let (mut m, log) = boot_with(
        "       li a2, 0x3ff64000          ; SPI2 (HSPI)
                li a3, 0xddccbbaa
                s32i a3, a2, 0x80          ; W0
                movi a3, 31
                s32i a3, a2, 0x28          ; MOSI_DLEN = 32 bits
                s32i a3, a2, 0x2c          ; MISO_DLEN
                li a3, 0x18000001          ; USER: usr_mosi | usr_miso | doutdin
                s32i a3, a2, 0x1c
                li a3, 0x40000             ; CMD: usr
                s32i a3, a2, 0x00
         busy:  l32i a4, a2, 0x00
                bbsi a4, 18, busy
                l32i a4, a2, 0x80
                li a5, RESULTS
                s32i a4, a5, 0",
        |io| io.spi_reply.extend([1, 2, 3, 4]),
    );
    m.run_for(ms(2));
    let l = log.lock().unwrap();
    assert_eq!(l.spi.len(), 1, "one transfer");
    assert_eq!(l.spi[0].2, vec![0xAA, 0xBB, 0xCC, 0xDD]);
    assert_eq!(m.sys.peek32(RESULTS), 0x0403_0201);
}

#[test]
fn ledc_pwm_is_reported_with_frequency_and_duty() {
    let (mut m, log) = boot(
        "       li a2, GPIO
                movi a3, 71                ; LEDC_HS_SIG_OUT0 -> GPIO18
                addmi a7, a2, 0x500
                s32i a3, a7, 0x78          ; FUNC18_OUT_SEL_CFG (0x530 + 18*4)
                li a3, 0x40000
                s32i a3, a2, 0x24          ; enable GPIO18
                li a2, 0x3ff59000          ; LEDC
                li a3, (1 << 25) | (20000 << 5) | 10   ; HSTIMER0: APB clock, div 20000/256, 10-bit
                s32i a3, a2, 0x140
                movi a3, 4                 ; HSCH0_CONF0: timer 0, sig_out_en
                s32i a3, a2, 0x00
                movi a3, 0
                s32i a3, a2, 0x04          ; HPOINT
                li a3, 512 << 4            ; duty 50 %
                s32i a3, a2, 0x08
                li a3, 0x80000000          ; CONF1: duty_start
                s32i a3, a2, 0x0c",
    );
    m.run_for(ms(2));
    let apb = m.sys.apb_hz() as f64;
    let l = log.lock().unwrap();
    let (_, pwm) = l.pwm.iter().rev().find(|(p, _)| *p == 18).expect("PWM on GPIO18");
    let want = apb / (20000.0 / 256.0) / 1024.0;
    assert!((pwm.freq_hz - want).abs() / want < 0.01, "freq {} want {}", pwm.freq_hz, want);
    assert!((pwm.duty - 0.5).abs() < 0.01, "duty {}", pwm.duty);
    assert!(pwm.running);
}

#[test]
fn timer_group_counts_with_emulated_time() {
    let (mut m, _log) = boot(
        "       li a2, 0x3ff5f000          ; TIMG0
                li a3, 0xc0000000 | (80 << 13)   ; enable | increase | divider 80
                s32i a3, a2, 0x00
                li a4, 20000
         spin:  addi a4, a4, -1
                bnez a4, spin
                s32i a4, a2, 0x0c          ; latch
                l32i a5, a2, 0x04
                li a6, RESULTS
                s32i a5, a6, 0
                rsr a7, ccount
                s32i a7, a6, 4",
    );
    m.run_for(ms(5));
    let count = m.sys.peek32(RESULTS) as f64;
    let cycles = m.sys.peek32(RESULTS + 4) as f64;
    let elapsed_s = cycles / m.sys.cpu_hz as f64;
    let want = elapsed_s * m.sys.apb_hz() as f64 / 80.0;
    assert!(count > 0.0);
    assert!((count - want).abs() <= want * 0.1 + 2.0, "timer {} expected about {}", count, want);
}

#[test]
fn efuse_holds_the_configured_mac() {
    let (mut m, _log) = boot(
        "li a2, 0x3ff5a000
         li a3, RESULTS
         l32i a4, a2, 0x04
         s32i a4, a3, 0
         l32i a4, a2, 0x08
         s32i a4, a3, 4",
    );
    m.run_for(ms(1));
    let mac = m.config.mac;
    let w1 = m.sys.peek32(RESULTS);
    let w2 = m.sys.peek32(RESULTS + 4);
    assert_eq!(w1, u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]));
    assert_eq!(w2 & 0xFFFF, (mac[0] as u32) << 8 | mac[1] as u32);
}

#[test]
fn rom_printf_is_emulated() {
    let (mut m, log) = boot(
        "       j over
                .align 4
         fmt:   .asciz \"v=%d %s!\\n\"
         word:  .asciz \"ok\"
                .align 4
         over:  la a6, fmt
                movi a7, 42
                la a8, word
                li a9, ets_printf
                callx4 a9",
    );
    m.run_for(ms(2));
    assert_eq!(log.lock().unwrap().uart0(), "v=42 ok!\n");
}

#[test]
fn app_cpu_starts_when_released() {
    let (mut m, _log) = boot(
        "       la a6, app_main
                li a9, ets_set_appcpu_boot_addr
                callx4 a9
                li a2, 0x3ff00000          ; DPORT
                movi a3, 1
                s32i a3, a2, 0x30          ; APPCPU_CTRL_B: clock on
                movi a3, 0
                s32i a3, a2, 0x34          ; CTRL_C: not stalled
                s32i a3, a2, 0x2c          ; CTRL_A: out of reset
                j _halt
                .align 4
         app_main:
                entry a1, 32
                li a2, RESULTS
                movi a3, 0x55
                s32i a3, a2, 16
                rsr a4, prid
                s32i a4, a2, 20
         app_idle: j app_idle",
    );
    m.run_for(ms(2));
    assert!(m.messages.iter().any(|s| s.contains("APP CPU started")), "{:?}", m.messages);
    assert_eq!(m.sys.peek32(RESULTS + 16), 0x55);
    assert_eq!(m.sys.peek32(RESULTS + 20), 0xABAB, "ran on core 1");
}

#[test]
fn software_reset_reboots_with_reason() {
    let (mut m, log) = boot(
        "movi a6, 0
         li a9, rtc_get_reset_reason
         callx4 a9
         li a2, UART0
         addi a3, a6, '0'
         s32i a3, a2, 0",
    );
    m.run_for(ms(2));
    m.reset(12).unwrap();
    m.run_for(ms(2));
    assert_eq!(m.resets, 1);
    let out = log.lock().unwrap().uart0();
    assert_eq!(out.len(), 2, "{:?}", out);
    assert!(out.ends_with((b'0' + 12) as char), "{:?}", out);
}

#[test]
fn deep_sleep_wakes_on_timer_and_keeps_rtc_memory() {
    let (mut m, log) = boot(
        "       li a2, 0x50000000          ; RTC slow memory: boot counter
                l32i a3, a2, 0
                addi a3, a3, 1
                s32i a3, a2, 0
                li a4, UART0
                addi a5, a3, '0'
                s32i a5, a4, 0
                bgei a3, 2, woke
                li a2, 0x3ff48000          ; RTC_CNTL
                li a3, 0x80000000
                s32i a3, a2, 0x0c          ; TIME_UPDATE
                l32i a4, a2, 0x10          ; TIME0 (slow clock ticks)
                li a5, 15000               ; ~0.1 s
                add a4, a4, a5
                s32i a4, a2, 0x04          ; SLP_TIMER0
                movi a4, 0
                s32i a4, a2, 0x08          ; SLP_TIMER1
                li a4, 8 << 11             ; wake-up enable: timer
                s32i a4, a2, 0x38
                s32i a3, a2, 0x84          ; DIG_PWC: deep sleep power-down
                s32i a3, a2, 0x18          ; STATE0: SLEEP_EN
         zz:    j zz
         woke:  movi a6, 0
                li a9, rtc_get_reset_reason
                callx4 a9
                li a2, RESULTS
                s32i a6, a2, 0",
    );
    m.run_for(ms(50));
    assert!(m.sleeping.as_ref().map(|s| s.deep).unwrap_or(false), "in deep sleep: {:?}", m.messages);
    m.run_for(ms(300));
    assert!(m.sleeping.is_none());
    assert_eq!(log.lock().unwrap().uart0(), "12");
    assert_eq!(m.sys.peek32(RESULTS), 5, "reset reason DEEPSLEEP_RESET");
    assert!(m.messages.iter().any(|s| s.contains("woke from deep sleep")));
}

#[test]
fn crash_on_unmapped_access_is_reported_not_fatal() {
    let (mut m, _log) = boot(
        "li a2, 0x20000000
         l32i a3, a2, 0",
    );
    let st = m.run_for(ms(5));
    // the fault goes to the (ROM) exception vector: the machine keeps a
    // record and either halts or keeps running, but never panics
    let _ = st;
    assert!(m.cpus[0].last_exception.is_some());
}

#[test]
fn rmt_transmits_waveform_items() {
    let (mut m, log) = boot(
        "       li a2, GPIO
                movi a3, 87                ; RMT_SIG_OUT0 -> GPIO18
                addmi a7, a2, 0x500
                s32i a3, a7, 0x78
                li a3, 0x40000
                s32i a3, a2, 0x24
                li a2, 0x3ff56000          ; RMT
                l32i a3, a2, 0x20          ; CH0CONF0: divider 1
                movi a4, -256
                and a3, a3, a4
                addi a3, a3, 1
                s32i a3, a2, 0x20
                li a5, 0x3ff56800          ; channel 0 RAM
                li a3, (20 << 16) | 0x8000 | 10   ; high 10 ticks, low 20 ticks
                s32i a3, a5, 0
                li a3, (5 << 16) | 0x8000 | 40    ; high 40, low 5
                s32i a3, a5, 4
                movi a3, 0                 ; end marker
                s32i a3, a5, 8
                li a3, (1 << 17) | (1 << 3) | 1   ; APB clock, reset read pointer, tx_start
                s32i a3, a2, 0x24",
    );
    m.run_for(ms(2));
    let l = log.lock().unwrap();
    let (pin, items) = l.rmt.first().expect("RMT transmission");
    assert_eq!(*pin, 18);
    let lv: Vec<bool> = items.iter().map(|i| i.level).collect();
    assert_eq!(&lv[..4], &[true, false, true, false]);
    let d: Vec<u32> = items.iter().map(|i| i.duration_ns).collect();
    // durations keep their 10:20:40:5 ratios
    assert_eq!(d[1], 2 * d[0]);
    assert_eq!(d[2], 4 * d[0]);
    let tick = 1e9 / m.sys.apb_hz() as f64;
    assert!((d[0] as f64 - 10.0 * tick).abs() <= tick, "{} ns per 10 ticks", d[0]);
}

#[test]
fn adc1_converts_the_circuit_voltage() {
    let (mut m, _log) = boot_with(
        "       li a2, 0x3ff48800          ; SENS
                li a3, 3 << 16             ; 12-bit
                s32i a3, a2, 0x00
                li a3, 3 << 12             ; channel 6 (GPIO34): 11 dB
                s32i a3, a2, 0x34
                li a3, (1 << 31) | (1 << (19 + 6)) | (1 << 18)
                s32i a3, a2, 0x54
                li a4, 1 << 17             ; start
                or a3, a3, a4
                s32i a3, a2, 0x54
         done:  l32i a5, a2, 0x54
                bbci a5, 16, done
                li a6, RESULTS
                extui a5, a5, 0, 16
                s32i a5, a6, 0",
        |io| {
            io.adc.insert(34, 1.65);
        },
    );
    m.run_for(ms(1));
    let raw = m.sys.peek32(RESULTS);
    assert!((2046..=2049).contains(&raw), "raw {}", raw);
}

#[test]
fn code_and_data_in_mapped_flash() {
    let rom = "
        .equ RESULTS, 0x3ffb0000
        _start: entry a1, 64
                li a2, 0x3f400020          ; DROM
                l32i a3, a2, 0
                li a4, RESULTS
                s32i a3, a4, 0
                li a9, 0x400d0020          ; IROM function
                callx4 a9
        _halt:  j _halt
    ";
    let ram = xtensa_core::assemble(rom, 0x4008_0400).unwrap();
    let irom = xtensa_core::assemble(
        "entry a1, 32
         li a2, 0x3ffb0004
         movi a3, 0x77
         s32i a3, a2, 0
         retw",
        0x400D_0020,
    )
    .unwrap();
    let img = esp32_soc::elf::build_app_image(
        0x4008_0400,
        &[(0x4008_0400, ram.bytes), (0x3F40_0020, b"FLSH and more data".to_vec()), (0x400D_0020, irom.bytes)],
    );
    let (mut m, _log) = boot_image(&img);
    m.run_for(ms(1));
    assert_eq!(&m.sys.peek32(RESULTS).to_le_bytes(), b"FLSH");
    assert_eq!(m.sys.peek32(RESULTS + 4), 0x77, "IROM code ran");
}

#[test]
fn gpio_edge_interrupt_reaches_the_cpu() {
    let (mut m, log) = boot(
        "       j main
                .align 1024
         vectors:
                .org vectors + 0x300
         kernel_vec:
                li a2, GPIO
                movi a3, 0x10
                s32i a3, a2, 0x4c          ; STATUS_W1TC: GPIO4
                li a2, RESULTS
                l32i a3, a2, 8
                addi a3, a3, 1
                s32i a3, a2, 8
                rfe
         main:  la a4, vectors
                wsr a4, vecbase
                li a4, GPIO
                li a5, (1 << 7) | (4 << 13) ; rising edge, PRO CPU interrupt
                s32i a5, a4, 0x98          ; GPIO_PIN4
                li a4, 0x3ff00000          ; DPORT
                movi a5, 1                 ; GPIO source (22) -> CPU interrupt 1
                s32i a5, a4, 0x104 + 22 * 4
                movi a5, 2
                wsr a5, intenable
                rsil a5, 0
         idle:  waiti 0
                j idle",
    );
    let count = |m: &mut esp32_soc::Machine| m.sys.peek32(RESULTS + 8);
    m.run_for(ms(2));
    assert_eq!(count(&mut m), 0);
    log.lock().unwrap().inputs.insert(4, true);
    m.run_for(ms(2));
    assert_eq!(count(&mut m), 1, "rising edge");
    log.lock().unwrap().inputs.insert(4, false);
    m.run_for(ms(2));
    assert_eq!(count(&mut m), 1, "falling edge ignored");
    log.lock().unwrap().inputs.insert(4, true);
    m.run_for(ms(2));
    assert_eq!(count(&mut m), 2);
}

#[test]
fn breakpoints_pause_the_machine() {
    let (img, prog) = image(
        "       movi a2, 1
         here:  movi a2, 2
                li a3, RESULTS
                s32i a2, a3, 0",
    );
    let (mut m, _log) = boot_image(&img);
    let bp = prog.symbol("here");
    m.cpus[0].breakpoints.push(bp);
    assert_eq!(m.run_for(ms(1)), RunState::Breakpoint(0, bp));
    assert_eq!(m.cpus[0].ar(2), 1);
    // resume past it
    m.cpus[0].breakpoints.clear();
    m.state = RunState::Running;
    m.run_for(ms(1));
    assert_eq!(m.sys.peek32(RESULTS), 2);
}

#[test]
fn ets_delay_us_advances_emulated_time() {
    let (mut m, _log) = boot(
        "rsr a12, ccount
         li a6, 2000
         li a9, ets_delay_us
         callx4 a9
         rsr a13, ccount
         sub a13, a13, a12
         li a2, RESULTS
         s32i a13, a2, 0",
    );
    m.run_for(ms(10));
    let cycles = m.sys.peek32(RESULTS) as f64;
    let us = cycles / (m.sys.cpu_hz as f64 / 1e6);
    assert!((1990.0..2200.0).contains(&us), "delay took {} us", us);
}
