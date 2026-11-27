    .file 1 "blink.c"
    .text
    .align 4
.Ltext_start:
.Lk_counter:
    .long counter
.Lk_uart:
    .long 0x3FF40000

    .global step
    .type step, @function
    .align 4
step:
    .loc 1 4 0
    nop
    .loc 1 5 0
    addi a2, a2, 3
    ret
.Lstep_end:
    .size step, .-step

    .global app_main
    .type app_main, @function
    .align 4
app_main:
    .loc 1 8 0
    nop
.Lloop:
    .loc 1 10 0
    l32r a8, .Lk_counter
    l32i a2, a8, 0
    call0 step
    l32r a8, .Lk_counter
    s32i a2, a8, 0
    .loc 1 11 0
    l32r a9, .Lk_uart
    movi a10, 46
    s32i a10, a9, 0
    .loc 1 9 0
    j .Lloop
.Lmain_end:
    .size app_main, .-app_main
.Ltext_end:

    .bss
    .align 4
    .global counter
    .type counter, @object
counter:
    .space 4
    .size counter, 4

    .section .debug_abbrev,"",@progbits
    .byte 1, 0x11, 1          # abbrev 1: compile_unit, has children
    .byte 0x03, 0x08          # DW_AT_name, string
    .byte 0x13, 0x05          # DW_AT_language, data2
    .byte 0x10, 0x17          # DW_AT_stmt_list, sec_offset
    .byte 0x11, 0x01          # DW_AT_low_pc, addr
    .byte 0x12, 0x06          # DW_AT_high_pc, data4 (length)
    .byte 0x1b, 0x08          # DW_AT_comp_dir, string
    .byte 0, 0
    .byte 2, 0x2e, 0          # abbrev 2: subprogram, no children
    .byte 0x03, 0x08          # name
    .byte 0x11, 0x01          # low_pc
    .byte 0x12, 0x06          # high_pc (length)
    .byte 0x3f, 0x19          # external, flag_present
    .byte 0, 0
    .byte 0

    .section .debug_info,"",@progbits
.Linfo_start:
    .long .Linfo_end - .Linfo_start - 4
    .short 4                  # DWARF 4
    .long .debug_abbrev       # abbrev offset (0)
    .byte 4                   # address size
    .byte 1
    .asciz "blink.c"
    .short 0x000c             # DW_LANG_C99
    .long .debug_line
    .long .Ltext_start
    .long .Ltext_end - .Ltext_start
    .asciz "."
    .byte 2
    .asciz "step"
    .long step
    .long .Lstep_end - step
    .byte 2
    .asciz "app_main"
    .long app_main
    .long .Lmain_end - app_main
    .byte 0
.Linfo_end:
