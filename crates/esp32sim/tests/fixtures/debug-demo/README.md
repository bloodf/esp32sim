# debug-demo fixture

A tiny bare-metal program with DWARF line info, used to test source-level
debugging (breakpoints by file:line, step into / over, symbol watches) without
an Xtensa C compiler.

* `blink.c` – the C program the assembly corresponds to (line numbers match).
* `blink.s` – hand-written Xtensa assembly with `.loc` directives for `blink.c`,
  plus a minimal `.debug_info` compile unit.
* `link.py` – a ~100-line static linker (pyelftools) that places `.text` in IRAM,
  `.bss` in DRAM and applies the relocations, producing `blink.elf`.

Rebuild:

    llvm-mc-17 -triple=xtensa -filetype=obj blink.s -o blink.o
    python3 link.py blink.o blink.elf
