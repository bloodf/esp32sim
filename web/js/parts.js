// Visual models of parts for the circuit view: SVG drawing, pin anchors,
// live state updates and direct interaction.
import { svg, hexToBytes } from './api.js';

const LED_COLORS = { red: '#ff3b30', green: '#34c759', blue: '#3d7bff', yellow: '#ffd60a', orange: '#ff9f0a', amber: '#ffb300', white: '#f5f5f5', purple: '#bf5af2', violet: '#bf5af2', pink: '#ff6ea8', ir: '#6b1d1d', infrared: '#6b1d1d' };
const BAND = ['#111', '#7b3f00', '#e53935', '#fb8c00', '#fdd835', '#43a047', '#1e88e5', '#8e24aa', '#9e9e9e', '#fafafa'];

function parseSI(v, dflt) {
  if (typeof v === 'number') return v;
  if (!v) return dflt;
  const s = String(v).trim().replace(/[Ωohm]/gi, '').replace(/[FRV]$/i, '');
  const m = s.match(/^([\d.]+)\s*([kKMmunp])?([\d]*)$/);
  if (!m) return parseFloat(s) || dflt;
  const mult = { k: 1e3, K: 1e3, M: 1e6, m: 1e-3, u: 1e-6, n: 1e-9, p: 1e-12 }[m[2]] || 1;
  const num = m[3] ? parseFloat(m[1] + '.' + m[3]) : parseFloat(m[1]);
  return num * mult;
}

function resistorBands(ohms) {
  if (!(ohms > 0)) return [0, 0, 0];
  let exp = Math.floor(Math.log10(ohms)) - 1;
  let sig = Math.round(ohms / Math.pow(10, exp));
  if (sig >= 100) { sig = Math.round(sig / 10); exp++; }
  return [Math.floor(sig / 10), sig % 10, Math.max(0, Math.min(9, exp))];
}

// Generic "module" PCB with a row of header pins.
function moduleBox(o) {
  const { w, h, color = '#1d5c8f', title = '', pins = [], side = 'bottom', spacing = 11 } = o;
  const g = svg('g');
  g.append(svg('rect', { x: 0, y: 0, width: w, height: h, rx: 3, fill: color, stroke: '#0006', 'stroke-width': 1 }));
  // mounting holes
  if (o.holes !== false && w > 40 && h > 30) {
    for (const [hx, hy] of [[5, 5], [w - 5, 5], [5, h - 5], [w - 5, h - 5]]) g.append(svg('circle', { cx: hx, cy: hy, r: 2, fill: '#0005', stroke: '#c9a22766' }));
  }
  const pinPos = [];
  const n = pins.length;
  pins.forEach((p, i) => {
    let x, y, tx, ty, rot = 0, anchor = 'middle';
    if (side === 'bottom') { x = w / 2 + (i - (n - 1) / 2) * spacing; y = h + 6; tx = x; ty = h - 4; rot = -90; anchor = 'start'; }
    else if (side === 'top') { x = w / 2 + (i - (n - 1) / 2) * spacing; y = -6; tx = x; ty = 4; rot = -90; anchor = 'end'; }
    else if (side === 'left') { x = -6; y = h / 2 + (i - (n - 1) / 2) * spacing; tx = 4; ty = y + 2.5; anchor = 'start'; }
    else { x = w + 6; y = h / 2 + (i - (n - 1) / 2) * spacing; tx = w - 4; ty = y + 2.5; anchor = 'end'; }
    // header pin
    const hx = side === 'left' ? 0 : side === 'right' ? w : x;
    const hy = side === 'top' ? 0 : side === 'bottom' ? h : y;
    g.append(svg('line', { x1: hx, y1: hy, x2: x, y2: y, stroke: '#bbb', 'stroke-width': 2 }));
    const t = svg('text', { x: tx, y: ty, class: 'pin-label', 'text-anchor': anchor, transform: rot ? `rotate(${rot} ${tx} ${ty})` : null }, p);
    if (rot) t.setAttribute('dominant-baseline', 'middle');
    g.append(t);
    pinPos.push({ name: p, x, y });
  });
  if (title) g.append(svg('text', { x: w / 2, y: side === 'top' ? h - 5 : 11, class: 'part-title', 'text-anchor': 'middle' }, title));
  return { g, pins: pinPos, w, h };
}

function text(x, y, s, attrs = {}) {
  return svg('text', { x, y, 'font-size': attrs.size || 8, fill: attrs.fill || '#e0e0e0', 'text-anchor': attrs.anchor || 'middle', 'font-family': attrs.mono ? 'monospace' : 'sans-serif', 'pointer-events': 'none', ...attrs.extra }, s);
}

function pixelCanvas(g, x, y, w, h, pw, ph) {
  const fo = svg('foreignObject', { x, y, width: w, height: h });
  const c = document.createElement('canvas');
  c.width = pw; c.height = ph;
  c.style.width = w + 'px'; c.style.height = h + 'px';
  c.style.imageRendering = 'pixelated';
  c.style.display = 'block';
  fo.append(c);
  g.append(fo);
  return c;
}

const SEGS = { // 7-seg segment polygons in a 20x34 box
  a: 'M3,1 L17,1 L15,4 L5,4 Z', b: 'M17.5,2 L18.5,3 L18,16 L16,15 L16.5,4.5 Z', c: 'M18,18 L18.5,31 L17,32 L15.5,29 L16,19 Z',
  d: 'M3,33 L17,33 L15,30 L5,30 Z', e: 'M2,18 L4,19 L3.5,29.5 L1.5,32 L1,31 Z', f: 'M2.5,2 L4.5,4.5 L4,15 L2,16 L1.5,3 Z', g: 'M3,17 L5,15.5 L15,15.5 L17,17 L15,18.5 L5,18.5 Z',
};
function sevenSeg(g, x, y, color = '#ff3b30') {
  const parts = {};
  const d = svg('g', { transform: `translate(${x},${y})` });
  for (const k of 'abcdefg') { const p = svg('path', { d: SEGS[k], fill: '#3a1a1a' }); d.append(p); parts[k] = p; }
  const dp = svg('circle', { cx: 21, cy: 32, r: 1.6, fill: '#3a1a1a' }); d.append(dp); parts.dp = dp;
  g.append(d);
  return (bits) => {
    'abcdefg'.split('').forEach((k, i) => parts[k].setAttribute('fill', bits & (1 << i) ? color : '#3a1a1a'));
    parts.dp.setAttribute('fill', bits & 0x80 ? color : '#3a1a1a');
  };
}

function bindPress(el, ctx, downAction = { action: 'press' }, upAction = { action: 'release' }) {
  el.classList.add('interactive');
  let held = false;
  el.addEventListener('pointerdown', (e) => { if (e.button !== 0) return; e.stopPropagation(); held = true; el.setPointerCapture(e.pointerId); ctx.input(downAction); });
  const up = (e) => { if (!held) return; held = false; e.stopPropagation(); ctx.input(upAction); };
  el.addEventListener('pointerup', up);
  el.addEventListener('pointercancel', up);
}

function bindClick(el, ctx, f) {
  el.classList.add('interactive');
  el.addEventListener('pointerdown', (e) => e.stopPropagation());
  el.addEventListener('click', (e) => { e.stopPropagation(); f(); });
}

// ---------------------------------------------------------------- renderers
const R = {};

R.led = (p, ctx) => {
  const col = LED_COLORS[(p.color || 'red').toLowerCase()] || p.color || '#ff3b30';
  const g = svg('g');
  const glow = svg('circle', { cx: 0, cy: 0, r: 22, fill: col, opacity: 0, filter: 'url(#glow)' });
  g.append(glow);
  g.append(svg('line', { x1: -5, y1: 10, x2: -5, y2: 24, stroke: '#aaa', 'stroke-width': 1.6 }));
  g.append(svg('line', { x1: 5, y1: 10, x2: 5, y2: 20, stroke: '#aaa', 'stroke-width': 1.6 }));
  g.append(svg('rect', { x: -11, y: 6, width: 22, height: 4, rx: 1, fill: col, opacity: 0.85 }));
  const dome = svg('path', { d: 'M-9,7 L-9,-3 A9,9 0 0 1 9,-3 L9,7 Z', fill: col, opacity: 0.35, stroke: '#0005' });
  g.append(dome);
  g.append(svg('path', { d: 'M-5,-6 A5,5 0 0 1 0,-10', stroke: '#fff8', fill: 'none', 'stroke-width': 1.5 }));
  const x = svg('text', { x: 0, y: 3, 'font-size': 12, fill: '#000', 'text-anchor': 'middle', opacity: 0 }, '✕');
  g.append(x);
  return {
    g, w: 24, h: 40, ox: -12, oy: -14,
    pins: [{ name: 'A', x: -5, y: 24 }, { name: 'C', x: 5, y: 20 }],
    update(s) {
      const b = s.brightness || 0;
      dome.setAttribute('opacity', 0.35 + 0.65 * b);
      glow.setAttribute('opacity', 0.75 * b);
      x.setAttribute('opacity', s.burned ? 1 : 0);
      if (s.burned) dome.setAttribute('fill', '#555');
    },
  };
};

R['rgb-led'] = (p) => {
  const g = svg('g');
  const glow = svg('circle', { cx: 0, cy: 0, r: 22, fill: '#000', opacity: 0, filter: 'url(#glow)' });
  g.append(glow);
  [-9, -3, 3, 9].forEach((x, i) => g.append(svg('line', { x1: x, y1: 10, x2: x, y2: i === 1 ? 26 : 22, stroke: '#aaa', 'stroke-width': 1.4 })));
  const dome = svg('path', { d: 'M-11,8 L-11,-3 A11,11 0 0 1 11,-3 L11,8 Z', fill: '#ddd', opacity: 0.5, stroke: '#0005' });
  g.append(dome);
  return {
    g, w: 26, h: 42, ox: -13, oy: -15,
    pins: [{ name: 'R', x: -9, y: 22 }, { name: 'COM', x: -3, y: 26 }, { name: 'G', x: 3, y: 22 }, { name: 'B', x: 9, y: 22 }],
    update(s) {
      const c = `rgb(${Math.round(s.r * 255)},${Math.round(s.g * 255)},${Math.round(s.b * 255)})`;
      const m = Math.max(s.r, s.g, s.b);
      dome.setAttribute('fill', m > 0.02 ? c : '#ddd');
      dome.setAttribute('opacity', 0.5 + 0.5 * m);
      glow.setAttribute('fill', c);
      glow.setAttribute('opacity', 0.7 * m);
    },
  };
};

R.resistor = (p) => {
  const ohms = parseSI(p.value, 1000);
  const [b1, b2, m] = resistorBands(ohms);
  const g = svg('g');
  g.append(svg('line', { x1: -26, y1: 0, x2: 26, y2: 0, stroke: '#aaa', 'stroke-width': 1.6 }));
  g.append(svg('rect', { x: -15, y: -5, width: 30, height: 10, rx: 4, fill: '#d8c08a', stroke: '#8a7346', 'stroke-width': 0.6 }));
  [[-9, BAND[b1]], [-4, BAND[b2]], [1, BAND[m]], [9, '#c9a227']].forEach(([x, c]) => g.append(svg('rect', { x, y: -5, width: 2.6, height: 10, fill: c })));
  const label = text(0, -9, formatOhms(ohms), { size: 7, fill: '#bbb' });
  g.append(label);
  return { g, w: 56, h: 22, ox: -28, oy: -13, pins: [{ name: '1', x: -26, y: 0 }, { name: '2', x: 26, y: 0 }] };
};
function formatOhms(v) { return v >= 1e6 ? +(v / 1e6).toFixed(2) + 'MΩ' : v >= 1e3 ? +(v / 1e3).toFixed(2) + 'kΩ' : +v.toFixed(1) + 'Ω'; }

R.capacitor = (p) => {
  const g = svg('g');
  g.append(svg('line', { x1: -4, y1: 6, x2: -4, y2: 20, stroke: '#aaa', 'stroke-width': 1.4 }));
  g.append(svg('line', { x1: 4, y1: 6, x2: 4, y2: 20, stroke: '#aaa', 'stroke-width': 1.4 }));
  g.append(svg('ellipse', { cx: 0, cy: 0, rx: 9, ry: 8, fill: '#e6a817', stroke: '#7a5a0c' }));
  g.append(text(0, 2.5, String(p.value || '100n'), { size: 6, fill: '#3a2a00' }));
  return { g, w: 22, h: 30, ox: -11, oy: -9, pins: [{ name: '1', x: -4, y: 20 }, { name: '2', x: 4, y: 20 }] };
};

R.potentiometer = (p, ctx) => {
  const g = svg('g');
  g.append(svg('rect', { x: -16, y: -16, width: 32, height: 30, rx: 3, fill: '#1565c0', stroke: '#0006' }));
  [-10, 0, 10].forEach((x) => g.append(svg('line', { x1: x, y1: 14, x2: x, y2: 24, stroke: '#aaa', 'stroke-width': 1.5 })));
  const knob = svg('g', { class: 'interactive' });
  knob.append(svg('circle', { cx: 0, cy: -1, r: 11, fill: '#cfd8dc', stroke: '#607d8b', 'stroke-width': 1.5 }));
  const ptr = svg('line', { x1: 0, y1: -1, x2: 0, y2: -11, stroke: '#263238', 'stroke-width': 2.4, 'stroke-linecap': 'round' });
  knob.append(ptr);
  g.append(knob);
  const val = text(0, -20, '', { size: 7, fill: '#9ecbff' });
  g.append(val);
  let pos = p.position ?? 0.5;
  const set = (v) => { pos = Math.max(0, Math.min(1, v)); ctx.input({ value: pos }); };
  knob.addEventListener('wheel', (e) => { e.preventDefault(); e.stopPropagation(); set(pos - Math.sign(e.deltaY) * 0.02); }, { passive: false });
  knob.addEventListener('pointerdown', (e) => {
    e.stopPropagation(); knob.setPointerCapture(e.pointerId);
    const y0 = e.clientY, p0 = pos;
    const mv = (ev) => set(p0 + (y0 - ev.clientY) / 150);
    const up = () => { knob.removeEventListener('pointermove', mv); knob.removeEventListener('pointerup', up); };
    knob.addEventListener('pointermove', mv); knob.addEventListener('pointerup', up);
  });
  return {
    g, w: 36, h: 44, ox: -18, oy: -24,
    pins: [{ name: '1', x: -10, y: 24 }, { name: 'W', x: 0, y: 24 }, { name: '2', x: 10, y: 24 }],
    update(s) { pos = s.position; ptr.setAttribute('transform', `rotate(${-135 + 270 * pos} 0 -1)`); val.textContent = Math.round(pos * 100) + '%'; },
  };
};

R.pushbutton = (p, ctx) => {
  const g = svg('g');
  g.append(svg('line', { x1: -20, y1: 0, x2: -12, y2: 0, stroke: '#aaa', 'stroke-width': 1.6 }));
  g.append(svg('line', { x1: 12, y1: 0, x2: 20, y2: 0, stroke: '#aaa', 'stroke-width': 1.6 }));
  g.append(svg('rect', { x: -12, y: -12, width: 24, height: 24, rx: 2, fill: '#37474f', stroke: '#000a' }));
  const cap = svg('circle', { cx: 0, cy: 0, r: 7.5, fill: '#263238', stroke: '#90a4ae', 'stroke-width': 1.2 });
  g.append(cap);
  bindPress(cap, ctx);
  return {
    g, w: 40, h: 26, ox: -20, oy: -13, pins: [{ name: '1', x: -20, y: 0 }, { name: '2', x: 20, y: 0 }],
    update(s) { cap.setAttribute('fill', s.pressed ? '#ef5350' : '#263238'); cap.setAttribute('r', s.pressed ? 6.8 : 7.5); },
  };
};

R.switch = (p, ctx) => {
  const g = svg('g');
  g.append(svg('rect', { x: -16, y: -8, width: 32, height: 16, rx: 3, fill: '#455a64', stroke: '#000a' }));
  const knob = svg('rect', { x: -13, y: -5, width: 12, height: 10, rx: 2, fill: '#eceff1' });
  g.append(knob);
  bindClick(g, ctx, () => ctx.input({ action: 'toggle' }));
  const pins = [{ name: '1', x: -10, y: 16 }, { name: '2', x: 10, y: 16 }];
  pins.forEach((pp) => g.append(svg('line', { x1: pp.x, y1: 8, x2: pp.x, y2: 16, stroke: '#aaa', 'stroke-width': 1.5 })));
  return { g, w: 34, h: 30, ox: -17, oy: -10, pins, update(s) { knob.setAttribute('x', s.on ? 1 : -13); knob.setAttribute('fill', s.on ? '#66bb6a' : '#eceff1'); } };
};

R['slide-switch'] = (p, ctx) => {
  const g = svg('g');
  g.append(svg('rect', { x: -18, y: -8, width: 36, height: 16, rx: 3, fill: '#455a64', stroke: '#000a' }));
  const knob = svg('rect', { x: -15, y: -5, width: 12, height: 10, rx: 2, fill: '#eceff1' });
  g.append(knob);
  bindClick(g, ctx, () => ctx.input({ action: 'toggle' }));
  const pins = [{ name: '1', x: -12, y: 16 }, { name: 'C', x: 0, y: 16 }, { name: '2', x: 12, y: 16 }];
  pins.forEach((pp) => g.append(svg('line', { x1: pp.x, y1: 8, x2: pp.x, y2: 16, stroke: '#aaa', 'stroke-width': 1.5 })));
  return { g, w: 38, h: 30, ox: -19, oy: -10, pins, update(s) { knob.setAttribute('x', s.on ? 3 : -15); } };
};

function keypad(p, ctx, cols) {
  const layout = cols === 4 ? ['123A', '456B', '789C', '*0#D'] : ['123', '456', '789', '*0#'];
  const cw = 26, pad = 8;
  const w = cols * cw + pad * 2, h = 4 * cw + pad * 2;
  const names = ['R1', 'R2', 'R3', 'R4', 'C1', 'C2', 'C3', 'C4'].slice(0, 4 + cols);
  const m = moduleBox({ w, h, color: '#202124', pins: names, side: 'bottom', spacing: 9, holes: false });
  const keys = {};
  layout.forEach((row, r) => [...row].forEach((k, c) => {
    const x = pad + c * cw + 2, y = pad + r * cw + 2;
    const kg = svg('g');
    const rect = svg('rect', { x, y, width: cw - 4, height: cw - 4, rx: 4, fill: /[A-D]/.test(k) ? '#c62828' : /[*#]/.test(k) ? '#1565c0' : '#eceff1' });
    kg.append(rect, text(x + (cw - 4) / 2, y + 15, k, { size: 10, fill: /[0-9]/.test(k) ? '#222' : '#fff' }));
    bindPress(kg, ctx, { key: k, action: 'press' }, { key: k, action: 'release' });
    m.g.append(kg);
    keys[k] = rect;
  }));
  return {
    ...m, update(s) {
      for (const [k, r] of Object.entries(keys)) r.setAttribute('stroke', (s.pressed || []).includes(k) ? '#4cf' : 'none'), r.setAttribute('stroke-width', 2.5);
    },
  };
}
R['keypad-4x4'] = (p, ctx) => keypad(p, ctx, 4);
R['keypad-4x3'] = (p, ctx) => keypad(p, ctx, 3);

R.joystick = (p, ctx) => {
  const m = moduleBox({ w: 70, h: 70, color: '#222', title: 'JOYSTICK', pins: ['VCC', 'VERT', 'HORZ', 'SEL', 'GND'], side: 'bottom', spacing: 11 });
  m.g.append(svg('circle', { cx: 35, cy: 38, r: 22, fill: '#333', stroke: '#555' }));
  const stick = svg('circle', { cx: 35, cy: 38, r: 11, fill: '#616161', stroke: '#9e9e9e', class: 'interactive' });
  m.g.append(stick);
  let x = 0.5, y = 0.5;
  stick.addEventListener('pointerdown', (e) => {
    e.stopPropagation(); stick.setPointerCapture(e.pointerId);
    const r = stick.getBoundingClientRect(); const cx0 = r.left + r.width / 2, cy0 = r.top + r.height / 2;
    const scale = r.width / 22;
    const mv = (ev) => { x = Math.max(0, Math.min(1, 0.5 + (ev.clientX - cx0) / (22 * scale))); y = Math.max(0, Math.min(1, 0.5 - (ev.clientY - cy0) / (22 * scale))); ctx.input({ x, y }); };
    const up = () => { stick.removeEventListener('pointermove', mv); stick.removeEventListener('pointerup', up); ctx.input({ x: 0.5, y: 0.5 }); };
    stick.addEventListener('pointermove', mv); stick.addEventListener('pointerup', up);
  });
  stick.addEventListener('dblclick', (e) => { e.stopPropagation(); ctx.input({ pressed: true }); setTimeout(() => ctx.input({ pressed: false }), 200); });
  return { ...m, update(s) { stick.setAttribute('cx', 35 + (s.x - 0.5) * 22); stick.setAttribute('cy', 38 - (s.y - 0.5) * 22); stick.setAttribute('fill', s.pressed ? '#ef5350' : '#616161'); } };
};

R['rotary-encoder'] = (p, ctx) => {
  const m = moduleBox({ w: 56, h: 56, color: '#1565c0', title: 'KY-040', pins: ['CLK', 'DT', 'SW', 'VCC', 'GND'], side: 'bottom', spacing: 10 });
  const knob = svg('g', { class: 'interactive' });
  knob.append(svg('circle', { cx: 28, cy: 30, r: 15, fill: '#424242', stroke: '#9e9e9e' }));
  const ptr = svg('line', { x1: 28, y1: 30, x2: 28, y2: 17, stroke: '#eee', 'stroke-width': 2 });
  knob.append(ptr);
  m.g.append(knob);
  knob.addEventListener('wheel', (e) => { e.preventDefault(); e.stopPropagation(); ctx.input({ steps: e.deltaY < 0 ? 1 : -1 }); }, { passive: false });
  bindPress(knob, ctx, { pressed: true }, { pressed: false });
  return { ...m, update(s) { ptr.setAttribute('transform', `rotate(${s.position * 18} 28 30)`); } };
};

function smallSensor(title, color, pins, extra) {
  return (p, ctx) => {
    const w = Math.max(40, pins.length * 11 + 10);
    const m = moduleBox({ w, h: 34, color, title, pins, side: 'bottom', spacing: 11 });
    const v = text(w / 2, 26, '', { size: 7, fill: '#fff' });
    m.g.append(v);
    if (extra) extra(m.g, w);
    return { ...m, update(s, c) { v.textContent = c.summary || ''; } };
  };
}

R.ldr = (p, ctx) => {
  const g = svg('g');
  g.append(svg('line', { x1: -4, y1: 8, x2: -4, y2: 20, stroke: '#aaa', 'stroke-width': 1.4 }));
  g.append(svg('line', { x1: 4, y1: 8, x2: 4, y2: 20, stroke: '#aaa', 'stroke-width': 1.4 }));
  g.append(svg('circle', { cx: 0, cy: 0, r: 9, fill: '#e8d7a8', stroke: '#8a7346' }));
  g.append(svg('path', { d: 'M-6,-3 h3 v6 h3 v-6 h3 v6 h3', stroke: '#b33', fill: 'none', 'stroke-width': 1.2 }));
  const v = text(0, -13, '', { size: 7, fill: '#ffd54f' });
  g.append(v);
  return { g, w: 22, h: 34, ox: -11, oy: -14, pins: [{ name: '1', x: -4, y: 20 }, { name: '2', x: 4, y: 20 }], update(s) { v.textContent = Math.round(s.lux) + ' lx'; } };
};
R.ntc = (p) => {
  const g = svg('g');
  g.append(svg('line', { x1: -3, y1: 6, x2: -3, y2: 20, stroke: '#aaa', 'stroke-width': 1.4 }));
  g.append(svg('line', { x1: 3, y1: 6, x2: 3, y2: 20, stroke: '#aaa', 'stroke-width': 1.4 }));
  g.append(svg('ellipse', { cx: 0, cy: 0, rx: 6, ry: 7, fill: '#263238' }));
  const v = text(0, -10, '', { size: 7, fill: '#ff8a65' });
  g.append(v);
  return { g, w: 18, h: 32, ox: -9, oy: -12, pins: [{ name: '1', x: -3, y: 20 }, { name: '2', x: 3, y: 20 }], update(s) { v.textContent = s.temperature.toFixed(1) + '°C'; } };
};
R.pir = (p, ctx) => {
  const m = moduleBox({ w: 50, h: 50, color: '#2e7d32', pins: ['VCC', 'OUT', 'GND'], side: 'bottom', spacing: 12 });
  const dome = svg('circle', { cx: 25, cy: 24, r: 17, fill: '#f5f5f5', stroke: '#bbb', class: 'interactive' });
  m.g.append(dome);
  bindClick(dome, ctx, () => ctx.input({ action: 'trigger' }));
  return { ...m, update(s) { dome.setAttribute('fill', s.active ? '#ffcc80' : '#f5f5f5'); } };
};
R['ir-obstacle'] = (p, ctx) => {
  const m = moduleBox({ w: 44, h: 30, color: '#1565c0', title: 'IR', pins: ['VCC', 'OUT', 'GND'], side: 'bottom', spacing: 11 });
  const led = svg('circle', { cx: 36, cy: 20, r: 3, fill: '#333', class: 'interactive' });
  m.g.append(led);
  bindClick(m.g, ctx, () => ctx.input({ action: 'toggle' }));
  return { ...m, update(s) { led.setAttribute('fill', s.active ? '#4caf50' : '#333'); } };
};
R.signal = (p, ctx) => {
  const m = moduleBox({ w: 50, h: 30, color: '#37474f', title: 'SIGNAL', pins: ['OUT', 'GND'], side: 'bottom', spacing: 14 });
  const v = text(25, 24, '', { size: 8, fill: '#80deea', mono: true });
  m.g.append(v);
  bindClick(m.g, ctx, () => ctx.input({ action: 'toggle' }));
  return { ...m, update(s) { v.textContent = s.volts.toFixed(2) + ' V'; } };
};

R.dht11 = (p) => dht(p, '#4fc3f7', 'DHT11');
R.dht22 = (p) => dht(p, '#f5f5f5', 'DHT22');
function dht(p, color, name) {
  const m = moduleBox({ w: 44, h: 52, color: '#eceff1', pins: ['VCC', 'SDA', 'NC', 'GND'], side: 'bottom', spacing: 9, holes: false });
  const grid = svg('rect', { x: 5, y: 5, width: 34, height: 30, rx: 2, fill: color, stroke: '#0003' });
  m.g.append(grid);
  for (let i = 0; i < 5; i++) m.g.append(svg('line', { x1: 8, y1: 10 + i * 5, x2: 36, y2: 10 + i * 5, stroke: '#0002' }));
  const v = text(22, 46, '', { size: 6.5, fill: '#333' });
  m.g.append(text(22, 22, name, { size: 8, fill: '#0007' }), v);
  return { ...m, update(s) { v.textContent = `${s.temperature.toFixed(1)}° ${s.humidity.toFixed(0)}%`; } };
}

R['hc-sr04'] = (p) => {
  const m = moduleBox({ w: 80, h: 40, color: '#1565c0', pins: ['VCC', 'TRIG', 'ECHO', 'GND'], side: 'bottom', spacing: 10 });
  for (const cx of [18, 62]) { m.g.append(svg('circle', { cx, cy: 18, r: 14, fill: '#cfd8dc', stroke: '#78909c', 'stroke-width': 2 })); m.g.append(svg('circle', { cx, cy: 18, r: 9, fill: '#455a64' })); }
  const v = text(40, 36, '', { size: 7, fill: '#fff' });
  m.g.append(v);
  return { ...m, update(s) { v.textContent = s.distance.toFixed(0) + ' cm'; } };
};

let audioCtx = null;
export function soundEnabled(on) { if (on && !audioCtx) audioCtx = new (window.AudioContext || window.webkitAudioContext)(); if (audioCtx) on ? audioCtx.resume() : audioCtx.suspend(); }
function buzzerRender(p, ctx, active) {
  const g = svg('g');
  g.append(svg('line', { x1: -4, y1: 12, x2: -4, y2: 22, stroke: '#aaa', 'stroke-width': 1.5 }));
  g.append(svg('line', { x1: 4, y1: 12, x2: 4, y2: 22, stroke: '#aaa', 'stroke-width': 1.5 }));
  g.append(svg('circle', { cx: 0, cy: 0, r: 13, fill: '#212121', stroke: '#000' }));
  g.append(svg('circle', { cx: 0, cy: 0, r: 3, fill: '#000' }));
  g.append(text(-8, -6, '+', { size: 7, fill: '#aaa' }));
  const waves = svg('path', { d: 'M16,-8 A12,12 0 0 1 16,8 M20,-12 A17,17 0 0 1 20,12', stroke: '#ffd54f', fill: 'none', 'stroke-width': 1.5, opacity: 0 });
  g.append(waves);
  let osc = null, gain = null;
  return {
    g, w: 30, h: 36, ox: -15, oy: -14, pins: [{ name: '+', x: -4, y: 22 }, { name: '-', x: 4, y: 22 }],
    update(s) {
      waves.setAttribute('opacity', s.on ? 1 : 0);
      if (!audioCtx || audioCtx.state !== 'running') { if (osc) { osc.stop(); osc = null; } return; }
      if (s.on && s.freq > 20) {
        if (!osc) { osc = audioCtx.createOscillator(); gain = audioCtx.createGain(); gain.gain.value = 0.04; osc.type = 'square'; osc.connect(gain).connect(audioCtx.destination); osc.start(); }
        osc.frequency.setTargetAtTime(s.freq, audioCtx.currentTime, 0.005);
      } else if (osc) { osc.stop(); osc = null; }
    },
    destroy() { if (osc) osc.stop(); },
  };
}
R.buzzer = (p, c) => buzzerRender(p, c, false);
R['buzzer-active'] = (p, c) => buzzerRender(p, c, true);

R.relay = (p, ctx) => {
  const ch = Math.max(1, Math.min(8, p.channels || 1));
  const names = ['VCC', 'GND'];
  if (ch === 1) names.push('IN', 'COM', 'NO', 'NC'); else for (let i = 1; i <= ch; i++) names.push('IN' + i, 'COM' + i, 'NO' + i, 'NC' + i);
  const w = Math.max(70, names.length * 9 + 10);
  const m = moduleBox({ w, h: 54, color: '#1565c0', title: ch === 1 ? 'RELAY' : `${ch}CH RELAY`, pins: names, side: 'bottom', spacing: 9 });
  const leds = [];
  for (let i = 0; i < ch; i++) {
    const bx = 6 + i * ((w - 12) / ch);
    m.g.append(svg('rect', { x: bx, y: 14, width: Math.min(30, (w - 12) / ch - 4), height: 22, rx: 2, fill: '#1e88e5', stroke: '#0d47a1' }));
    const led = svg('circle', { cx: bx + 4, cy: 42, r: 2.5, fill: '#400' });
    m.g.append(led); leds.push(led);
  }
  return { ...m, update(s) { (s.on || []).forEach((on, i) => leds[i] && leds[i].setAttribute('fill', on ? '#ff1744' : '#400')); } };
};
R['i2c-relay'] = (p, ctx) => {
  const ch = Math.max(1, Math.min(8, p.channels || 4));
  const names = ['VCC', 'GND', 'SDA', 'SCL'];
  for (let i = 1; i <= ch; i++) names.push('COM' + i, 'NO' + i, 'NC' + i);
  const w = Math.max(90, names.length * 8.5 + 10);
  const m = moduleBox({ w, h: 56, color: '#00695c', title: `I2C RELAY ×${ch}`, pins: names, side: 'bottom', spacing: 8.5 });
  const leds = [];
  for (let i = 0; i < ch; i++) {
    const bx = 6 + i * ((w - 12) / ch);
    m.g.append(svg('rect', { x: bx, y: 15, width: (w - 12) / ch - 4, height: 22, rx: 2, fill: '#1e88e5' }));
    const led = svg('circle', { cx: bx + 4, cy: 43, r: 2.5, fill: '#400' });
    m.g.append(led); leds.push(led);
  }
  return { ...m, update(s) { (s.on || []).forEach((on, i) => leds[i] && leds[i].setAttribute('fill', on ? '#ff1744' : '#400')); } };
};

R.servo = (p) => {
  const g = svg('g');
  g.append(svg('rect', { x: -28, y: -12, width: 56, height: 24, rx: 3, fill: '#1e88e5', stroke: '#0d47a1' }));
  g.append(svg('rect', { x: -36, y: -4, width: 8, height: 8, fill: '#1e88e5' }));
  g.append(svg('rect', { x: 28, y: -4, width: 8, height: 8, fill: '#1e88e5' }));
  g.append(svg('circle', { cx: 14, cy: 0, r: 7, fill: '#eceff1' }));
  const arm = svg('path', { d: 'M14,-3 L40,-2 A2,2 0 0 1 40,2 L14,3 Z', fill: '#fafafa', stroke: '#999' });
  g.append(arm);
  const cols = ['#ff9800', '#f44336', '#6d4c41'];
  const pins = [{ name: 'PWM', x: -46, y: -6 }, { name: 'V+', x: -46, y: 0 }, { name: 'GND', x: -46, y: 6 }];
  pins.forEach((pp, i) => g.append(svg('line', { x1: -28, y1: pp.y / 2, x2: pp.x, y2: pp.y, stroke: cols[i], 'stroke-width': 2 })));
  const v = text(0, -16, '', { size: 7, fill: '#9ecbff' });
  g.append(v);
  return { g, w: 92, h: 40, ox: -48, oy: -20, pins, update(s) { arm.setAttribute('transform', `rotate(${-(s.angle - 90)} 14 0)`); v.textContent = s.angle.toFixed(0) + '°'; } };
};

R.ws2812 = (p) => {
  const n = Math.max(1, Math.min(256, p.count || 8));
  const layout = p.layout || 'strip';
  const g = svg('g');
  const dots = [];
  let w, h, pins;
  if (layout === 'ring') {
    const r = Math.max(24, n * 3.2);
    w = h = 2 * r + 24;
    g.append(svg('circle', { cx: 0, cy: 0, r: r + 8, fill: '#111', stroke: '#333' }));
    g.append(svg('circle', { cx: 0, cy: 0, r: r - 8, fill: '#1a1b1e' }));
    for (let i = 0; i < n; i++) {
      const a = (i / n) * Math.PI * 2 - Math.PI / 2;
      const d = svg('circle', { cx: Math.cos(a) * r, cy: Math.sin(a) * r, r: 4.2, fill: '#222', stroke: '#444' });
      g.append(d); dots.push(d);
    }
    pins = [{ name: 'DIN', x: -15, y: r + 18 }, { name: 'VCC', x: -5, y: r + 18 }, { name: 'GND', x: 5, y: r + 18 }, { name: 'DOUT', x: 15, y: r + 18 }];
    pins.forEach((pp) => g.append(svg('line', { x1: pp.x, y1: r + 8, x2: pp.x, y2: pp.y, stroke: '#bbb', 'stroke-width': 1.5 })));
    pins.forEach((pp) => g.append(text(pp.x, r + 26, pp.name, { size: 5.5, fill: '#aaa' })));
    return finish(-r - 12, -r - 12, 2 * r + 24, 2 * r + 40);
  } else if (layout === 'matrix') {
    const cols = p.width || Math.ceil(Math.sqrt(n));
    const rows = Math.ceil(n / cols);
    w = cols * 12 + 8; h = rows * 12 + 8;
    g.append(svg('rect', { x: 0, y: 0, width: w, height: h, fill: '#111', stroke: '#333' }));
    for (let i = 0; i < n; i++) {
      const r = Math.floor(i / cols), c = (r % 2 === 1 && p.serpentine) ? cols - 1 - (i % cols) : i % cols;
      const d = svg('rect', { x: 4 + c * 12 + 1, y: 4 + r * 12 + 1, width: 10, height: 10, rx: 2, fill: '#222' });
      g.append(d); dots.push(d);
    }
    pins = [{ name: 'DIN', x: -6, y: 8 }, { name: 'VCC', x: -6, y: 18 }, { name: 'GND', x: -6, y: 28 }, { name: 'DOUT', x: w + 6, y: 8 }];
    return finish(0, 0, w, h);
  } else {
    w = n * 16 + 12; h = 18;
    g.append(svg('rect', { x: 0, y: 0, width: w, height: h, rx: 2, fill: '#111', stroke: '#333' }));
    for (let i = 0; i < n; i++) {
      g.append(svg('rect', { x: 8 + i * 16, y: 3, width: 12, height: 12, fill: '#eee' }));
      const d = svg('circle', { cx: 14 + i * 16, cy: 9, r: 4.5, fill: '#222' });
      g.append(d); dots.push(d);
    }
    pins = [{ name: 'DIN', x: -8, y: 3 }, { name: 'VCC', x: -8, y: 9 }, { name: 'GND', x: -8, y: 15 }, { name: 'DOUT', x: w + 8, y: 9 }];
    pins.forEach((pp) => g.append(svg('line', { x1: pp.x < 0 ? 0 : w, y1: pp.y, x2: pp.x, y2: pp.y, stroke: '#bbb', 'stroke-width': 1.2 })));
    return finish(0, -4, w, h + 8);
  }
  function finish(ox, oy, ww, hh) {
    return {
      g, w: ww, h: hh, ox, oy, pins,
      update(s) {
        (s.pixels || []).forEach((hx, i) => {
          if (!dots[i]) return;
          const b = hexToBytes(hx);
          const m = Math.max(b[0], b[1], b[2]);
          const k = m ? 255 / m : 0;
          // show hue at full intensity, brightness via opacity-ish blending
          const f = 0.35 + 0.65 * (m / 255);
          const c = m ? `rgb(${Math.round(b[0] * k * f)},${Math.round(b[1] * k * f)},${Math.round(b[2] * k * f)})` : '#222';
          dots[i].setAttribute('fill', c);
          dots[i].setAttribute('filter', m > 10 ? 'url(#glow-small)' : null);
        });
      },
    };
  }
};

R['7segment'] = (p) => {
  const digits = Math.max(1, Math.min(4, p.digits || 1));
  const w = digits * 26 + 8, h = 46;
  const names = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'DP'].concat(digits === 1 ? ['COM'] : ['D1', 'D2', 'D3', 'D4'].slice(0, digits));
  const m = moduleBox({ w: Math.max(w, names.length * 9 + 6), h, color: '#212121', pins: names, side: 'bottom', spacing: 9, holes: false });
  const setters = [];
  for (let i = 0; i < digits; i++) setters.push(sevenSeg(m.g, 6 + i * 26, 6));
  return { ...m, update(s) { const b = hexToBytes(s.segments); setters.forEach((f, i) => f(b[i] || 0)); } };
};
R.tm1637 = (p) => {
  const digits = p.digits || 4;
  const m = moduleBox({ w: digits * 24 + 30, h: 46, color: '#1565c0', pins: ['CLK', 'DIO', 'VCC', 'GND'], side: 'right', spacing: 9 });
  m.g.append(svg('rect', { x: 4, y: 4, width: digits * 24 + 6, height: 38, fill: '#111' }));
  const setters = [];
  for (let i = 0; i < digits; i++) setters.push(sevenSeg(m.g, 8 + i * 24, 6));
  return { ...m, update(s) { const b = hexToBytes(s.segments); setters.forEach((f, i) => f(s.on ? b[i] || 0 : 0)); } };
};
R.max7219 = (p) => {
  const chips = p.chips || 1;
  const m = moduleBox({ w: chips * 74 + 20, h: 80, color: '#1565c0', pins: ['VCC', 'GND', 'DIN', 'CS', 'CLK'], side: 'left', spacing: 10 });
  const dots = [];
  for (let c = 0; c < chips; c++) {
    m.g.append(svg('rect', { x: 14 + c * 74, y: 4, width: 72, height: 72, fill: '#111' }));
    const cd = [];
    for (let r = 0; r < 8; r++) for (let k = 0; k < 8; k++) { const d = svg('circle', { cx: 19 + c * 74 + k * 9, cy: 9 + r * 9, r: 3.4, fill: '#300' }); m.g.append(d); cd.push(d); }
    dots.push(cd);
  }
  return {
    ...m, update(s) {
      (s.chips || []).forEach((ch, ci) => {
        const rows = hexToBytes(ch.rows);
        // chip 0 is the one connected to DIN: draw it on the left
        const cd = dots[ci]; if (!cd) return;
        for (let r = 0; r < 8; r++) for (let k = 0; k < 8; k++) cd[r * 8 + k].setAttribute('fill', ch.on && ((rows[r] >> (7 - k)) & 1) || ch.test ? '#ff1744' : '#300');
      });
    },
  };
};

function oled(p, spi) {
  const w = p.width || 128, h = p.height || 64;
  const sw = w * 0.9, sh = h * 0.9;
  const pins = spi ? ['GND', 'VCC', 'SCK', 'MOSI', 'RES', 'DC', 'CS'] : ['GND', 'VCC', 'SCL', 'SDA'];
  const m = moduleBox({ w: sw + 16, h: sh + 26, color: '#0d2b45', pins, side: 'top', spacing: 10 });
  m.g.append(svg('rect', { x: 6, y: 12, width: sw + 4, height: sh + 4, fill: '#000' }));
  const canvas = pixelCanvas(m.g, 8, 14, sw, sh, w, h);
  const c2d = canvas.getContext('2d');
  const img = c2d.createImageData(w, h);
  const color = p.display_color === 'blue' ? [80, 180, 255] : p.display_color === 'yellow-blue' ? null : [230, 240, 255];
  let ver = -1;
  return {
    ...m, update(s) {
      if (s.version === ver) return; ver = s.version;
      const bm = hexToBytes(s.bitmap);
      for (let i = 0; i < w * h; i++) {
        const on = (bm[i >> 3] >> (7 - (i & 7))) & 1;
        const y = Math.floor(i / w);
        const col = color || (y < 16 ? [255, 220, 60] : [80, 180, 255]);
        const a = (s.contrast ?? 255) / 255 * 0.6 + 0.4;
        img.data[i * 4] = on ? col[0] * a : 4; img.data[i * 4 + 1] = on ? col[1] * a : 6; img.data[i * 4 + 2] = on ? col[2] * a : 10; img.data[i * 4 + 3] = 255;
      }
      c2d.putImageData(img, 0, 0);
    },
  };
}
R.ssd1306 = (p) => oled(p, false);
R.sh1106 = (p) => oled(p, false);
R['ssd1306-spi'] = (p) => oled(p, true);
R['sh1106-spi'] = (p) => oled(p, true);

function lcd(p, parallel) {
  const cols = p.cols || (p.__kind === 'lcd2004-i2c' ? 20 : 16);
  const rows = p.rows || (p.__kind === 'lcd2004-i2c' ? 4 : 2);
  const cw = 9, ch = 14;
  const pins = parallel ? ['VSS', 'VDD', 'V0', 'RS', 'RW', 'E', 'D0', 'D1', 'D2', 'D3', 'D4', 'D5', 'D6', 'D7', 'A', 'K'] : ['GND', 'VCC', 'SDA', 'SCL'];
  const w = cols * cw + 24, h = rows * ch + 28;
  const m = moduleBox({ w, h, color: '#0b6b2f', pins, side: parallel ? 'top' : 'left', spacing: parallel ? 7 : 10 });
  const glass = svg('rect', { x: 8, y: 10, width: cols * cw + 8, height: rows * ch + 8, fill: '#2b4b12', rx: 2 });
  m.g.append(glass);
  const cells = [];
  for (let r = 0; r < rows; r++) {
    const t = svg('text', { x: 12, y: 22 + r * ch, 'font-size': 12.5, 'font-family': 'monospace', fill: '#d0f0ff', 'xml:space': 'preserve', 'letter-spacing': '1.3' });
    m.g.append(t); cells.push(t);
  }
  return {
    ...m, update(s) {
      glass.setAttribute('fill', s.backlight ? '#3d7ad6' : '#1d2c45');
      const on = s.on;
      (s.text || []).forEach((line, r) => { if (cells[r]) { cells[r].textContent = on ? line : ''; cells[r].setAttribute('fill', s.backlight ? '#f0f6ff' : '#7f8fa8'); } });
    },
  };
}
R['lcd1602-i2c'] = (p) => lcd(p, false);
R['lcd2004-i2c'] = (p) => lcd({ ...p, __kind: 'lcd2004-i2c' }, false);
R['lcd-parallel'] = (p) => lcd(p, true);

function tft(p, kind, ctx) {
  const dims = { ili9341: [240, 320], st7789: [240, 240], st7735: [128, 160] }[kind];
  const w = p.width || dims[0], h = p.height || dims[1];
  const sc = kind === 'st7735' ? 0.9 : 0.6;
  const sw = w * sc, sh = h * sc;
  const m = moduleBox({ w: sw + 20, h: sh + 16, color: '#b71c1c', pins: ['VCC', 'GND', 'CS', 'RESET', 'DC', 'MOSI', 'SCK', 'LED', 'MISO'], side: 'left', spacing: 9 });
  m.g.append(svg('rect', { x: 8, y: 6, width: sw + 4, height: sh + 4, fill: '#000' }));
  const canvas = pixelCanvas(m.g, 10, 8, sw, sh, w, h);
  canvas.style.imageRendering = 'auto';
  const c2d = canvas.getContext('2d');
  let ver = -1, busy = false;
  return {
    ...m, update(s) {
      if (s.version === ver || busy) return;
      busy = true; ver = s.version;
      fetch('/api/fb/' + encodeURIComponent(ctx.id)).then((r) => r.ok ? r.arrayBuffer() : null).then((buf) => {
        busy = false;
        if (!buf) return;
        const dv = new DataView(buf); const fw = dv.getUint32(0, true), fh = dv.getUint32(4, true);
        const rgb = new Uint8Array(buf, 8);
        const img = c2d.createImageData(fw, fh);
        for (let i = 0, j = 0; i < fw * fh; i++, j += 3) { img.data[i * 4] = rgb[j]; img.data[i * 4 + 1] = rgb[j + 1]; img.data[i * 4 + 2] = rgb[j + 2]; img.data[i * 4 + 3] = 255; }
        c2d.putImageData(img, 0, 0);
      }).catch(() => (busy = false));
    },
  };
}
R.ili9341 = (p, c) => tft(p, 'ili9341', c);
R.st7735 = (p, c) => tft(p, 'st7735', c);
R.st7789 = (p, c) => tft(p, 'st7789', c);

function expander(p, kind) {
  const bits = kind === 'pcf8575' ? 16 : kind === 'mcp23017' ? 16 : 8;
  let names;
  if (kind === 'mcp23017') names = ['VDD', 'VSS', 'SDA', 'SCL', 'INTA', 'INTB', 'RESET', ...Array.from({ length: 8 }, (_, i) => 'GPA' + i), ...Array.from({ length: 8 }, (_, i) => 'GPB' + i)];
  else if (bits === 16) names = ['VCC', 'GND', 'SDA', 'SCL', 'INT', ...['P00', 'P01', 'P02', 'P03', 'P04', 'P05', 'P06', 'P07', 'P10', 'P11', 'P12', 'P13', 'P14', 'P15', 'P16', 'P17']];
  else names = ['VCC', 'GND', 'SDA', 'SCL', 'INT', 'P0', 'P1', 'P2', 'P3', 'P4', 'P5', 'P6', 'P7'];
  const w = names.length * 9 + 10;
  const m = moduleBox({ w, h: 40, color: '#4a148c', title: kind.toUpperCase(), pins: names, side: 'bottom', spacing: 9 });
  const leds = [];
  for (let i = 0; i < bits; i++) { const d = svg('rect', { x: 8 + i * ((w - 16) / bits), y: 20, width: (w - 16) / bits - 2, height: 8, fill: '#333' }); m.g.append(d); leds.push(d); }
  return { ...m, update(s) { const v = s.inputs ?? 0; leds.forEach((d, i) => d.setAttribute('fill', (v >> i) & 1 ? '#66bb6a' : '#333')); } };
}
R.pcf8574 = (p) => expander(p, 'pcf8574');
R.pcf8574a = (p) => expander(p, 'pcf8574a');
R.pcf8575 = (p) => expander(p, 'pcf8575');
R.mcp23017 = (p) => expander(p, 'mcp23017');

R.ads1115 = smallSensor('ADS1115', '#4a148c', ['VDD', 'GND', 'SCL', 'SDA', 'ADDR', 'ALRT', 'A0', 'A1', 'A2', 'A3']);
R.ads1015 = smallSensor('ADS1015', '#4a148c', ['VDD', 'GND', 'SCL', 'SDA', 'ADDR', 'ALRT', 'A0', 'A1', 'A2', 'A3']);
R.mcp3421 = smallSensor('MCP3421', '#00695c', ['VIN+', 'VSS', 'SCL', 'SDA', 'VDD', 'VIN-']);
R.at24c = smallSensor('AT24C EEPROM', '#37474f', ['VCC', 'GND', 'SCL', 'SDA', 'WP']);
R.mpu6050 = imu('GY-521 MPU6050', '#1565c0', ['VCC', 'GND', 'SCL', 'SDA', 'XDA', 'XCL', 'AD0', 'INT']);
R.mpu6500 = imu('MPU6500', '#1565c0', ['VCC', 'GND', 'SCL', 'SDA', 'AD0', 'INT']);
R.mpu9250 = imu('MPU9250 9DOF', '#6a1b9a', ['VCC', 'GND', 'SCL', 'SDA', 'AD0', 'INT']);
R.adxl345 = imu('ADXL345', '#c62828', ['VCC', 'GND', 'SCL', 'SDA', 'CS', 'SDO', 'INT1', 'INT2']);
R.hmc5883l = imu('GY-271 HMC5883L', '#1565c0', ['VCC', 'GND', 'SCL', 'SDA', 'DRDY']);
R.qmc5883l = imu('GY-271 QMC5883L', '#1565c0', ['VCC', 'GND', 'SCL', 'SDA', 'DRDY']);
R['gy-86'] = imu('GY-86 10DOF', '#6a1b9a', ['VCC', 'GND', 'SCL', 'SDA', 'INT', 'DRDY']);
R['gy-87'] = imu('GY-87 10DOF', '#6a1b9a', ['VCC', 'GND', 'SCL', 'SDA', 'INT', 'DRDY']);
R['gy-91'] = imu('GY-91 10DOF', '#6a1b9a', ['VCC', 'GND', 'SCL', 'SDA', 'INT']);
function imu(title, color, pins) {
  return () => {
    const w = Math.max(64, pins.length * 10 + 8);
    const m = moduleBox({ w, h: 56, color, title, pins, side: 'bottom', spacing: 10 });
    m.g.append(svg('rect', { x: w / 2 - 9, y: 16, width: 18, height: 18, fill: '#111' }));
    const arrow = svg('g', { transform: `translate(${w / 2},25)` });
    arrow.append(svg('line', { x1: 0, y1: 0, x2: 18, y2: 0, stroke: '#ff5252', 'stroke-width': 1.4 }), svg('line', { x1: 0, y1: 0, x2: 0, y2: -14, stroke: '#69f0ae', 'stroke-width': 1.4 }));
    m.g.append(arrow);
    const v = text(w / 2, 47, '', { size: 6.5, fill: '#fff' });
    m.g.append(v);
    return { ...m, update(s) { arrow.setAttribute('transform', `translate(${w / 2},25) rotate(${s.yaw || 0})`); v.textContent = `r${(s.roll || 0).toFixed(0)}° p${(s.pitch || 0).toFixed(0)}° ${s.temperature?.toFixed?.(1) ?? ''}°C`; } };
  };
}
// breakout boards: the extra pins (address select, SPI select, alert) match the simulator's
const I2C4 = ['VCC', 'GND', 'SCL', 'SDA'];
for (const [k, t, extra] of [['ms5611', 'MS5611', ['CSB', 'SDO']], ['bmp280', 'BMP280', ['CSB', 'SDO']], ['bme280', 'BME280', ['CSB', 'SDO']], ['bmp180', 'BMP180', []],
  ['bh1750', 'BH1750', ['ADDR']], ['sht31', 'SHT31', ['ADDR', 'ALERT']], ['aht20', 'AHT20', []]]) {
  R[k] = (p, ctx) => {
    const pins = [...I2C4, ...extra];
    const w = Math.max(60, pins.length * 11 + 8);
    const m = moduleBox({ w, h: 40, color: '#4a148c', title: t, pins, side: 'bottom', spacing: 11 });
    const v = text(w / 2, 28, '', { size: 6.5, fill: '#fff' });
    m.g.append(v);
    return { ...m, update(s, c) { v.textContent = c.summary || ''; } };
  };
}
for (const [k, t, pins] of [['ds3231', 'DS3231 RTC', ['VCC', 'GND', 'SCL', 'SDA', 'SQW', '32K']], ['ds1307', 'DS1307 RTC', ['VCC', 'GND', 'SCL', 'SDA', 'SQW']]]) {
  R[k] = () => {
    const m = moduleBox({ w: 70, h: 44, color: '#1565c0', title: t, pins, side: 'bottom', spacing: 11 });
    m.g.append(svg('circle', { cx: 56, cy: 24, r: 10, fill: '#b0bec5', stroke: '#78909c' }));
    const v = text(28, 30, '', { size: 6.5, fill: '#fff', mono: true });
    m.g.append(v);
    return { ...m, update(s, c) { v.textContent = (c.summary || '').slice(11); } };
  };
}

R.pn532 = (p, ctx) => {
  const m = moduleBox({ w: 90, h: 70, color: '#b71c1c', title: 'PN532 NFC', pins: ['GND', 'VCC', 'SDA', 'SCL', 'IRQ', 'RSTO'], side: 'bottom', spacing: 11 });
  m.g.append(svg('rect', { x: 14, y: 16, width: 62, height: 40, rx: 3, fill: 'none', stroke: '#ffcc80', 'stroke-width': 2 }));
  m.g.append(svg('rect', { x: 19, y: 21, width: 52, height: 30, rx: 3, fill: 'none', stroke: '#ffcc80', 'stroke-width': 1.5 }));
  const card = svg('g', { class: 'interactive' });
  const cr = svg('rect', { x: 96, y: 8, width: 54, height: 34, rx: 4, fill: '#eceff1', stroke: '#90a4ae' });
  const ct = text(123, 28, 'tap card', { size: 7, fill: '#455a64' });
  card.append(cr, ct);
  m.g.append(card);
  let present = false;
  bindClick(card, ctx, () => ctx.input(present ? { action: 'remove' } : { action: 'tap', card: p.card || 'mifare' }));
  return {
    ...m, w: 152, update(s) {
      present = !!s.card;
      cr.setAttribute('x', present ? 26 : 96); cr.setAttribute('y', present ? 20 : 8);
      cr.setAttribute('fill', present ? '#fff59dcc' : '#eceff1');
      ct.setAttribute('x', present ? 53 : 123); ct.setAttribute('y', present ? 40 : 28);
      ct.textContent = present ? s.card.uid.toUpperCase().match(/../g).join(':') : (s.waiting_for_card ? 'tap card ⇢' : 'tap card');
    },
  };
};

R.mfrc522 = (p, ctx) => {
  const m = moduleBox({ w: 100, h: 64, color: '#1565c0', title: 'RFID-RC522', pins: ['SDA', 'SCK', 'MOSI', 'MISO', 'IRQ', 'GND', 'RST', '3.3V'], side: 'bottom', spacing: 11 });
  for (let i = 0; i < 3; i++) m.g.append(svg('rect', { x: 18 + i * 4, y: 16 + i * 4, width: 64 - i * 8, height: 38 - i * 8, rx: 3, fill: 'none', stroke: '#ffd54f', 'stroke-width': 1.2 }));
  const card = svg('g', { class: 'interactive' });
  const cr = svg('rect', { x: 106, y: 8, width: 54, height: 34, rx: 4, fill: '#eceff1', stroke: '#90a4ae' });
  const ct = text(133, 28, 'tap card', { size: 7, fill: '#455a64' });
  card.append(cr, ct);
  m.g.append(card);
  let present = false;
  bindClick(card, ctx, () => ctx.input(present ? { action: 'remove' } : { action: 'tap', card: p.card || 'mifare' }));
  return {
    ...m, w: 162, update(s) {
      present = !!s.card;
      cr.setAttribute('x', present ? 23 : 106); cr.setAttribute('y', present ? 14 : 8);
      cr.setAttribute('fill', present ? '#fff59dcc' : '#eceff1');
      ct.setAttribute('x', present ? 50 : 133); ct.setAttribute('y', present ? 34 : 28);
      ct.textContent = present ? s.card.uid.toUpperCase().match(/../g).join(':') : 'tap card';
    },
  };
};

R.ds18b20 = () => {
  // TO-92 package, flat side up
  const g = svg('g');
  g.append(svg('path', { d: 'M-12,14 L-12,-6 A12,12 0 0 1 12,-6 L12,14 Z', fill: '#263238', stroke: '#000' }));
  g.append(text(0, 2, 'DS18B20', { size: 5, fill: '#cfd8dc' }));
  const v = text(0, -16, '', { size: 7, fill: '#ffcc80' });
  g.append(v);
  const pins = [];
  ['GND', 'DQ', 'VDD'].forEach((n, i) => {
    const x = -8 + i * 8;
    g.append(svg('line', { x1: x, y1: 14, x2: x, y2: 28, stroke: '#aaa', 'stroke-width': 1.5 }));
    g.append(text(x, 36, n, { size: 5, fill: '#b0bec5' }));
    pins.push({ name: n, x, y: 28 });
  });
  return { g, w: 30, h: 60, ox: -15, oy: -24, pins, update(s) { v.textContent = s.temperature.toFixed(2) + '°C'; } };
};

R.hx711 = () => {
  const m = moduleBox({ w: 120, h: 50, color: '#2e7d32', title: 'HX711', pins: ['VCC', 'GND', 'DT', 'SCK', 'E+', 'E-', 'A-', 'A+'], side: 'bottom', spacing: 11 });
  // load cell bar
  m.g.append(svg('rect', { x: 8, y: -34, width: 104, height: 16, rx: 2, fill: '#b0bec5', stroke: '#607d8b' }));
  const plate = svg('rect', { x: 70, y: -46, width: 36, height: 8, rx: 1, fill: '#78909c' });
  const v = text(88, -48, '', { size: 7, fill: '#fff' });
  m.g.append(plate, v);
  return { ...m, oy: -56, h: m.h + 56, update(s) { v.textContent = s.weight.toFixed(1) + ' g'; plate.setAttribute('y', -46 + Math.min(4, Math.abs(s.weight) / 500)); } };
};

R.ina219 = () => {
  const m = moduleBox({ w: 70, h: 44, color: '#6a1b9a', title: 'INA219', pins: ['VCC', 'GND', 'SCL', 'SDA', 'VIN+', 'VIN-'], side: 'bottom', spacing: 10 });
  m.g.append(svg('rect', { x: 24, y: 16, width: 22, height: 8, fill: '#eceff1' }));
  const v = text(35, 36, '', { size: 6.5, fill: '#fff' });
  m.g.append(v);
  return { ...m, update(s) { v.textContent = `${s.current_ma.toFixed(1)} mA ${s.bus_voltage.toFixed(2)} V`; } };
};

function motorGlyph(g, cx, cy, label) {
  g.append(svg('circle', { cx, cy, r: 15, fill: '#ffca28', stroke: '#8d6e63', 'stroke-width': 2 }));
  const rotor = svg('g', { transform: `translate(${cx},${cy})` });
  rotor.append(svg('rect', { x: -2, y: -13, width: 4, height: 26, rx: 1, fill: '#5d4037' }), svg('circle', { r: 3, fill: '#3e2723' }));
  g.append(rotor, text(cx, cy + 24, label, { size: 6, fill: '#cfd8dc' }));
  return rotor;
}

R.l298n = () => {
  const m = moduleBox({ w: 130, h: 70, color: '#c62828', title: 'L298N', pins: ['ENA', 'IN1', 'IN2', 'IN3', 'IN4', 'ENB', '12V', 'GND', '5V'], side: 'bottom', spacing: 11 });
  m.g.append(svg('rect', { x: 50, y: 14, width: 30, height: 24, fill: '#212121' }));
  const ra = motorGlyph(m.g, 22, 32, 'A'), rb = motorGlyph(m.g, 108, 32, 'B');
  const ang = [0, 0];
  let last = performance.now(), speed = [0, 0];
  const spin = () => {
    const now = performance.now(), dt = (now - last) / 1000; last = now;
    [ra, rb].forEach((r, i) => { ang[i] = (ang[i] + speed[i] * 720 * dt) % 360; const t = r.getAttribute('transform').replace(/ rotate\([^)]*\)/, ''); r.setAttribute('transform', `${t} rotate(${ang[i]})`); });
    if (speed[0] || speed[1]) requestAnimationFrame(spin); else spinning = false;
  };
  let spinning = false;
  return { ...m, update(s) { speed = s.speed || [0, 0]; if ((speed[0] || speed[1]) && !spinning) { spinning = true; last = performance.now(); requestAnimationFrame(spin); } } };
};

function stepperBody(m, cx, cy, r, color) {
  m.g.append(svg('rect', { x: cx - r - 4, y: cy - r - 4, width: 2 * r + 8, height: 2 * r + 8, rx: 4, fill: color, stroke: '#000' }));
  m.g.append(svg('circle', { cx, cy, r, fill: '#455a64' }));
  const shaft = svg('line', { x1: cx, y1: cy, x2: cx, y2: cy - r + 3, stroke: '#eceff1', 'stroke-width': 3, 'stroke-linecap': 'round' });
  m.g.append(shaft, svg('circle', { cx, cy, r: 3, fill: '#cfd8dc' }));
  return shaft;
}

R.a4988 = () => {
  const m = moduleBox({ w: 90, h: 40, color: '#1b5e20', title: 'A4988', pins: ['EN', 'STEP', 'DIR', 'SLEEP', 'RESET', 'VDD', 'GND'], side: 'bottom', spacing: 11 });
  const shaft = stepperBody(m, 45, -36, 20, '#37474f');
  const v = text(45, -2, '', { size: 6, fill: '#fff' });
  m.g.append(v);
  return { ...m, oy: -64, h: m.h + 64, update(s) { shaft.setAttribute('transform', `rotate(${s.angle} 45 -36)`); v.textContent = `${s.position} steps`; } };
};

R.uln2003 = () => {
  const m = moduleBox({ w: 80, h: 44, color: '#1565c0', title: 'ULN2003', pins: ['IN1', 'IN2', 'IN3', 'IN4', 'VCC', 'GND'], side: 'bottom', spacing: 11 });
  const leds = [];
  for (let i = 0; i < 4; i++) { const d = svg('circle', { cx: 22 + i * 12, cy: 26, r: 3, fill: '#4a1010' }); m.g.append(d); leds.push(d); }
  const shaft = stepperBody(m, 40, -34, 18, '#0d47a1');
  return { ...m, oy: -60, h: m.h + 60, update(s) { shaft.setAttribute('transform', `rotate(${s.angle} 40 -34)`); leds.forEach((d, i) => d.setAttribute('fill', (s.coils >> i) & 1 ? '#ff5252' : '#4a1010')); } };
};

// LM393 comparator sensor modules: a sensing element above a small board
function lm393(title, color, glyph, pins = ['VCC', 'GND', 'DO', 'AO']) {
  return () => {
    const m = moduleBox({ w: 56, h: 34, color: '#1a237e', title, pins, side: 'bottom', spacing: 11 });
    const g = svg('g', { transform: 'translate(0,-40)' });
    glyph(g);
    m.g.append(g);
    const led = svg('circle', { cx: 48, cy: 22, r: 2.6, fill: '#3b1010' });
    const v = text(22, 25, '', { size: 6.5, fill: '#fff' });
    m.g.append(led, v);
    return { ...m, oy: -44, h: m.h + 44, update(s) { v.textContent = Math.round(s.level * 100) + '%'; led.setAttribute('fill', s.triggered ? '#ff5252' : '#3b1010'); } };
  };
}
R['soil-moisture'] = lm393('SOIL', '#1a237e', (g) => { g.append(svg('path', { d: 'M14,0 h10 v30 l-5,8 l-5,-8 Z M32,0 h10 v30 l-5,8 l-5,-8 Z', fill: '#b0bec5', stroke: '#546e7a' })); });
R.mq2 = lm393('MQ-2', '#1a237e', (g) => { g.append(svg('rect', { x: 12, y: 6, width: 32, height: 30, rx: 4, fill: '#cfd8dc', stroke: '#78909c' })); for (let i = 0; i < 5; i++) g.append(svg('line', { x1: 15, y1: 10 + i * 5, x2: 41, y2: 10 + i * 5, stroke: '#90a4ae' })); });
R['sound-sensor'] = lm393('SOUND', '#1a237e', (g) => { g.append(svg('circle', { cx: 28, cy: 22, r: 14, fill: '#263238', stroke: '#b0bec5', 'stroke-width': 2 })); g.append(svg('circle', { cx: 28, cy: 22, r: 8, fill: '#37474f' })); });
R['flame-sensor'] = lm393('FLAME', '#1a237e', (g) => { g.append(svg('rect', { x: 23, y: 10, width: 10, height: 22, rx: 5, fill: '#212121' })); g.append(svg('path', { d: 'M28,0 C20,10 36,10 28,-6', fill: '#ff7043', opacity: 0.6 })); });
R['rain-sensor'] = lm393('RAIN', '#1a237e', (g) => { g.append(svg('rect', { x: 6, y: 4, width: 44, height: 32, rx: 2, fill: '#c62828' })); for (let i = 0; i < 6; i++) g.append(svg('line', { x1: 10, y1: 8 + i * 5, x2: 46, y2: 8 + i * 5, stroke: '#ffd54f' })); });
R['water-level'] = () => {
  const m = moduleBox({ w: 30, h: 90, color: '#b71c1c', title: '', pins: ['VCC', 'GND', 'S'], side: 'bottom', spacing: 9 });
  for (let i = 0; i < 8; i++) m.g.append(svg('line', { x1: 6, y1: 8 + i * 9, x2: 24, y2: 8 + i * 9, stroke: '#ffd54f', 'stroke-width': 1.5 }));
  const water = svg('rect', { x: 0, y: 90, width: 30, height: 0, fill: '#42a5f5', opacity: 0.45 });
  m.g.append(water);
  return { ...m, update(s) { const h = 80 * s.level; water.setAttribute('y', 88 - h); water.setAttribute('height', h); } };
};
function digitalModule(title, glyph) {
  return () => {
    const m = moduleBox({ w: 46, h: 46, color: '#1565c0', title, pins: ['VCC', 'OUT', 'GND'], side: 'bottom', spacing: 11 });
    glyph(m.g);
    const led = svg('circle', { cx: 39, cy: 9, r: 2.6, fill: '#103b10' });
    m.g.append(led);
    return { ...m, update(s) { led.setAttribute('fill', s.active ? '#69f0ae' : '#103b10'); } };
  };
}
R['touch-sensor'] = digitalModule('TTP223', (g) => g.append(svg('circle', { cx: 23, cy: 19, r: 6, fill: 'none', stroke: '#ffd54f', 'stroke-width': 2 })));
R['hall-sensor'] = digitalModule('HALL', (g) => g.append(svg('rect', { x: 18, y: 14, width: 10, height: 8, fill: '#212121' })));
R['vibration-sensor'] = digitalModule('SW-420', (g) => g.append(svg('rect', { x: 13, y: 14, width: 20, height: 8, rx: 4, fill: '#90a4ae' })));
R['tilt-sensor'] = digitalModule('TILT', (g) => g.append(svg('rect', { x: 16, y: 13, width: 14, height: 9, rx: 3, fill: '#cfd8dc' })));

R['ir-receiver'] = (p, ctx) => {
  const g = svg('g');
  // receiver on a small board
  g.append(svg('rect', { x: 0, y: 0, width: 40, height: 30, rx: 3, fill: '#212121' }));
  g.append(svg('path', { d: 'M10,4 h20 v14 a10,10 0 0 1 -20,0 Z', fill: '#111', stroke: '#555' }));
  const led = svg('circle', { cx: 34, cy: 25, r: 2.5, fill: '#3b1010' });
  g.append(led);
  const pins = [];
  ['OUT', 'GND', 'VCC'].forEach((n, i) => {
    const x = 8 + i * 12;
    g.append(svg('line', { x1: x, y1: 30, x2: x, y2: 38, stroke: '#bbb', 'stroke-width': 2 }));
    g.append(text(x, 46, n, { size: 5.5, fill: '#b0bec5' }));
    pins.push({ name: n, x, y: 38 });
  });
  // remote control
  const keys = ['CH-', 'CH', 'CH+', 'PREV', 'NEXT', 'PLAY', 'VOL-', 'VOL+', 'EQ', '0', '100+', '200+', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
  const rm = svg('g', { transform: 'translate(52,-20)' });
  rm.append(svg('rect', { x: 0, y: 0, width: 58, height: 92, rx: 6, fill: '#eceff1', stroke: '#90a4ae' }));
  keys.forEach((k, i) => {
    const bx = 4 + (i % 3) * 17.5, by = 4 + Math.floor(i / 3) * 12.5;
    const b = svg('g', { class: 'interactive' });
    b.append(svg('rect', { x: bx, y: by, width: 15, height: 10, rx: 2, fill: i < 3 ? '#e53935' : '#455a64' }), text(bx + 7.5, by + 7, k, { size: 4.6, fill: '#fff' }));
    bindClick(b, ctx, () => ctx.input({ key: k }));
    rm.append(b);
  });
  g.append(rm);
  return { g, w: 112, h: 92, ox: 0, oy: -20, pins, update(s) { led.setAttribute('fill', s.receiving ? '#ff5252' : '#3b1010'); } };
};

R['sd-card'] = () => {
  const m = moduleBox({ w: 60, h: 56, color: '#1565c0', title: 'microSD', pins: ['GND', 'VCC', 'MISO', 'MOSI', 'SCK', 'CS'], side: 'bottom', spacing: 9 });
  const slot = svg('rect', { x: 12, y: 16, width: 36, height: 24, rx: 2, fill: '#b0bec5', stroke: '#455a64' });
  const card = svg('path', { d: 'M16,18 h22 l6,6 v14 h-28 Z', fill: '#263238' });
  const act = svg('circle', { cx: 52, cy: 44, r: 2.2, fill: '#103b10' });
  m.g.append(slot, card, act);
  let lastIo = 0;
  return { ...m, update(s) {
    card.setAttribute('opacity', s.inserted ? 1 : 0);
    const io = (s.reads || 0) + (s.writes || 0);
    act.setAttribute('fill', io !== lastIo ? '#69f0ae' : '#103b10');
    lastIo = io;
  } };
};

R['gps-neo6m'] = () => {
  const m = moduleBox({ w: 64, h: 56, color: '#1e3a5f', title: 'GPS NEO-6M', pins: ['VCC', 'RX', 'TX', 'GND', 'PPS'], side: 'bottom', spacing: 10 });
  m.g.append(svg('rect', { x: 14, y: 16, width: 36, height: 28, rx: 2, fill: '#cfae63', stroke: '#8d6e63' }));
  m.g.append(svg('rect', { x: 22, y: 22, width: 20, height: 16, fill: '#455a64' }));
  const led = svg('circle', { cx: 56, cy: 46, r: 2.4, fill: '#3b1010' });
  m.g.append(led);
  let blink = false;
  return { ...m, update(s) { blink = !blink; led.setAttribute('fill', s.fix && blink ? '#40c4ff' : '#0d2a4a'); } };
};

R['hc-05'] = () => {
  const m = moduleBox({ w: 70, h: 40, color: '#0d47a1', title: 'HC-05', pins: ['STATE', 'RXD', 'TXD', 'GND', 'VCC', 'EN'], side: 'bottom', spacing: 10 });
  m.g.append(svg('path', { d: 'M52,10 l8,6 l-8,6 l0,-12 m0,12 l8,6 l-8,6 Z', fill: 'none', stroke: '#90caf9', 'stroke-width': 1.2 }));
  const led = svg('circle', { cx: 12, cy: 20, r: 2.6, fill: '#3b1010' });
  m.g.append(led);
  let blink = false;
  return { ...m, update(s) { blink = !blink; led.setAttribute('fill', s.connected ? '#ff5252' : (blink ? '#ff5252' : '#3b1010')); } };
};

R['touch-pad'] = (p, ctx) => {
  const g = svg('g', { class: 'interactive' });
  const pad = svg('circle', { cx: 0, cy: 0, r: 16, fill: '#c9a227', stroke: '#8d6e1c', 'stroke-width': 2 });
  const finger = svg('ellipse', { cx: 4, cy: -3, rx: 10, ry: 13, fill: '#f1c9a5', opacity: 0 });
  g.append(pad, svg('circle', { cx: 0, cy: 0, r: 9, fill: 'none', stroke: '#8d6e1c', 'stroke-dasharray': '3 2' }), finger);
  g.append(svg('line', { x1: 0, y1: 16, x2: 0, y2: 30, stroke: '#bbb', 'stroke-width': 2 }));
  g.addEventListener('pointerdown', (e) => { e.stopPropagation(); ctx.input({ action: 'touch' }); });
  g.addEventListener('pointerup', (e) => { e.stopPropagation(); ctx.input({ action: 'release' }); });
  return { g, w: 36, h: 50, ox: -18, oy: -18, pins: [{ name: 'PAD', x: 0, y: 30 }], update(s) { finger.setAttribute('opacity', s.touched ? 0.9 : 0); } };
};

R['74hc595'] = () => {
  const names = ['VCC', 'GND', 'DS', 'SHCP', 'STCP', 'OE', 'MR', 'Q7S', 'Q0', 'Q1', 'Q2', 'Q3', 'Q4', 'Q5', 'Q6', 'Q7'];
  const m = moduleBox({ w: 160, h: 36, color: '#212121', title: '74HC595', pins: names, side: 'bottom', spacing: 9.5, holes: false });
  const leds = [];
  for (let i = 0; i < 8; i++) { const d = svg('circle', { cx: 90 + i * 8, cy: 22, r: 2.8, fill: '#333' }); m.g.append(d); leds.push(d); }
  return { ...m, update(s) { leds.forEach((d, i) => d.setAttribute('fill', (s.outputs >> i) & 1 ? '#66bb6a' : '#333')); } };
};

// Fallback: generic module with the part's pins.
function generic(p, ctx, info) {
  const pins = (info && info.pins || []).map((x) => x.name);
  const m = moduleBox({ w: Math.max(60, pins.length * 10 + 10), h: 36, color: '#37474f', title: (info && info.name) || p.__kind, pins, side: 'bottom', spacing: 10 });
  const v = text(m.w / 2, 26, '', { size: 6.5, fill: '#fff' });
  m.g.append(v);
  return { ...m, update(s, c) { v.textContent = (c.summary || '').slice(0, 30); } };
}

export function renderPart(kind, props, ctx, info) {
  const f = R[kind] || ((p, c) => generic(p, c, info));
  const r = f({ ...props, __kind: kind }, ctx);
  r.ox = r.ox ?? 0; r.oy = r.oy ?? 0;
  return r;
}

// Small icon for the parts palette.
export function partIcon(kind) {
  const s = svg('svg', { width: 28, height: 28, viewBox: '-20 -20 40 40' });
  const colors = { Output: '#ff7043', Input: '#42a5f5', Display: '#26c6da', Sensor: '#ab47bc', I2C: '#66bb6a', Passive: '#d4b483', Logic: '#78909c', 'RFID/NFC': '#ef5350', Time: '#ffca28' };
  const r = renderPart(kind, {}, { input() {}, id: '' }, null);
  const scale = Math.min(36 / (r.w || 36), 36 / (r.h || 36), 1.2);
  const g = svg('g', { transform: `scale(${scale}) translate(${-(r.ox + r.w / 2)},${-(r.oy + r.h / 2)})` });
  g.append(r.g);
  s.append(g);
  return s;
}
