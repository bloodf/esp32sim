// Circuit editor: board + parts + wires, live simulation view.
import { svg, api, toast, bus } from './api.js';
import { renderPart } from './parts.js';

// Same rules as the simulator (crates/devices/src/registry.rs, pad_number):
// named pins, ADC / touch channel aliases, then GPIOn / IOn / Dn / Gn / Pn.
const PAD_ALIASES = {
  TX: 1, TX0: 1, TXD0: 1, U0TXD: 1, RX: 3, RX0: 3, RXD0: 3, U0RXD: 3, TX2: 17, U2TXD: 17, RX2: 16, U2RXD: 16,
  VP: 36, SVP: 36, SENSOR_VP: 36, VN: 39, SVN: 39, SENSOR_VN: 39, SDA: 21, SCL: 22, MOSI: 23, MISO: 19, SCK: 18, SCLK: 18, SS: 5, CS: 5,
  LED: 2, BUILTIN_LED: 2, LED_BUILTIN: 2, BOOT: 0, DAC1: 25, DAC2: 26, CLK: 6, SD0: 7, SD1: 8, SD2: 9, SD3: 10, CMD: 11,
  A0: 36, A3: 39, A4: 32, A5: 33, A6: 34, A7: 35, A10: 4, A11: 0, A12: 2, A13: 15, A14: 13, A15: 12, A16: 14, A17: 27, A18: 25, A19: 26,
  T0: 4, T1: 0, T2: 2, T3: 15, T4: 13, T5: 12, T6: 14, T7: 27, T8: 33, T9: 32,
};
export function padNumber(label) {
  const s = String(label).trim().toUpperCase();
  if (s in PAD_ALIASES) return PAD_ALIASES[s];
  const m = s.replace(/^GPIO/, '').replace(/^IO/, '').replace(/^D/, '').replace(/^G/, '').replace(/^P/, '').match(/^\d+$/);
  if (!m) return null;
  const n = parseInt(m[0], 10);
  // pads that exist on the ESP32
  return n < 40 && n !== 20 && n !== 24 && !(n >= 28 && n <= 31) ? n : null;
}

const WIRE_PALETTE = ['#66bb6a', '#ab47bc', '#26c6da', '#ffa726', '#ec407a', '#26a69a', '#d4e157', '#8d6e63', '#7e57c2', '#29b6f6'];

export class Circuit {
  constructor(svgEl) {
    this.svg = svgEl;
    this.project = null;
    this.catalog = [];
    this.boards = [];
    this.parts = new Map(); // id -> {r, g, cfg}
    this.view = { x: 0, y: 0, k: 1.4 };
    this.selected = null; // {type: 'part'|'wire', id|index}
    this.wiring = null; // {from, points: []}
    this.boardPins = new Map();
    this.states = {};
    this.pads = {};
    this.dirtyTimer = null;
    this.build();
  }

  build() {
    const s = this.svg;
    s.innerHTML = '';
    const defs = svg('defs');
    defs.innerHTML = `
      <filter id="glow" x="-100%" y="-100%" width="300%" height="300%"><feGaussianBlur stdDeviation="5"/></filter>
      <filter id="glow-small" x="-100%" y="-100%" width="300%" height="300%"><feGaussianBlur stdDeviation="1.2" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
      <pattern id="grid" width="20" height="20" patternUnits="userSpaceOnUse"><circle cx="1" cy="1" r="0.8" fill="#2a2c31"/></pattern>`;
    s.append(defs);
    this.root = svg('g');
    this.bg = svg('rect', { x: -5000, y: -5000, width: 10000, height: 10000, fill: 'url(#grid)' });
    this.root.append(this.bg);
    this.layerWires = svg('g');
    this.layerParts = svg('g');
    this.layerTop = svg('g');
    this.root.append(this.layerWires, this.layerParts, this.layerTop);
    s.append(this.root);
    this.tooltip = svg('g', { 'pointer-events': 'none', opacity: 0 });
    this.tipRect = svg('rect', { rx: 3, fill: '#000d', stroke: '#555' });
    this.tipText = svg('text', { 'font-size': 9, fill: '#fff', 'font-family': 'monospace' });
    this.tooltip.append(this.tipRect, this.tipText);
    this.layerTop.append(this.tooltip);
    this.applyView();
    this.bindCanvas();
  }

  applyView() {
    this.root.setAttribute('transform', `translate(${this.view.x},${this.view.y}) scale(${this.view.k})`);
  }

  toLocal(e) {
    const r = this.svg.getBoundingClientRect();
    return { x: (e.clientX - r.left - this.view.x) / this.view.k, y: (e.clientY - r.top - this.view.y) / this.view.k };
  }

  bindCanvas() {
    const s = this.svg;
    s.addEventListener('wheel', (e) => {
      e.preventDefault();
      const r = s.getBoundingClientRect();
      const mx = e.clientX - r.left, my = e.clientY - r.top;
      const k2 = Math.max(0.3, Math.min(6, this.view.k * (e.deltaY < 0 ? 1.12 : 1 / 1.12)));
      this.view.x = mx - ((mx - this.view.x) * k2) / this.view.k;
      this.view.y = my - ((my - this.view.y) * k2) / this.view.k;
      this.view.k = k2;
      this.applyView();
    }, { passive: false });
    s.addEventListener('pointerdown', (e) => {
      if (e.target !== this.bg && e.target !== s) return;
      if (this.wiring) {
        const p = this.toLocal(e);
        this.wiring.points.push([Math.round(p.x), Math.round(p.y)]);
        return;
      }
      this.select(null);
      const x0 = e.clientX, y0 = e.clientY, vx = this.view.x, vy = this.view.y;
      s.setPointerCapture(e.pointerId);
      const mv = (ev) => { this.view.x = vx + ev.clientX - x0; this.view.y = vy + ev.clientY - y0; this.applyView(); };
      const up = () => { s.removeEventListener('pointermove', mv); s.removeEventListener('pointerup', up); };
      s.addEventListener('pointermove', mv); s.addEventListener('pointerup', up);
    });
    s.addEventListener('pointermove', (e) => {
      if (this.wiring) this.drawPreview(this.toLocal(e));
    });
    window.addEventListener('keydown', (e) => {
      if (!document.getElementById('view-circuit').classList.contains('active')) return;
      if (['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement.tagName)) return;
      if (e.key === 'Escape' && this.wiring) { this.cancelWire(); }
      if ((e.key === 'Delete' || e.key === 'Backspace') && this.selected) { e.preventDefault(); this.deleteSelected(); }
      if (e.key === 'r' && this.selected && this.selected.type === 'part') this.rotateSelected();
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z') { e.preventDefault(); if (e.shiftKey) this.redo(); else this.undo(); }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'y') { e.preventDefault(); this.redo(); }
    });
    // drop from parts palette
    s.addEventListener('dragover', (e) => e.preventDefault());
    s.addEventListener('drop', (e) => {
      e.preventDefault();
      const kind = e.dataTransfer.getData('text/part');
      if (kind) { const p = this.toLocal(e); this.addPart(kind, p.x, p.y); }
    });
  }

  setCatalog(catalog, boards) {
    this.catalog = catalog;
    this.boards = boards;
  }

  info(kind) { return this.catalog.find((c) => c.type === kind); }

  load(project) {
    this.project = project;
    if (!this.project.components) this.project.components = [];
    if (!this.project.connections) this.project.connections = [];
    if (!this.project.layout) this.project.layout = {};
    this.history = [];
    this.future = [];
    this.lastSnap = this.snapshot();
    this.render();
  }

  // ------------------------------------------------------------ undo / redo
  snapshot() {
    const p = this.project || {};
    return JSON.stringify({ board: p.board, components: p.components, connections: p.connections, layout: p.layout });
  }
  restore(snap) {
    Object.assign(this.project, JSON.parse(snap));
    this.lastSnap = snap;
    this.selected = null;
    this.render();
    bus.emit('select', null);
    clearTimeout(this.dirtyTimer);
    this.save(true);
  }
  undo() {
    if (!this.history || !this.history.length) return;
    this.future.push(this.lastSnap);
    this.restore(this.history.pop());
  }
  redo() {
    if (!this.future || !this.future.length) return;
    this.history.push(this.lastSnap);
    this.restore(this.future.pop());
  }

  boardDef() {
    const id = (this.project && this.project.board) || 'esp32-devkit-v1';
    return this.boards.find((b) => b.id === id) || this.boards[0];
  }

  render() {
    this.layerParts.innerHTML = '';
    this.layerWires.innerHTML = '';
    this.parts.clear();
    if (!this.project) return;
    this.renderBoard();
    let auto = 0;
    for (const c of this.project.components) {
      if (c.x === undefined) { c.x = 420 + (auto % 3) * 140; c.y = 60 + Math.floor(auto / 3) * 130; auto++; }
      this.renderComponent(c);
    }
    this.renderWires();
    this.updateStates(this.states);
  }

  renderBoard() {
    const b = this.boardDef();
    if (!b) return;
    const pos = this.project.layout.board || { x: 120, y: 40 };
    const rows = Math.max(b.left.length, b.right.length);
    const pitch = 14, w = 150, top = 96, h = top + rows * pitch + 44;
    const g = svg('g', { transform: `translate(${pos.x},${pos.y})`, class: 'part' });
    this.boardBox = { x0: pos.x, y0: pos.y, x1: pos.x + w, y1: pos.y + h };
    g.append(svg('rect', { x: 0, y: 0, width: w, height: h, rx: 5, fill: '#111418', stroke: '#000' }));
    // module (metal can + antenna)
    g.append(svg('rect', { x: 30, y: 8, width: 90, height: 70, rx: 3, fill: '#263238' }));
    g.append(svg('rect', { x: 36, y: 26, width: 78, height: 48, rx: 2, fill: '#b0bec5', stroke: '#78909c' }));
    g.append(svg('path', { d: 'M40,12 h10 v8 h10 v-8 h10 v8 h10 v-8 h10 v8 h10', stroke: '#c9a227', fill: 'none', 'stroke-width': 1.6 }));
    g.append(svg('text', { x: 75, y: 46, 'text-anchor': 'middle', 'font-size': 8, fill: '#37474f', 'font-weight': 700 }, 'ESP-WROOM-32'));
    g.append(svg('text', { x: 75, y: 58, 'text-anchor': 'middle', 'font-size': 6, fill: '#455a64' }, 'emulated'));
    // USB
    g.append(svg('rect', { x: 58, y: h - 14, width: 34, height: 18, rx: 2, fill: '#9e9e9e', stroke: '#616161' }));
    // leds
    this.pwrLed = svg('circle', { cx: 44, cy: h - 22, r: 3, fill: '#ff1744', filter: 'url(#glow-small)' });
    this.ioLed = svg('circle', { cx: 106, cy: h - 22, r: 3, fill: '#0d2a4a' });
    g.append(this.pwrLed, this.ioLed);
    g.append(svg('text', { x: 106, y: h - 13, 'text-anchor': 'middle', 'font-size': 5, fill: '#90a4ae' }, 'D2'));
    // buttons
    const mkBtn = (x, label, onDown, onUp) => {
      const bg = svg('g', { class: 'interactive' });
      bg.append(svg('rect', { x: x - 9, y: h - 36, width: 18, height: 12, rx: 2, fill: '#cfd8dc' }), svg('circle', { cx: x, cy: h - 30, r: 3.5, fill: '#37474f' }));
      bg.append(svg('text', { x, y: h - 39, 'text-anchor': 'middle', 'font-size': 5.5, fill: '#b0bec5' }, label));
      bg.addEventListener('pointerdown', (e) => { e.stopPropagation(); onDown(); });
      bg.addEventListener('pointerup', (e) => { e.stopPropagation(); onUp && onUp(); });
      g.append(bg);
    };
    mkBtn(26, 'EN', () => bus.emit('control', 'reset'));
    mkBtn(124, 'BOOT', () => bus.emit('input', 'boot_button', { action: 'press' }), () => bus.emit('input', 'boot_button', { action: 'release' }));
    // headers
    this.boardPins.clear();
    const pinRow = (list, side) => list.forEach((p, i) => {
      const y = top + i * pitch;
      const x = side === 'left' ? 0 : w;
      const px = side === 'left' ? -7 : w + 7;
      g.append(svg('rect', { x: side === 'left' ? 2 : w - 14, y: y - 5, width: 12, height: 10, fill: '#212121', stroke: '#424242' }));
      g.append(svg('line', { x1: x, y1: y, x2: px, y2: y, stroke: '#bdbdbd', 'stroke-width': 2 }));
      g.append(svg('text', { x: side === 'left' ? 17 : w - 17, y: y + 2.5, 'font-size': 7, fill: '#e0e0e0', 'text-anchor': side === 'left' ? 'start' : 'end', 'font-family': 'monospace' }, p.label));
      const dot = svg('circle', { cx: px, cy: y, r: 3, class: 'pin-dot' });
      const key = p.gpio >= 0 ? 'GPIO' + p.gpio : p.label === 'GND' ? 'GND' : p.label === '3V3' ? '3V3' : (p.label === 'VIN' || p.label === '5V') ? '5V' : p.label;
      const ep = p.gpio >= 0 ? `esp32.GPIO${p.gpio}` : key;
      const abs = { x: pos.x + px, y: pos.y + y, side };
      if (!this.boardPins.has(key)) this.boardPins.set(key, []);
      this.boardPins.get(key).push(abs);
      this.bindPin(dot, ep, abs, p.gpio >= 0 ? `GPIO${p.gpio} (${p.label})` : p.label);
      g.append(dot);
    });
    pinRow(b.left, 'left');
    pinRow(b.right, 'right');
    g.append(svg('text', { x: w / 2, y: -6, 'text-anchor': 'middle', class: 'part-title', 'font-size': 9 }, b.name));
    this.makeDraggable(g, () => this.project.layout.board || { x: 120, y: 40 }, (x, y) => { this.project.layout.board = { x, y }; });
    this.layerParts.append(g);
  }

  renderComponent(c) {
    const info = this.info(c.type);
    const ctx = { id: c.id, input: (action) => bus.emit('input', c.id, action) };
    let r;
    try { r = renderPart(c.type, c, ctx, info); } catch (e) { console.error(e); return; }
    const rot = c.rotate || 0;
    const g = svg('g', { class: 'part', transform: `translate(${c.x},${c.y}) rotate(${rot})` });
    g.append(svg('rect', { class: 'outline', x: r.ox - 4, y: r.oy - 4, width: r.w + 8, height: r.h + 8, rx: 4, fill: 'transparent' }));
    g.append(r.g);
    const label = svg('text', { x: r.ox + r.w / 2, y: r.oy - 7, 'text-anchor': 'middle', 'font-size': 8, fill: '#90a4ae', 'pointer-events': 'none' }, c.id);
    g.append(label);
    const pinAbs = {};
    const rad = (rot * Math.PI) / 180;
    for (const p of r.pins) {
      const ax = c.x + p.x * Math.cos(rad) - p.y * Math.sin(rad);
      const ay = c.y + p.x * Math.sin(rad) + p.y * Math.cos(rad);
      pinAbs[p.name] = { x: ax, y: ay };
      const dot = svg('circle', { cx: p.x, cy: p.y, r: 2.6, class: 'pin-dot' });
      this.bindPin(dot, `${c.id}.${p.name}`, pinAbs[p.name], `${c.id}.${p.name}`);
      g.append(dot);
    }
    g.addEventListener('pointerdown', (e) => { if (!this.wiring) this.select({ type: 'part', id: c.id }); });
    this.makeDraggable(g, () => ({ x: c.x, y: c.y }), (x, y) => { c.x = x; c.y = y; });
    this.layerParts.append(g);
    this.parts.set(c.id, { r, g, cfg: c, pins: pinAbs });
  }

  bindPin(dot, endpoint, abs, title) {
    dot.setAttribute('data-ep', endpoint);
    dot.addEventListener('pointerdown', (e) => {
      e.stopPropagation();
      if (!this.wiring) this.startWire(endpoint, abs);
      else this.finishWire(endpoint);
    });
    dot.addEventListener('pointerenter', () => {
      const v = this.pinVoltage(endpoint);
      this.showTip(abs, `${title}${v !== null ? `  ${v}` : ''}`);
    });
    dot.addEventListener('pointerleave', () => this.tooltip.setAttribute('opacity', 0));
  }

  pinVoltage(ep) {
    const m = ep.match(/^esp32\.GPIO(\d+)$/);
    if (m) {
      const pad = this.pads[m[1]];
      if (!pad) return 'not connected';
      return pad.v === null || pad.v === undefined ? `${pad.mode}, floating` : `${pad.mode}, ${pad.v.toFixed(2)} V` + (pad.pwm ? ` PWM ${pad.pwm.freq.toFixed(0)}Hz ${(pad.pwm.duty * 100).toFixed(0)}%` : '');
    }
    if (ep === 'GND') return '0 V';
    if (ep === '3V3') return '3.3 V';
    if (ep === '5V') return '5 V';
    return null;
  }

  showTip(p, t) {
    this.tipText.textContent = t;
    this.tipText.setAttribute('x', p.x + 8);
    this.tipText.setAttribute('y', p.y - 8);
    const bb = this.tipText.getBBox();
    this.tipRect.setAttribute('x', bb.x - 3); this.tipRect.setAttribute('y', bb.y - 2);
    this.tipRect.setAttribute('width', bb.width + 6); this.tipRect.setAttribute('height', bb.height + 4);
    this.tooltip.setAttribute('opacity', 1);
  }

  makeDraggable(g, getPos, setPos) {
    g.addEventListener('pointerdown', (e) => {
      if (e.button !== 0 || this.wiring) return;
      if (e.target.classList && (e.target.classList.contains('pin-dot') || e.target.closest('.interactive'))) return;
      e.stopPropagation();
      const start = this.toLocal(e); const p0 = getPos();
      let moved = false;
      g.setPointerCapture(e.pointerId);
      const mv = (ev) => {
        const p = this.toLocal(ev);
        const nx = Math.round((p0.x + p.x - start.x) / 5) * 5, ny = Math.round((p0.y + p.y - start.y) / 5) * 5;
        if (Math.abs(p.x - start.x) + Math.abs(p.y - start.y) > 2) moved = true;
        setPos(nx, ny);
        g.setAttribute('transform', g.getAttribute('transform').replace(/translate\([^)]*\)/, `translate(${nx},${ny})`));
      };
      const up = () => {
        g.removeEventListener('pointermove', mv); g.removeEventListener('pointerup', up);
        if (moved) { this.render(); this.changed(false); }
      };
      g.addEventListener('pointermove', mv); g.addEventListener('pointerup', up);
    });
  }

  // ------------------------------------------------------------ wires
  endpointPos(ep, near = null) {
    const s = String(ep).trim();
    const up = s.toUpperCase();
    const rail = (k) => {
      const list = this.boardPins.get(k) || [];
      if (near && list.length > 1) {
        // a rail exists on several header pins: use the one facing the other end of the wire
        const d = (p) => Math.abs(p.x - near.x) + Math.abs(p.y - near.y) * 0.3;
        return [...list].sort((a, b) => d(a) - d(b))[0];
      }
      return list[0] || null;
    };
    if (['GND', '0V', 'VSS'].includes(up)) return rail('GND');
    if (['3V3', '3.3V', 'VCC', '3V'].includes(up)) return rail('3V3');
    if (['5V', 'VIN', 'VBUS'].includes(up)) return rail('5V');
    const dot = s.indexOf('.');
    const comp = dot < 0 ? '' : s.slice(0, dot), pin = dot < 0 ? s : s.slice(dot + 1);
    if (!comp || ['esp32', 'board', 'esp', 'mcu'].includes(comp.toLowerCase())) {
      const pu = pin.toUpperCase();
      if (pu === 'GND') return rail('GND');
      if (pu === '3V3') return rail('3V3');
      if (pu === 'VIN' || pu === '5V') return rail('5V');
      const n = padNumber(pin);
      if (n === null) return null;
      return (this.boardPins.get('GPIO' + n) || [])[0] || null;
    }
    const part = [...this.parts.entries()].find(([id]) => id.toLowerCase() === comp.toLowerCase());
    if (!part) return null;
    const pins = part[1].pins;
    const key = Object.keys(pins).find((k) => k.toLowerCase() === pin.toLowerCase());
    return key ? pins[key] : null;
  }

  connList() {
    // normalise connections into [{a, b, color, points, index}]
    const out = [];
    (this.project.connections || []).forEach((c, index) => {
      if (Array.isArray(c)) { for (let i = 0; i + 1 < c.length; i++) out.push({ a: c[i], b: c[i + 1], index, sub: i }); }
      else if (typeof c === 'string') { const parts = c.split(/\s*(?:->|<->| - )\s*/); for (let i = 0; i + 1 < parts.length; i++) out.push({ a: parts[i], b: parts[i + 1], index, sub: i }); }
      else if (c && c.from) out.push({ a: c.from, b: c.to, color: c.color, points: c.points, index, sub: 0 });
    });
    return out;
  }

  wireColor(a, b, i) {
    const t = (a + ' ' + b).toUpperCase();
    if (/\bGND\b|\.GND|VSS|\.K\b|\.C\b|\.-/.test(t)) return '#3a3a3a';
    if (/3V3|VCC|VDD|\+$|V\+/.test(t)) return '#e53935';
    if (/5V|VIN/.test(t)) return '#ff7043';
    if (/SDA/.test(t) || /GPIO21\b|D21\b/.test(t)) return '#42a5f5';
    if (/SCL/.test(t) || /GPIO22\b|D22\b/.test(t)) return '#fdd835';
    return WIRE_PALETTE[i % WIRE_PALETTE.length];
  }

  route(pa, pb, pts, lane = 0) {
    const out = [[pa.x, pa.y]];
    const exit = (p) => (p.side === 'left' ? -12 : p.side === 'right' ? 12 : 0);
    const ax = pa.x + exit(pa), bx = pb.x + exit(pb);
    if (pts && pts.length) {
      out.push([ax, pa.y]);
      for (const p of pts) out.push(p);
      out.push([bx, pb.y]);
      out.push([pb.x, pb.y]);
      return out.map((p) => p.join(',')).join(' ');
    }
    // a board pin whose wire would have to cross the board: go around it
    const bb = this.boardBox;
    const around = (p, q) => bb && p.side && ((p.side === 'left' && q.x > bb.x0) || (p.side === 'right' && q.x < bb.x1));
    if (around(pa, pb) || around(pb, pa)) {
      const [p, q, rev] = around(pa, pb) ? [pa, pb, false] : [pb, pa, true];
      const off = 10 + (lane % 10) * 5;
      const px = p.side === 'left' ? bb.x0 - 7 - off : bb.x1 + 7 + off;
      const below = Math.abs(p.y - bb.y1) + Math.abs(q.y - bb.y1) < Math.abs(p.y - bb.y0) + Math.abs(q.y - bb.y0);
      const py = below ? bb.y1 + 10 + (lane % 10) * 5 : bb.y0 - 10 - (lane % 10) * 5;
      const qx = q.x + exit(q);
      let path = [[p.x, p.y], [px, p.y], [px, py], [qx, py], [qx, q.y], [q.x, q.y]];
      if (q.side && around(q, p)) {
        // both ends on the board, on opposite sides
        const qx2 = q.side === 'left' ? bb.x0 - 7 - off : bb.x1 + 7 + off;
        path = [[p.x, p.y], [px, p.y], [px, py], [qx2, py], [qx2, q.y], [q.x, q.y]];
      }
      if (rev) path.reverse();
      return path.map((p) => p.join(',')).join(' ');
    }
    out.push([ax, pa.y]);
    const mx = pa.side || pb.side ? (pa.side ? bx : ax) : (ax + bx) / 2;
    out.push([mx, pa.y], [mx, pb.y]);
    out.push([bx, pb.y]);
    out.push([pb.x, pb.y]);
    return out.map((p) => p.join(',')).join(' ');
  }

  renderWires() {
    this.layerWires.innerHTML = '';
    this.connList().forEach((w, i) => {
      let pa = this.endpointPos(w.a), pb = this.endpointPos(w.b);
      if (!pa || !pb) return;
      pa = this.endpointPos(w.a, pb) || pa;
      pb = this.endpointPos(w.b, pa) || pb;
      const color = w.color || this.wireColor(w.a, w.b, i);
      const pl = svg('polyline', { class: 'wire', points: this.route(pa, pb, w.points, i), stroke: color });
      const sel = this.selected && this.selected.type === 'wire' && this.selected.index === w.index;
      if (sel) pl.classList.add('selected');
      pl.addEventListener('pointerdown', (e) => { e.stopPropagation(); this.select({ type: 'wire', index: w.index }); });
      const title = svg('title', {}, `${w.a} ↔ ${w.b}`);
      pl.append(title);
      this.layerWires.append(pl);
    });
  }

  startWire(from, abs) {
    this.wiring = { from, abs, points: [] };
    this.preview = svg('polyline', { class: 'wire-preview' });
    this.layerTop.append(this.preview);
    document.getElementById('circuit-hint').classList.add('active');
    document.getElementById('circuit-hint').textContent = `Wiring from ${from} — click a pin to connect, click empty space for a bend, Esc to cancel`;
  }

  drawPreview(p) {
    if (!this.preview) return;
    const pts = [[this.wiring.abs.x, this.wiring.abs.y], ...this.wiring.points, [p.x, p.y]];
    this.preview.setAttribute('points', pts.map((q) => q.join(',')).join(' '));
  }

  cancelWire() {
    this.wiring = null;
    if (this.preview) this.preview.remove();
    this.preview = null;
    document.getElementById('circuit-hint').classList.remove('active');
    document.getElementById('circuit-hint').textContent = 'Click a pin to start a wire · drag parts · Del removes the selection';
  }

  finishWire(to) {
    const w = this.wiring;
    this.cancelWire();
    if (!w || w.from === to) return;
    const conn = { from: w.from, to };
    if (w.points.length) conn.points = w.points;
    this.project.connections.push(conn);
    this.renderWires();
    this.changed(true);
  }

  // ------------------------------------------------------------ editing
  select(sel) {
    this.selected = sel;
    for (const [id, p] of this.parts) p.g.classList.toggle('selected', !!sel && sel.type === 'part' && sel.id === id);
    this.renderWires();
    bus.emit('select', sel);
  }

  deleteSelected() {
    const s = this.selected;
    if (!s) return;
    if (s.type === 'wire') {
      this.project.connections.splice(s.index, 1);
    } else if (s.type === 'part') {
      this.project.components = this.project.components.filter((c) => c.id !== s.id);
      const re = new RegExp(`^${s.id.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\.`, 'i');
      this.project.connections = this.project.connections.filter((c) => {
        const eps = Array.isArray(c) ? c : typeof c === 'string' ? c.split(/\s*(?:->| - )\s*/) : [c.from, c.to];
        return !eps.some((e) => re.test(e));
      });
    }
    this.select(null);
    this.render();
    this.changed(true);
  }

  rotateSelected() {
    const c = this.project.components.find((x) => x.id === this.selected.id);
    if (!c) return;
    c.rotate = ((c.rotate || 0) + 90) % 360;
    this.render();
    this.changed(false);
  }

  addPart(kind, x, y) {
    const info = this.info(kind);
    const NICE = { resistor: 'r', capacitor: 'c', potentiometer: 'pot', pushbutton: 'btn', 'slide-switch': 'sw', switch: 'sw', 'rgb-led': 'rgb',
      'keypad-4x4': 'keypad', 'keypad-4x3': 'keypad', 'rotary-encoder': 'enc', ssd1306: 'oled', sh1106: 'oled', 'ssd1306-spi': 'oled', 'sh1106-spi': 'oled',
      'lcd1602-i2c': 'lcd', 'lcd2004-i2c': 'lcd', 'lcd-parallel': 'lcd', ili9341: 'tft', st7735: 'tft', st7789: 'tft', ws2812: 'strip', dht11: 'dht', dht22: 'dht',
      'hc-sr04': 'sonar', mpu6050: 'imu', mpu9250: 'imu', 'gy-86': 'imu', 'gy-87': 'imu', 'gy-91': 'imu', bmp280: 'baro', bme280: 'bme', bmp180: 'baro', ms5611: 'baro',
      ds3231: 'rtc', ds1307: 'rtc', 'buzzer-active': 'buzzer', 'ir-receiver': 'ir', 'sd-card': 'sd', '74hc595': 'sr', 'i2c-relay': 'relays', '7segment': 'seg', ds18b20: 'temp',
      mfrc522: 'rfid', pn532: 'nfc', hx711: 'scale', l298n: 'motors', a4988: 'stepper', uln2003: 'stepper', 'soil-moisture': 'soil', 'water-level': 'water' };
    const base = NICE[kind] || kind.replace(/[^a-z0-9]/gi, '').replace(/^(\d)/, 'p$1').slice(0, 10) || 'part';
    let n = 1;
    while (this.project.components.some((c) => c.id === `${base}${n}`)) n++;
    const c = { id: `${base}${n}`, type: kind, ...(info ? info.props : {}), x: Math.round(x / 5) * 5, y: Math.round(y / 5) * 5 };
    this.project.components.push(c);
    this.render();
    this.select({ type: 'part', id: c.id });
    this.changed(true);
    toast(`Added ${c.id} — click its pins to wire it`);
  }

  addPartCentered(kind) {
    const r = this.svg.getBoundingClientRect();
    let p = this.toLocal({ clientX: r.left + r.width * 0.6, clientY: r.top + r.height * 0.4 });
    const bb = this.boardBox;
    // next to the board rather than on top of it
    if (bb) p = { x: bb.x1 + 110, y: bb.y0 + 90 };
    let { x, y } = p;
    // size of the new part (rendered off-screen) and of the parts already placed
    let size = { ox: -20, oy: -20, w: 40, h: 40 };
    try { const r = renderPart(kind, { ...((this.info(kind) || {}).props || {}) }, { id: '', input() {} }, this.info(kind)); size = { ox: r.ox || 0, oy: r.oy || 0, w: r.w, h: r.h }; } catch (e) { /* default size */ }
    const rects = [...this.parts.values()].map((pt) => ({ x0: pt.cfg.x + (pt.r.ox || 0) - 25, y0: pt.cfg.y + (pt.r.oy || 0) - 30, x1: pt.cfg.x + (pt.r.ox || 0) + pt.r.w + 25, y1: pt.cfg.y + (pt.r.oy || 0) + pt.r.h + 25 }));
    if (bb) rects.push({ x0: bb.x0 - 40, y0: bb.y0 - 30, x1: bb.x1 + 40, y1: bb.y1 + 30 });
    const busy = (x, y) => {
      const a = { x0: x + size.ox, y0: y + size.oy, x1: x + size.ox + size.w, y1: y + size.oy + size.h };
      return rects.some((b) => a.x0 < b.x1 && a.x1 > b.x0 && a.y0 < b.y1 && a.y1 > b.y0);
    };
    // don't drop new parts on top of each other: scan rows to the right of the board
    for (let i = 0; i < 200 && busy(x, y); i++) {
      x += 40;
      if (x > p.x + 700) { x = p.x; y += 60; }
    }
    this.addPart(kind, x, y);
  }

  fit() {
    const bb = this.layerParts.getBBox();
    const r = this.svg.getBoundingClientRect();
    if (!bb.width || !r.width) return;
    const k = Math.min((r.width - 60) / bb.width, (r.height - 60) / bb.height, 3);
    this.view.k = k;
    this.view.x = (r.width - bb.width * k) / 2 - bb.x * k;
    this.view.y = (r.height - bb.height * k) / 2 - bb.y * k;
    this.applyView();
  }

  zoom(f) {
    const r = this.svg.getBoundingClientRect();
    const mx = r.width / 2, my = r.height / 2;
    const k2 = Math.max(0.3, Math.min(6, this.view.k * f));
    this.view.x = mx - ((mx - this.view.x) * k2) / this.view.k;
    this.view.y = my - ((my - this.view.y) * k2) / this.view.k;
    this.view.k = k2;
    this.applyView();
  }

  // electrical = true -> simulation is rebuilt
  changed(electrical) {
    const snap = this.snapshot();
    if (this.history && snap !== this.lastSnap) {
      this.history.push(this.lastSnap);
      if (this.history.length > 200) this.history.shift();
      this.future = [];
      this.lastSnap = snap;
    }
    clearTimeout(this.dirtyTimer);
    this.dirtyTimer = setTimeout(() => this.save(electrical), electrical ? 150 : 600);
  }

  async save(reload) {
    try {
      const body = { ...this.project, reload };
      delete body.yaml; delete body.path;
      await api('/api/project', { method: 'PUT', json: body });
      bus.emit('project-saved', reload);
    } catch (e) {
      toast('Circuit not saved: ' + e.message, true);
    }
  }

  // ------------------------------------------------------------ live state
  updateStates(board) {
    if (!board) return;
    this.states = board;
    this.pads = board.pads || {};
    const comps = board.components || [];
    for (const c of comps) {
      if (c.id === 'board_led') {
        if (this.ioLed) { const b = c.state.brightness || 0; this.ioLed.setAttribute('fill', b > 0.05 ? '#40a0ff' : '#0d2a4a'); this.ioLed.setAttribute('filter', b > 0.05 ? 'url(#glow-small)' : null); }
        continue;
      }
      const p = this.parts.get(c.id);
      if (p && p.r.update) { try { p.r.update(c.state || {}, c); } catch (e) { console.warn(e); } }
    }
  }
}
