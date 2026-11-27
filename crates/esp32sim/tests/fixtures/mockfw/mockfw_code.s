; mockfw — tiny ESP32 firmware used by the end-to-end tests (assembled by
; xtensa_core::asm at test time, so no Xtensa toolchain is needed).
;
;   boot        prints "mockfw ready"
;   GPIO4       push button (pull-up in the circuit); while pressed (low)
;               the board LED on GPIO2 is on; presses are counted
;   GPIO5       LED switched by serial commands
;   GPIO18      heartbeat, toggles every 500 ms
;   serial (UART0, lines ending in LF or CR):
;     ping        -> pong
;     led on/off  -> GPIO5 high/low, replies ok
;     count       -> count=<button presses>
;     reboot      -> software reset (ROM software_reset)
;     anything    -> echo:<line>
;
; Symbols from the data segment (strings, BUF, VARS) and ROM addresses are
; provided as .equ lines by the generator.

        .equ UART0, 0x3ff40000
        .equ GPIO,  0x3ff44000

_start: entry a1, 64
        li a2, GPIO
        li a3, (1 << 2) | (1 << 5) | (1 << 18)
        s32i a3, a2, 0x24          ; ENABLE_W1TS: outputs
        li a2, VARS
        movi a3, 0
        s32i a3, a2, 0             ; line length
        s32i a3, a2, 4             ; button presses
        s32i a3, a2, 12            ; loop counter
        s32i a3, a2, 16            ; heartbeat level
        movi a3, 1
        s32i a3, a2, 8             ; last button level (idle high)
        li a10, s_ready
        call8 puts
main:
        call8 poll_button
        call8 poll_uart
        call8 heartbeat
        li a10, 1000
        li a8, ets_delay_us
        callx8 a8
        j main

; ---------------------------------------------------------------- button
        .align 4
poll_button:
        entry a1, 32
        li a2, GPIO
        l32i a3, a2, 0x3c          ; GPIO_IN
        extui a3, a3, 4, 1
        movi a4, 4                 ; GPIO2 bit
        bnez a3, pb_up
        s32i a4, a2, 0x08          ; pressed: board LED on
        j pb_edge
pb_up:  s32i a4, a2, 0x0c
pb_edge:
        li a5, VARS
        l32i a6, a5, 8
        s32i a3, a5, 8
        beq a6, a3, pb_done
        bnez a3, pb_done           ; rising edge = release
        l32i a7, a5, 4
        addi a7, a7, 1
        s32i a7, a5, 4
pb_done:
        retw

; ------------------------------------------------------------------ uart
        .align 4
poll_uart:
        entry a1, 32
        li a2, UART0
pu_next:
        l32i a3, a2, 0x1c          ; STATUS: rx count in bits 0..7
        extui a3, a3, 0, 8
        beqz a3, pu_out
        l32i a4, a2, 0
        extui a4, a4, 0, 8
        beqi a4, 10, pu_line
        movi a5, 13
        beq a4, a5, pu_line
        li a5, VARS
        l32i a6, a5, 0
        movi a7, 63
        bge a6, a7, pu_next        ; line too long: drop
        li a7, BUF
        add a7, a7, a6
        s8i a4, a7, 0
        addi a6, a6, 1
        s32i a6, a5, 0
        j pu_next
pu_line:
        li a5, VARS
        l32i a6, a5, 0
        beqz a6, pu_next           ; empty line (2nd half of CRLF)
        li a7, BUF
        add a7, a7, a6
        movi a8, 0
        s8i a8, a7, 0
        s32i a8, a5, 0
        call8 handle_line
        j pu_next
pu_out:
        retw

        .align 4
handle_line:
        entry a1, 32
        li a10, BUF
        li a11, s_ping
        call8 streq
        bnez a10, hl_ping
        li a10, BUF
        li a11, s_ledon
        call8 streq
        bnez a10, hl_ledon
        li a10, BUF
        li a11, s_ledoff
        call8 streq
        bnez a10, hl_ledoff
        li a10, BUF
        li a11, s_count
        call8 streq
        bnez a10, hl_count
        li a10, BUF
        li a11, s_reboot
        call8 streq
        bnez a10, hl_reboot
        li a10, s_echo
        call8 puts
        li a10, BUF
        call8 puts
        li a10, s_nl
        call8 puts
        retw
hl_ping:
        li a10, s_pong
        call8 puts
        retw
hl_ledon:
        li a2, GPIO
        movi a3, 0x20
        s32i a3, a2, 0x08
        li a10, s_ok
        call8 puts
        retw
hl_ledoff:
        li a2, GPIO
        movi a3, 0x20
        s32i a3, a2, 0x0c
        li a10, s_ok
        call8 puts
        retw
hl_count:
        li a10, s_counteq
        call8 puts
        li a2, VARS
        l32i a10, a2, 4
        call8 print_dec
        li a10, s_nl
        call8 puts
        retw
hl_reboot:
        li a10, s_rebooting
        call8 puts
        li a10, 20000              ; let the UART drain
        li a8, ets_delay_us
        callx8 a8
        li a8, software_reset
        callx8 a8
        retw

; --------------------------------------------------------------- helpers
        .align 4
streq:                             ; a2 = 1 if the C strings a2 and a3 are equal
        entry a1, 32
se_loop:
        l8ui a4, a2, 0
        l8ui a5, a3, 0
        bne a4, a5, se_no
        beqz a4, se_yes
        addi a2, a2, 1
        addi a3, a3, 1
        j se_loop
se_no:  movi a2, 0
        retw
se_yes: movi a2, 1
        retw

        .align 4
puts:                              ; write the C string a2 to UART0
        entry a1, 32
        li a3, UART0
p_loop: l8ui a4, a2, 0
        beqz a4, p_done
p_wait: l32i a5, a3, 0x1c
        extui a5, a5, 16, 8        ; tx fifo count
        movi a6, 120
        bge a5, a6, p_wait
        s32i a4, a3, 0
        addi a2, a2, 1
        j p_loop
p_done: retw

        .align 4
print_dec:                         ; print the unsigned number a2
        entry a1, 32
        li a3, DEC_END
        movi a4, 0
        s8i a4, a3, 0
        movi a5, 10
pd_loop:
        remu a6, a2, a5
        quou a2, a2, a5
        addi a6, a6, '0'
        addi a3, a3, -1
        s8i a6, a3, 0
        bnez a2, pd_loop
        mov a10, a3
        call8 puts
        retw

        .align 4
heartbeat:                         ; toggle GPIO18 every 500 loop iterations
        entry a1, 32
        li a2, VARS
        l32i a3, a2, 12
        addi a3, a3, 1
        li a4, 500
        blt a3, a4, hb_store
        movi a3, 0
        l32i a5, a2, 16
        movi a6, 1
        xor a5, a5, a6
        s32i a5, a2, 16
        li a7, GPIO
        li a8, 1 << 18
        bnez a5, hb_on
        s32i a8, a7, 0x0c
        j hb_store
hb_on:  s32i a8, a7, 0x08
hb_store:
        s32i a3, a2, 12
        retw
