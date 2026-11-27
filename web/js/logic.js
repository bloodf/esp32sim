// Logic analyzer: level changes of the ESP32 pins wired into the circuit,
// recorded by the simulator (pin-level traffic: GPIO, bit-banged protocols,
// PWM transitions the circuit sees). Scroll to zoom, drag to pan, click to
// place a cursor; the distance between two cursors is shown.
import { api, el } from './api.js';

const WINDOWS = [['10 µs', 1e4], ['100 µs', 1e5], ['1 ms', 1e6], ['10 ms', 1e7], ['100 ms', 1e8], ['1 s', 1e9], ['10 s', 1e10]];

export class LogicAnalyzer {
  constructor(root) {
    this.root = root;
    this.next = 0;
    this.events = new Map(); // pad -> [[t, level], ...]
    this.pads = [];
    this.shown = new Set(JSON.parse(localStorage.getItem('esp32sim.logic.pads') || '[]'));
    this.now = 0;
    this.span = 1e7;
    this.end = null; // null = follow live
    this.cursors = [];
    this.visible = false;
    this.render();
    setInterval(() => this.visible && !this.frozen && this.poll(), 250);
  }

  reset() { this.next = 0; this.events.clear(); this.end = null; this.cursors = []; }

  render() {
    this.root.innerHTML = '';
    this.padBox = el('div', { class: 'la-pads' });
    const winSel = el('select', { onchange: () => { this.span = parseFloat(winSel.value); this.draw(); } },
      ...WINDOWS.map(([n, v]) => el('option', { value: v, selected: v === this.span }, n)));
    this.winSel = winSel;
    this.liveBtn = el('button', { onclick: () => { this.end = null; this.frozen = false; this.liveBtn.classList.add('primary'); this.draw(); } }, '● Live');
    this.liveBtn.classList.add('primary');
    this.freezeBtn = el('button', { onclick: () => { this.frozen = !this.frozen; this.freezeBtn.textContent = this.frozen ? '▶ Capture' : '⏸ Hold'; } }, '⏸ Hold');
    this.info = el('span', { class: 'muted la-info' });
    this.root.append(el('div', { class: 'plot-bar' }, el('span', { class: 'muted' }, 'window'), winSel, this.liveBtn, this.freezeBtn,
      el('button', { onclick: () => { this.cursors = []; this.draw(); } }, 'clear cursors'), this.info, el('div', { class: 'grow' }), this.padBox));
    this.canvas = el('canvas', { class: 'plot-canvas la-canvas' });
    this.root.append(this.canvas);
    new ResizeObserver(() => this.draw()).observe(this.canvas);
    // zoom / pan / cursors
    this.canvas.addEventListener('wheel', (e) => {
      e.preventDefault();
      const f = e.deltaY > 0 ? 1.5 : 1 / 1.5;
      const { t0, t1 } = this.range();
      const tx = this.tAt(e.offsetX);
      this.span = Math.min(1e11, Math.max(1e3, this.span * f));
      const frac = (tx - t0) / (t1 - t0);
      this.end = tx + (1 - frac) * this.span;
      this.liveBtn.classList.remove('primary');
      this.draw();
    }, { passive: false });
    let drag = null;
    this.canvas.addEventListener('pointerdown', (e) => { drag = { x: e.offsetX, end: this.range().t1, moved: false }; this.canvas.setPointerCapture(e.pointerId); });
    this.canvas.addEventListener('pointermove', (e) => {
      if (!drag) return;
      const dx = e.offsetX - drag.x;
      if (Math.abs(dx) > 3) drag.moved = true;
      if (drag.moved) {
        this.end = drag.end - dx * this.span / Math.max(1, this.plotW());
        this.liveBtn.classList.remove('primary');
        this.draw();
      }
    });
    this.canvas.addEventListener('pointerup', (e) => {
      if (drag && !drag.moved) { this.cursors.push(this.tAt(e.offsetX)); if (this.cursors.length > 2) this.cursors.shift(); this.draw(); }
      drag = null;
    });
  }

  show(v) { this.visible = v; if (v) this.poll(); }

  async poll() {
    let r;
    try { r = await api('/api/trace?since=' + this.next); } catch (e) { return; }
    if (!r || r.now === undefined) return;
    if (r.now < this.now) this.reset(); // simulation restarted
    this.now = r.now;
    if (this.next === 0 && r.levels) {
      // starting point: current levels at time 0 of our view
      r.levels.forEach((l, pad) => { if (l !== null && r.pads.includes(pad)) this.events.set(pad, [[0, l]]); });
    }
    for (const [t, pad, l] of r.events) {
      if (!this.events.has(pad)) this.events.set(pad, []);
      const arr = this.events.get(pad);
      arr.push([t, l]);
      if (arr.length > 200000) arr.splice(0, arr.length - 150000);
    }
    this.next = r.next;
    if (JSON.stringify(r.pads) !== JSON.stringify(this.pads)) {
      this.pads = r.pads;
      if (!this.shown.size) r.pads.slice(0, 8).forEach((p) => this.shown.add(p));
      this.renderPads();
    }
    this.draw();
  }

  renderPads() {
    this.padBox.innerHTML = '';
    for (const p of this.pads) {
      const cb = el('input', { type: 'checkbox', checked: this.shown.has(p), onchange: () => {
        if (cb.checked) this.shown.add(p); else this.shown.delete(p);
        localStorage.setItem('esp32sim.logic.pads', JSON.stringify([...this.shown]));
        this.draw();
      } });
      this.padBox.append(el('label', { class: 'la-pad' }, cb, `GPIO${p}`));
    }
  }

  plotW() { return this.canvas.clientWidth - 70; }
  range() { const t1 = this.end === null ? this.now : this.end; return { t0: t1 - this.span, t1 }; }
  tAt(x) { const { t0, t1 } = this.range(); return t0 + (x - 62) / this.plotW() * (t1 - t0); }

  draw() {
    const c = this.canvas;
    const w = c.clientWidth, h = c.clientHeight;
    if (!w || !h) return;
    const dpr = window.devicePixelRatio || 1;
    if (c.width !== w * dpr || c.height !== h * dpr) { c.width = w * dpr; c.height = h * dpr; }
    const g = c.getContext('2d');
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.fillStyle = '#111214'; g.fillRect(0, 0, w, h);
    const pads = this.pads.filter((p) => this.shown.has(p));
    const { t0, t1 } = this.range();
    const X = (t) => 62 + (t - t0) / (t1 - t0) * this.plotW();
    // time grid
    g.font = '10px monospace'; g.fillStyle = '#8a8f98'; g.strokeStyle = '#24262b';
    const step = niceStep((t1 - t0) / 8);
    for (let t = Math.ceil(t0 / step) * step; t <= t1; t += step) {
      const x = X(t);
      g.beginPath(); g.moveTo(x, 0); g.lineTo(x, h - 14); g.stroke();
      g.fillText(fmtT(t), x + 2, h - 3);
    }
    const rowH = Math.max(18, Math.min(40, (h - 20) / Math.max(1, pads.length)));
    pads.forEach((p, i) => {
      const top = 4 + i * rowH, hi = top + 3, lo = top + rowH - 5;
      g.fillStyle = '#b0bec5'; g.fillText(`GPIO${p}`, 4, top + rowH / 2 + 3);
      const ev = this.events.get(p) || [];
      // level at t0
      let idx = lowerBound(ev, t0) - 1;
      let level = idx >= 0 ? ev[idx][1] : (ev.length ? ev[0][1] : null);
      let x = 62;
      g.strokeStyle = ['#ff7043', '#4a9eff', '#66bb6a', '#fdd835', '#ab47bc', '#26c6da'][i % 6];
      g.lineWidth = 1.3;
      g.beginPath();
      const yOf = (l) => (l === null ? (hi + lo) / 2 : l ? hi : lo);
      g.moveTo(x, yOf(level));
      let transitions = 0;
      for (let k = Math.max(0, idx + 1); k < ev.length && ev[k][0] <= t1; k++) {
        const xx = X(ev[k][0]);
        g.lineTo(xx, yOf(level));
        level = ev[k][1];
        g.lineTo(xx, yOf(level));
        transitions++;
      }
      g.lineTo(62 + this.plotW(), yOf(level));
      g.stroke();
      if (transitions > this.plotW() * 2) { g.fillStyle = '#ffffff18'; g.fillRect(62, hi, this.plotW(), lo - hi); }
    });
    // cursors
    g.strokeStyle = '#ffd54f'; g.setLineDash([4, 3]);
    for (const t of this.cursors) { const x = X(t); g.beginPath(); g.moveTo(x, 0); g.lineTo(x, h - 14); g.stroke(); }
    g.setLineDash([]);
    let txt = `t = ${fmtT(t1)}`;
    if (this.cursors.length === 2) {
      const d = Math.abs(this.cursors[1] - this.cursors[0]);
      txt += `   Δ = ${fmtT(d)}  (${d > 0 ? (1e9 / d).toPrecision(4) + ' Hz' : '—'})`;
    }
    this.info.textContent = pads.length ? txt : 'no pins selected';
  }
}

function lowerBound(arr, t) { let lo = 0, hi = arr.length; while (lo < hi) { const m = (lo + hi) >> 1; if (arr[m][0] < t) lo = m + 1; else hi = m; } return lo; }
function niceStep(x) { const p = Math.pow(10, Math.floor(Math.log10(x))); const f = x / p; return (f < 2 ? 2 : f < 5 ? 5 : 10) * p; }
function fmtT(ns) {
  const a = Math.abs(ns);
  if (a >= 1e9) return (ns / 1e9).toFixed(3) + ' s';
  if (a >= 1e6) return (ns / 1e6).toFixed(3) + ' ms';
  if (a >= 1e3) return (ns / 1e3).toFixed(2) + ' µs';
  return Math.round(ns) + ' ns';
}
