// Debugger panel: run control, stepping, call stack, registers, watches, memory, disassembly.
import { api, el, toast } from './api.js';

export class DebugPanel {
  constructor(root, editor, state) {
    this.root = root;
    this.editor = editor;
    this.state = state;
    this.core = 1;
    this.watches = JSON.parse(localStorage.getItem('esp32sim.watches') || '[]');
    this.lastState = '';
    this.regs = null;
    this.render();
    document.addEventListener('breakpoints-changed', () => this.renderBreakpoints());
  }

  render() {
    const r = this.root;
    r.innerHTML = '';
    const coreSel = this.coreSel = el('select', { class: 'dbg-core', onchange: (e) => { this.core = parseInt(e.target.value, 10); this.refresh(); } },
      el('option', { value: 1, selected: this.core === 1 }, 'Core 1 (APP — Arduino loop)'), el('option', { value: 0, selected: this.core === 0 }, 'Core 0 (PRO — WiFi/system)'));
    r.append(el('div', { class: 'dbg-controls' },
      el('button', { title: 'Continue (F5)', onclick: () => this.control('resume') }, '▶'),
      el('button', { title: 'Pause (F6)', onclick: () => this.control('pause') }, '⏸'),
      el('button', { title: 'Step over (F10)', onclick: () => this.step('over') }, '⤼ over'),
      el('button', { title: 'Step into (F11)', onclick: () => this.step('into') }, '⤓ into'),
      el('button', { title: 'Step one instruction', onclick: () => this.step('instruction') }, '⟶ instr')), coreSel);
    this.info = el('div', { class: 'muted', style: { margin: '8px 0', fontSize: '12px' } });
    r.append(this.info);
    r.append(el('div', { class: 'section-title' }, 'Call stack'));
    this.stack = el('div');
    r.append(this.stack);
    r.append(el('div', { class: 'section-title' }, 'Locals'));
    this.localsEl = el('div', { class: 'kv locals' });
    r.append(this.localsEl);
    r.append(el('div', { class: 'section-title' }, 'Watch (global symbols)'));
    const wi = el('input', { placeholder: 'variable name, e.g. counter' });
    wi.addEventListener('keydown', (e) => { if (e.key === 'Enter' && wi.value) { this.watches.push(wi.value.trim()); wi.value = ''; this.saveWatches(); this.refreshWatches(); } });
    this.watchEl = el('div', { class: 'kv' });
    r.append(wi, this.watchEl);
    r.append(el('div', { class: 'section-title' }, 'Breakpoints'));
    this.bpEl = el('div');
    r.append(this.bpEl);
    r.append(el('div', { class: 'section-title' }, 'Registers'));
    this.regEl = el('div', { class: 'kv' });
    r.append(this.regEl);
    r.append(el('div', { class: 'section-title' }, 'Disassembly'));
    this.disEl = el('div', { class: 'dis' });
    r.append(this.disEl);
    r.append(el('div', { class: 'section-title' }, 'Memory'));
    const mi = el('input', { placeholder: 'address (hex) e.g. 0x3ffb0000' });
    this.memEl = el('div', { class: 'hexdump' });
    mi.addEventListener('keydown', (e) => { if (e.key === 'Enter') this.mem(mi.value); });
    r.append(mi, this.memEl);
    this.renderBreakpoints();
  }

  saveWatches() { localStorage.setItem('esp32sim.watches', JSON.stringify(this.watches)); }

  control(cmd) {
    this.editor.clearExecution();
    api('/api/control', { json: { cmd } }).catch((e) => toast(e.message, true));
  }

  async step(kind) {
    this.info.textContent = 'stepping…';
    try {
      const regs = await api('/api/debug/step', { json: { core: this.core, kind } });
      this.show(regs);
    } catch (e) { toast(e.message, true); }
  }

  async refresh() {
    try {
      const regs = await api('/api/debug/regs?core=' + this.core);
      this.show(regs, false);
    } catch (e) { /* not running */ }
    this.refreshWatches();
  }

  onStatus(st) {
    const s = st.state || '';
    if (s !== this.lastState) {
      this.lastState = s;
      if (s.startsWith('breakpoint')) {
        const m = s.match(/core (\d)/);
        if (m) this.core = parseInt(m[1], 10);
        this.refresh().then(() => {
          if (st.location && st.location.file) this.editor.showExecution(st.location.file, st.location.line);
        });
        document.querySelector('#activity button[data-panel=debug]').click();
        toast('Breakpoint hit');
      } else if (s === 'running') {
        this.editor.clearExecution();
      }
    }
  }

  show(regs, reveal = true) {
    if (!regs || !regs.pc) return;
    this.regs = regs;
    if (this.coreSel) this.coreSel.value = String(regs.core);
    const loc = regs.location;
    this.info.textContent = `core ${regs.core} · pc ${regs.pc} · ${regs.symbol}${loc ? ` · ${loc.file.split('/').pop()}:${loc.line}` : ''}`;
    if (loc && reveal) this.editor.showExecution(loc.file, loc.line);
    this.stack.innerHTML = '';
    for (const f of regs.backtrace || []) {
      this.stack.append(el('div', { class: 'frame', title: f.file ? `${f.file}:${f.line}` : '', onclick: () => f.file && this.editor.showExecution(f.file, f.line) }, f.text + (f.line ? `  (${f.file.split('/').pop()}:${f.line})` : '')));
    }
    this.localsEl.innerHTML = '';
    const locals = regs.locals || [];
    if (!locals.length) this.localsEl.append(el('span', { class: 'muted' }, loc ? 'none in scope' : 'needs the firmware ELF with -g'), el('span', {}, ''));
    for (const v of locals) this.localsEl.append(el('span', { title: v.type }, v.name), el('span', { title: `${v.type} = ${v.value}` }, v.value));
    this.regEl.innerHTML = '';
    regs.a.forEach((v, i) => this.regEl.append(el('span', {}, 'a' + i), el('span', {}, v)));
    for (const k of ['ps', 'sar', 'windowbase', 'windowstart', 'lbeg', 'lend', 'lcount', 'exccause', 'excvaddr', 'ccount', 'intenable', 'interrupt']) this.regEl.append(el('span', {}, k), el('span', {}, String(regs[k])));
    this.disEl.innerHTML = '';
    for (const d of regs.disassembly || []) this.disEl.append(el('div', { class: d.current ? 'cur' : '' }, `${d.addr}  ${d.text}`));
    this.refreshWatches();
  }

  async refreshWatches() {
    // several refreshes can overlap (status updates, steps): only the newest one renders
    const seq = (this.watchSeq = (this.watchSeq || 0) + 1);
    const rows = [];
    for (const [i, w] of this.watches.entries()) {
      let v;
      try { v = await api('/api/debug/symbol?name=' + encodeURIComponent(w)); } catch (e) { v = { error: e.message }; }
      const txt = v.error ? v.error : `${v.i32}  (0x${(v.u32 >>> 0).toString(16)}, ${Number(v.f32).toPrecision(5)}f)`;
      rows.push(el('span', { title: 'click to remove', style: { cursor: 'pointer' }, onclick: () => { this.watches.splice(i, 1); this.saveWatches(); this.refreshWatches(); } }, w), el('span', { title: v.address || '' }, txt));
    }
    if (seq !== this.watchSeq) return;
    this.watchEl.innerHTML = '';
    this.watchEl.append(...rows);
  }

  renderBreakpoints() {
    if (!this.bpEl) return;
    this.bpEl.innerHTML = '';
    const list = this.editor.allBreakpoints();
    if (!list.length) this.bpEl.append(el('div', { class: 'muted', style: { fontSize: '12px' } }, 'Click in the editor gutter to add breakpoints (needs a build with -g and the firmware ELF).'));
    for (const b of list) this.bpEl.append(el('div', { class: 'frame', onclick: () => this.editor.open(b.file, b.line) }, `● ${b.file}:${b.line}`));
  }

  async mem(addr) {
    try {
      const r = await api('/api/debug/mem?len=256&addr=' + encodeURIComponent(addr.trim()));
      const base = parseInt(r.addr, 16);
      const bytes = r.hex.match(/../g) || [];
      let out = '';
      for (let i = 0; i < bytes.length; i += 16) {
        const row = bytes.slice(i, i + 16);
        const ascii = row.map((b) => { const c = parseInt(b, 16); return c >= 32 && c < 127 ? String.fromCharCode(c) : '.'; }).join('');
        out += (base + i).toString(16).padStart(8, '0') + '  ' + row.join(' ').padEnd(48) + ' ' + ascii + '\n';
      }
      this.memEl.textContent = out;
    } catch (e) { toast(e.message, true); }
  }
}
