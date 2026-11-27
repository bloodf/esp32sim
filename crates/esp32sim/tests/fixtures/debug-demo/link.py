"""Tiny static linker for one llvm-mc Xtensa object: places .text in IRAM and
.bss in DRAM, applies R_XTENSA_32 / SLOT0_OP(call0) relocations (also inside
the DWARF sections) and writes an ELF executable with symbols + debug info."""
import struct, sys
from elftools.elf.elffile import ELFFile

src, out = sys.argv[1], sys.argv[2]
f = open(src, 'rb'); elf = ELFFile(f)
ADDR = {'.text': 0x40080400, '.bss': 0x3FFB0000}
secs = {s.name: s for s in elf.iter_sections()}
data = {n: bytearray(s.data()) if s['sh_type'] != 'SHT_NOBITS' else bytearray(s['sh_size']) for n, s in secs.items()}
symtab = secs['.symtab']
syms = list(symtab.iter_symbols())
sec_by_idx = {i: s.name for i, s in enumerate(elf.iter_sections())}

def sym_addr(sym):
    sec = sec_by_idx.get(sym['st_shndx']) if isinstance(sym['st_shndx'], int) else None
    return ADDR.get(sec, 0) + sym['st_value']

for rs in elf.iter_sections():
    if not rs.name.startswith('.rela'):
        continue
    target = rs.name[5:]
    base = ADDR.get(target, 0)
    for r in rs.iter_relocations():
        sym = syms[r['r_info_sym']]
        S = sym_addr(sym) + r['r_addend']
        off = r['r_offset']
        t = r['r_info_type']
        if t == 1:  # R_XTENSA_32
            old = struct.unpack_from('<I', data[target], off)[0]
            struct.pack_into('<I', data[target], off, (old + S) & 0xFFFFFFFF)
        elif t == 20:  # SLOT0_OP: call0 / j
            P = base + off
            w = data[target][off] | data[target][off + 1] << 8 | data[target][off + 2] << 16
            if w & 0xF == 5:  # CALLn
                rel = ((S & ~3) - ((P & ~3) + 4)) >> 2
                w = (w & 0x3F) | ((rel & 0x3FFFF) << 6)
            else:
                raise SystemExit('unsupported SLOT0 op %06x' % w)
            data[target][off:off + 3] = bytes([w & 0xFF, (w >> 8) & 0xFF, (w >> 16) & 0xFF])
        else:
            raise SystemExit('reloc type %d' % t)

# output layout
keep = ['.text', '.bss', '.debug_abbrev', '.debug_info', '.debug_line']
names = [''] + keep + ['.symtab', '.strtab', '.shstrtab']
shstr = b'\0'; shname = {}
for n in names[1:]:
    shname[n] = len(shstr); shstr += n.encode() + b'\0'
# symbols
strtab = b'\0'; symbytes = b''
newidx = {n: i for i, n in enumerate(names)}
symbytes += b'\0' * 16
for s in syms[1:]:
    if s['st_info']['type'] == 'STT_SECTION':
        continue
    nameoff = len(strtab); strtab += s.name.encode() + b'\0'
    sec = sec_by_idx.get(s['st_shndx']) if isinstance(s['st_shndx'], int) else None
    info = {'STB_LOCAL': 0, 'STB_GLOBAL': 1}[s['st_info']['bind']] << 4 | {'STT_NOTYPE': 0, 'STT_OBJECT': 1, 'STT_FUNC': 2}[s['st_info']['type']]
    symbytes += struct.pack('<IIIBBH', nameoff, sym_addr(s), s['st_size'], info, 0, newidx.get(sec, 0))
blobs = {n: bytes(data[n]) for n in keep}
blobs['.symtab'] = symbytes; blobs['.strtab'] = strtab; blobs['.shstrtab'] = shstr
# file: ehdr, 2 phdrs, section data, shdrs
off = 52 + 2 * 32
offs = {}
body = b''
for n in names[1:]:
    while (off + len(body)) % 4: body += b'\0'
    offs[n] = off + len(body); body += blobs[n]
while (off + len(body)) % 4: body += b'\0'
shoff = off + len(body)
ph = b''
for n, flags in (('.text', 5), ('.bss', 6)):
    ph += struct.pack('<IIIIIIII', 1, offs[n], ADDR[n], ADDR[n], len(blobs[n]), len(blobs[n]), flags, 4)
sh = b'\0' * 40
for n in names[1:]:
    typ = {'.symtab': 2, '.strtab': 3, '.shstrtab': 3}.get(n, 1)
    flg = {'.text': 6, '.bss': 3}.get(n, 0)
    link = newidx['.strtab'] if n == '.symtab' else 0
    info = 1 if n == '.symtab' else 0
    es = 16 if n == '.symtab' else 0
    sh += struct.pack('<IIIIIIIIII', shname[n], typ, flg, ADDR.get(n, 0), offs[n], len(blobs[n]), link, info, 4, es)
app_main = [s for s in syms if s.name == 'app_main'][0]
eh = b'\x7fELF' + bytes([1, 1, 1, 0]) + b'\0' * 8
eh += struct.pack('<HHIIIIIHHHHHH', 2, 94, 1, sym_addr(app_main), 52, shoff, 0x300, 52, 32, 2, 40, len(names), newidx['.shstrtab'])
open(out, 'wb').write(eh + ph + body + sh)
print('entry 0x%08x' % sym_addr(app_main))
