; mockfw data segment (DRAM): strings, line buffer and variables.
s_ready:     .asciz "mockfw ready\n"
s_ping:      .asciz "ping"
s_pong:      .asciz "pong\n"
s_ledon:     .asciz "led on"
s_ledoff:    .asciz "led off"
s_ok:        .asciz "ok\n"
s_count:     .asciz "count"
s_counteq:   .asciz "count="
s_reboot:    .asciz "reboot"
s_rebooting: .asciz "rebooting\n"
s_echo:      .asciz "echo:"
s_nl:        .asciz "\n"
             .align 4
BUF:         .space 64
VARS:        .space 32
DEC_BUF:     .space 15
DEC_END:     .space 1
             .align 4
