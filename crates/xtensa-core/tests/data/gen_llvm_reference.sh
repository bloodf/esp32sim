#!/bin/sh
# Regenerates llvm_check.hex (reference encoding of llvm_check.s) with LLVM.
set -e
cd "$(dirname "$0")"
llvm-mc-17 -triple=xtensa -filetype=obj llvm_check.s -o /tmp/llvm_check.o
if llvm-readelf-17 -r /tmp/llvm_check.o | grep -q R_XTENSA; then echo "unresolved relocations" >&2; exit 1; fi
llvm-objcopy-17 -O binary -j .text /tmp/llvm_check.o /tmp/llvm_check.bin
od -An -v -tx1 /tmp/llvm_check.bin | tr -d ' \n' > llvm_check.hex
echo >> llvm_check.hex
