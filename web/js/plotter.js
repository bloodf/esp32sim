// Serial plotter (like the Arduino IDE's): every serial line with numbers becomes
// a sample. Accepts "12 34 56", "1.5,2.5", "temp:21.5 hum:40" and "temp=21.5".
import { el } from './api.js';

const COLORS = ['#4a9eff', '#ff7043', '#66bb6a', '#fdd835', '#ab47bc', '#26c6da', '#ec407a', '#8d6e63'];

export class Plotter {
  constructor(root) {
    this.root = root;
    this.series = new Map(); // name -> { values: [], color }
    this.max = 500;
    this.paused = false;
    this.partial = '';
    this.count = 0;
    this.render();
  }

  render() {
    this.root.innerHTML = '';
    this.legend = el('div', { class: 'plot-legend' });
    const bar = el('div', { class: 'plot-bar' },
      this.legend,
      el('div', { class: 'grow' }),
      el('label', { class: 'muted' }, 'points ', (this.maxSel = el('select', { onchange: () => { this.max = parseInt(this.maxSel.value, 10); this.draw(); } },
        ...[100, 250, 500, 1000, 2000].map((n) => el('option', { value: n, selected: n === this.max }, String(n)))))),
      (this.pauseBtn = el('button', { onclick: () => { this.paused = !this.paused; this.pauseBtn.textContent = this.paused ? '▶ Resume' : '⏸ Pause'; } }, '⏸ Pause')),
      el('button', { onclick: () => { this.series.clear(); this.count = 0; this.draw(); } }, '🧹 Clear'));
    this.canvas = el('canvas', { class: 'plot-canvas' });
    this.hint = el('div', { class: 'plot-hint muted' }, 'Print numbers on a line to plot them, e.g. Serial.printf("%d %d\\n", a, b) or "temp:21.5 hum:40".');
    this.root.append(bar, this.canvas, this.hint);
    new ResizeObserver(() => this.draw()).observe(this.canvas);
  }

  /** Feed raw serial text. */
  feed(text) {
    this.partial += text;
    let nl;
    while ((nl = this.partial.search(/\r?\n/)) >= 0) {
      const line = this.partial.slice(0, nl);
      this.partial = this.partial.slice(nl + (this.partial[nl] === '\r' ? 2 : 1));
      this.line(line);
    }
    if (this.partial.length > 1000) this.partial = '';
  }

  line(line) {
    if (this.paused) return;
    const t = line.trim();
    if (!t) return;
    const vals = [];
    // labelled values first: name:value or name=value
    const re = /([A-Za-z_][\w.-]*)\s*[:=]\s*(-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?)/g;
    const lab = [...t.matchAll(re)];
    // a data line holds only label:value pairs (so ordinary log text is not plotted)
    if (lab.length && !/[^\s,;|\t]/.test(t.replace(re, ''))) lab.forEach((m) => vals.push([m[1], parseFloat(m[2])]));
    else if (lab.length) return;
    else {
      const parts = t.split(/[\s,;\t]+/);
      if (!parts.every((p) => /^-?\d+(\.\d+)?([eE][-+]?\d+)?$/.test(p))) return; // not a data line
      parts.forEach((p, i) => vals.push([`value ${i + 1}`, parseFloat(p)]));
    }
    if (!vals.length) return;
    for (const [name, v] of vals) {
      if (!this.series.has(name)) this.series.set(name, { values: [], color: COLORS[this.series.size % COLORS.length] });
      const s = this.series.get(name);
      s.values.push(v);
      if (s.values.length > 4000) s.values.splice(0, s.values.length - 4000);
    }
    this.count++;
    if (!this.pending) { this.pending = true; requestAnimationFrame(() => { this.pending = false; this.draw(); }); }
  }

  draw() {
    const c = this.canvas;
    const w = c.clientWidth, h = c.clientHeight;
    if (!w || !h) return;
    const dpr = window.devicePixelRatio || 1;
    if (c.width !== w * dpr || c.height !== h * dpr) { c.width = w * dpr; c.height = h * dpr; }
    const g = c.getContext('2d');
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.fillStyle = '#111214';
    g.fillRect(0, 0, w, h);
    this.hint.style.display = this.series.size ? 'none' : '';
    this.legend.innerHTML = '';
    if (!this.series.size) return;
    let lo = Infinity, hi = -Infinity;
    for (const s of this.series.values()) for (const v of s.values.slice(-this.max)) { lo = Math.min(lo, v); hi = Math.max(hi, v); }
    if (lo === hi) { lo -= 1; hi += 1; }
    const pad = (hi - lo) * 0.08; lo -= pad; hi += pad;
    const L = 52, R = 10, T = 8, B = 18;
    const y = (v) => T + (h - T - B) * (1 - (v - lo) / (hi - lo));
    // grid + axis labels
    g.strokeStyle = '#2a2d33'; g.fillStyle = '#8a8f98'; g.font = '11px monospace'; g.lineWidth = 1;
    for (let i = 0; i <= 4; i++) {
      const v = lo + (hi - lo) * i / 4, yy = y(v);
      g.beginPath(); g.moveTo(L, yy); g.lineTo(w - R, yy); g.stroke();
      g.fillText(fmt(v), 4, yy + 4);
    }
    const n = this.max;
    const x = (i) => L + (w - L - R) * i / Math.max(1, n - 1);
    for (const [name, s] of this.series) {
      const vals = s.values.slice(-n);
      const off = n - vals.length;
      g.strokeStyle = s.color; g.lineWidth = 1.5; g.beginPath();
      vals.forEach((v, i) => { const xx = x(i + off), yy = y(v); if (i) g.lineTo(xx, yy); else g.moveTo(xx, yy); });
      g.stroke();
      this.legend.append(el('span', { class: 'plot-key' }, el('i', { style: { background: s.color } }), `${name} `, el('b', {}, fmt(vals[vals.length - 1]))));
    }
  }
}

function fmt(v) {
  if (v === undefined) return '';
  const a = Math.abs(v);
  return a >= 1e5 || (a > 0 && a < 1e-2) ? v.toExponential(2) : (Math.round(v * 100) / 100).toString();
}
