# Assembled by both llvm-mc-17 and xtensa_core::asm; bytes must match.
# Regenerate the reference with tests/data/gen_llvm_reference.sh
start:
    add a2, a3, a4
    addx2 a5, a6, a7
    addx4 a8, a9, a10
    addx8 a11, a12, a13
    sub a14, a15, a0
    subx2 a1, a2, a3
    subx4 a4, a5, a6
    subx8 a7, a8, a9
    and a2, a3, a4
    or a5, a6, a7
    xor a8, a9, a10
    neg a2, a3
    abs a4, a5
    extui a2, a3, 4, 8
    extui a6, a7, 20, 12
    srai a2, a3, 5
    srai a4, a5, 20
    srli a6, a7, 9
    slli a8, a9, 3
    slli a10, a11, 25
    src a2, a3, a4
    srl a5, a6
    sll a7, a8
    sra a9, a10
    ssr a2
    ssl a3
    ssa8l a4
    ssai 5
    ssai 27
    movi a2, 5
    movi a3, -1
    movi a4, 2047
    movi a5, -2048
    addi a6, a7, 100
    addi a8, a9, -128
    addmi a10, a11, 512
    addmi a12, a13, -32768
    l8ui a2, a3, 255
    l16ui a4, a5, 510
    l16si a6, a7, 2
    l32i a8, a9, 1020
    s8i a10, a11, 7
    s16i a12, a13, 12
    s32i a14, a15, 16
    moveqz a2, a3, a4
    movnez a5, a6, a7
    movltz a8, a9, a10
    movgez a11, a12, a13
    wsr a2, sar
    jx a3
    callx0 a4
    ret
    memw
    isync
    dsync
    esync
    rsync
    nop
back:
    beqz a2, back
    bnez a3, fwd
    bltz a4, back
    bgez a5, fwd
    beqi a6, -1, back
    bnei a7, 256, fwd
    blti a8, 12, back
    bgei a9, 32, fwd
    bltui a10, 32768, back
    bgeui a11, 65536, fwd
    beq a2, a3, back
    bne a4, a5, fwd
    blt a6, a7, back
    bge a8, a9, fwd
    bltu a10, a11, back
    bgeu a12, a13, fwd
    bany a14, a15, back
    bnone a2, a3, fwd
    ball a4, a5, back
    bnall a6, a7, fwd
    bbc a8, a9, back
    bbs a10, a11, fwd
    bbci a12, 3, back
    bbsi a13, 31, fwd
    bbci a14, 17, back
    j back
    j fwd
fwd:
    nop
