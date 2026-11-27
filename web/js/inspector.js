// Right-hand inspector: properties, live state and controls of the selected part.
import { el, bus } from './api.js';

const IMU = ['mpu6050', 'mpu6500', 'mpu9250', 'adxl345', 'gy-86', 'gy-87', 'gy-91'];
const MAG = ['hmc5883l', 'qmc5883l'];
const BARO = ['ms5611', 'bmp280', 'bme280', 'bmp180'];

function slider(label, min, max, step, value, onInput, fmt = (v) => v) {
  const val = el('span', { class: 'val' }, fmt(value));
  const inp = el('input', { type: 'range', min, max, step, value });
  inp.addEventListener('input', () => { val.textContent = fmt(parseFloat(inp.value)); onInput(parseFloat(inp.value)); });
  return { node: el('div', { class: 'field' }, el('label', {}, label), el('div', { class: 'row' }, inp, val)), set(v) { if (document.activeElement !== inp) { inp.value = v; val.textContent = fmt(v); } } };
}

export class Inspector {
  constructor(root, state) {
    this.root = root;
    this.state = state;
    this.sel = null;
    this.live = null;
    this.circuit = null;
    this.renderPins();
  }

  select(sel, circuit) {
    this.sel = sel;
    this.circuit = circuit;
    this.render();
  }

  input(action) { bus.emit('input', this.sel.id, action); }

  compState() {
    if (!this.sel || !this.state.board) return null;
    return (this.state.board.components || []).find((c) => c.id === this.sel.id);
  }

  render() {
    const r = this.root;
    r.innerHTML = '';
    this.live = null;
    if (!this.sel) return this.renderPins();
    if (this.sel.type === 'wire') return this.renderWire();
    const cfg = this.circuit.project.components.find((c) => c.id === this.sel.id);
    if (!cfg) return;
    const info = this.circuit.info(cfg.type) || { name: cfg.type, props: {}, pins: [] };
    r.append(el('div', { class: 'insp-title' }, cfg.id), el('div', { class: 'insp-kind' }, `${info.name} · ${cfg.type}`));
    const summary = el('div', { class: 'summary' }, '…');
    r.append(summary);
    const controls = el('div');
    r.append(controls);
    const live = { summary, sliders: {}, stateEl: null };
    this.live = live;
    this.buildControls(cfg, controls, live);
    // properties
    r.append(el('div', { class: 'section-title' }, 'Properties'));
    const keys = new Set([...Object.keys(info.props || {}), ...Object.keys(cfg)]);
    ['id', 'type', 'x', 'y', 'rotate'].forEach((k) => keys.delete(k));
    for (const k of keys) {
      const v = cfg[k] ?? info.props[k];
      const inp = k === 'color' && cfg.type === 'led'
        ? el('select', {}, ...['red', 'green', 'blue', 'yellow', 'orange', 'white', 'purple'].map((c) => el('option', { value: c, selected: c === v }, c)))
        : el('input', { value: typeof v === 'object' ? JSON.stringify(v) : v });
      inp.addEventListener('change', () => {
        let nv = inp.value;
        if (nv === 'true' || nv === 'false') nv = nv === 'true';
        else if (/^-?\d+(\.\d+)?$/.test(nv)) nv = parseFloat(nv);
        cfg[k] = nv;
        this.circuit.render();
        this.circuit.changed(true);
      });
      r.append(el('div', { class: 'field' }, el('label', {}, k), inp));
    }
    r.append(el('div', { class: 'section-title' }, 'Pins'));
    r.append(el('div', { class: 'muted', style: { fontSize: '12px', lineHeight: '1.6' } }, (info.pins || []).map((p) => `${p.name} (${p.kind})`).join(' · ')));
    const det = el('details', { class: 'state' }, el('summary', { class: 'muted' }, 'Raw state'), (live.stateEl = el('pre')));
    r.append(det);
    r.append(el('div', { class: 'btn-row', style: { marginTop: '10px' } },
      el('button', { onclick: () => this.circuit.rotateSelected() }, '⟳ Rotate (R)'),
      el('button', { class: 'danger', onclick: () => this.circuit.deleteSelected() }, '🗑 Delete')));
    this.update(this.state.board);
  }

  buildControls(cfg, c, live) {
    const k = cfg.type;
    const btns = (...defs) => c.append(el('div', { class: 'btn-row' }, ...defs.map(([t, a]) => el('button', { onclick: () => this.input(a) }, t))));
    const sl = (name, label, min, max, step, key, fmt) => {
      const s = slider(label, min, max, step, 0, (v) => this.input({ [key]: v }), fmt);
      live.sliders[name] = s;
      c.append(s.node);
    };
    if (k === 'pushbutton') {
      const b = el('button', {}, 'Hold to press');
      b.addEventListener('pointerdown', () => this.input({ action: 'press' }));
      b.addEventListener('pointerup', () => this.input({ action: 'release' }));
      c.append(el('div', { class: 'btn-row' }, b, el('button', { onclick: () => this.input({ action: 'click' }) }, 'Click'), el('button', { onclick: () => this.input({ action: 'toggle' }) }, 'Toggle')));
    } else if (['switch', 'slide-switch'].includes(k)) btns(['Toggle', { action: 'toggle' }]);
    else if (k === 'potentiometer') sl('position', 'Position', 0, 1, 0.005, 'value', (v) => (v * 100).toFixed(0) + '%');
    else if (k === 'signal') { sl('volts', 'Voltage', 0, 5, 0.01, 'volts', (v) => v.toFixed(2) + ' V'); btns(['High', { action: 'high' }], ['Low', { action: 'low' }]); }
    else if (k === 'ldr') sl('lux', 'Light (lux)', 0, 5000, 1, 'lux', (v) => v.toFixed(0));
    else if (k === 'ntc') sl('temperature', 'Temperature °C', -20, 120, 0.5, 'temperature');
    else if (k === 'dht11' || k === 'dht22') { sl('temperature', 'Temperature °C', -20, 60, 0.1, 'temperature'); sl('humidity', 'Humidity %', 0, 100, 0.5, 'humidity'); }
    else if (k === 'hc-sr04') sl('distance', 'Distance cm', 2, 450, 1, 'distance');
    else if (['pir', 'ir-obstacle', 'touch-sensor', 'hall-sensor', 'vibration-sensor', 'tilt-sensor'].includes(k)) btns(['Trigger', { action: 'trigger' }], ['Toggle', { action: 'toggle' }]);
    else if (['soil-moisture', 'mq2', 'sound-sensor', 'flame-sensor', 'rain-sensor', 'water-level'].includes(k)) {
      const label = { 'soil-moisture': 'Moisture', mq2: 'Gas concentration', 'sound-sensor': 'Loudness', 'flame-sensor': 'Flame (IR)', 'rain-sensor': 'Wetness', 'water-level': 'Water level' }[k];
      sl('level', label, 0, 1, 0.01, 'level', (v) => (v * 100).toFixed(0) + '%');
      if (k !== 'water-level') sl('threshold', 'Trimmer threshold', 0, 1, 0.01, 'threshold', (v) => (v * 100).toFixed(0) + '%');
    }
    else if (k === 'rotary-encoder') btns(['⟲ -1', { steps: -1 }], ['+1 ⟳', { steps: 1 }], ['+10', { steps: 10 }], ['Press', { pressed: true }], ['Release', { pressed: false }]);
    else if (k === 'joystick') { sl('x', 'X', 0, 1, 0.01, 'x'); sl('y', 'Y', 0, 1, 0.01, 'y'); btns(['Press', { pressed: true }], ['Release', { pressed: false }]); }
    else if (k.startsWith('keypad')) {
      const inp = el('input', { placeholder: 'keys to type, e.g. 1234#' });
      c.append(el('div', { class: 'field' }, el('label', {}, 'Type a sequence'), el('div', { class: 'row' }, inp, el('button', { onclick: () => this.input({ type: inp.value }) }, 'Type'))));
    } else if (IMU.includes(k) || MAG.includes(k)) {
      sl('roll', 'Roll °', -180, 180, 1, 'roll'); sl('pitch', 'Pitch °', -90, 90, 1, 'pitch'); sl('yaw', 'Yaw / heading °', -180, 180, 1, 'yaw');
      if (IMU.includes(k)) {
        const g = { x: 0, y: 0, z: 0 };
        for (const ax of ['x', 'y', 'z']) { const s = slider(`Gyro ${ax} °/s`, -500, 500, 1, 0, (v) => { g[ax] = v; this.input({ gyro: { ...g } }); }); live.sliders['gyro' + ax] = s; c.append(s.node); }
        sl('temperature', 'Temperature °C', -20, 80, 0.5, 'temperature');
      }
      if (['gy-86', 'gy-87', 'gy-91'].includes(k)) sl('altitude', 'Altitude m', -100, 5000, 1, 'altitude');
    } else if (BARO.includes(k)) {
      sl('temperature', 'Temperature °C', -20, 60, 0.1, 'temperature'); sl('altitude', 'Altitude m', -100, 5000, 1, 'altitude');
      if (k === 'bme280') sl('humidity', 'Humidity %', 0, 100, 0.5, 'humidity');
    } else if (k === 'sht31' || k === 'aht20') { sl('temperature', 'Temperature °C', -20, 60, 0.1, 'temperature'); sl('humidity', 'Humidity %', 0, 100, 0.5, 'humidity'); }
    else if (k === 'bh1750') sl('lux', 'Light (lux)', 0, 20000, 1, 'lux');
    else if (k === 'ds3231' || k === 'ds1307') btns(['Sync to this computer\'s clock', { time: Math.floor(Date.now() / 1000) - new Date().getTimezoneOffset() * 60 }]);
    else if (k === 'pn532') {
      const type = el('select', {}, el('option', { value: 'mifare' }, 'MIFARE Classic 1K (4-byte UID)'), el('option', { value: 'ntag213' }, 'NTAG213 (7-byte UID)'));
      const uid = el('input', { placeholder: 'UID e.g. DE:AD:BE:EF (blank = default)' });
      const txt = el('input', { placeholder: 'NDEF text (NTAG only)' });
      c.append(el('div', { class: 'field' }, el('label', {}, 'Card'), type), el('div', { class: 'field' }, el('label', {}, 'UID'), uid), el('div', { class: 'field' }, el('label', {}, 'Text'), txt));
      c.append(el('div', { class: 'btn-row' },
        el('button', { class: 'primary', onclick: () => { const a = { action: 'tap', card: type.value }; if (uid.value) a.uid = uid.value; if (txt.value) a.text = txt.value; this.input(a); } }, '📶 Tap card'),
        el('button', { onclick: () => this.input({ action: 'remove' }) }, 'Remove')));
    } else if (k === 'mfrc522') {
      const type = el('select', {}, el('option', { value: 'mifare' }, 'MIFARE Classic 1K (4-byte UID)'), el('option', { value: 'ntag213' }, 'NTAG213 / Ultralight (7-byte UID)'));
      const uid = el('input', { placeholder: 'UID e.g. DE:AD:BE:EF (blank = default)' });
      c.append(el('div', { class: 'field' }, el('label', {}, 'Card'), type), el('div', { class: 'field' }, el('label', {}, 'UID'), uid));
      c.append(el('div', { class: 'btn-row' },
        el('button', { class: 'primary', onclick: () => { const a = { action: 'tap', card: type.value }; if (uid.value) a.uid = uid.value; this.input(a); } }, '📶 Tap card'),
        el('button', { onclick: () => this.input({ action: 'remove' }) }, 'Remove')));
    } else if (k === 'ir-receiver') {
      const keys = ['CH-', 'CH', 'CH+', 'PREV', 'NEXT', 'PLAY', 'VOL-', 'VOL+', 'EQ', '0', '100+', '200+', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
      const grid = el('div', { class: 'btn-row', style: { flexWrap: 'wrap' } }, ...keys.map((key) => el('button', { onclick: () => this.input({ key }) }, key)));
      c.append(el('div', { class: 'section-title' }, 'Remote (NEC, address 0x00)'), grid);
      const hold = el('button', { onclick: () => this.input({ key: 'VOL+', repeats: 5 }) }, 'Hold VOL+ (repeat codes)');
      c.append(el('div', { class: 'btn-row' }, hold));
    } else if (k === 'sd-card') {
      btns(['⏏ Eject', { action: 'eject' }], ['Insert', { action: 'insert' }], ['⟳ Refresh files', { action: 'refresh' }]);
      c.append(el('div', { class: 'section-title' }, 'Files on the card'));
      live.files = el('div', { class: 'sd-files' });
      c.append(live.files);
    } else if (k === 'gps-neo6m') {
      const lat = el('input', { value: cfg.lat ?? 40.4168, style: { width: '90px' } });
      const lon = el('input', { value: cfg.lon ?? -3.7038, style: { width: '90px' } });
      c.append(el('div', { class: 'field' }, el('label', {}, 'Position (lat, lon)'), el('div', { class: 'row' }, lat, lon,
        el('button', { onclick: () => this.input({ lat: parseFloat(lat.value), lon: parseFloat(lon.value) }) }, 'Set'))));
      sl('speed', 'Speed km/h', 0, 200, 1, 'speed'); sl('course', 'Course °', 0, 359, 1, 'course'); sl('alt', 'Altitude m', -100, 5000, 1, 'alt');
      btns(['Lose fix', { fix: false }], ['Get fix', { fix: true }]);
    } else if (k === 'hc-05') {
      btns(['📱 Connect phone', { action: 'connect' }], ['Disconnect', { action: 'disconnect' }], ['Clear', { action: 'clear' }]);
      const msg = el('input', { placeholder: 'text the phone sends (Enter)' });
      msg.addEventListener('keydown', (e) => { if (e.key === 'Enter') { this.input({ send: msg.value }); msg.value = ''; } });
      c.append(el('div', { class: 'field' }, el('label', {}, 'Phone → ESP32'), msg));
      c.append(el('div', { class: 'section-title' }, 'ESP32 → phone'));
      live.phone = el('pre', { class: 'phone-rx' });
      c.append(live.phone);
    } else if (k === 'touch-pad') {
      const b = el('button', {}, 'Hold to touch');
      b.addEventListener('pointerdown', () => this.input({ action: 'touch' }));
      b.addEventListener('pointerup', () => this.input({ action: 'release' }));
      c.append(el('div', { class: 'btn-row' }, b, el('button', { onclick: () => this.input({ action: 'toggle' }) }, 'Toggle')));
    } else if (k === 'ds18b20') sl('temperature', 'Temperature °C', -55, 125, 0.0625, 'temperature', (v) => v.toFixed(2));
    else if (k === 'hx711') { sl('weight', 'Weight g', -500, 5000, 1, 'weight'); btns(['Empty the scale', { weight: 0 }]); }
    else if (k === 'ina219') { sl('current_ma', 'Current mA (when VIN± unwired)', -3200, 3200, 1, 'current_ma'); sl('bus_voltage', 'Bus voltage V', 0, 26, 0.01, 'bus_voltage'); }
    else if (k === 'a4988') btns(['Zero position', { action: 'zero' }]);
    else if (k === 'led') btns(['Repair (if burned)', { action: 'repair' }]);
  }

  update(board) {
    if (!this.sel) { if (board) this.renderPins(board); return; }
    if (!this.live) return;
    const c = this.compState();
    if (!c) return;
    this.live.summary.textContent = c.summary || '—';
    if (this.live.stateEl && this.live.stateEl.parentElement.open) this.live.stateEl.textContent = JSON.stringify(c.state, null, 1);
    const s = c.state || {};
    if (this.live.phone && s.phone_rx !== undefined && this.live.phone.textContent !== s.phone_rx) {
      this.live.phone.textContent = s.phone_rx || '(nothing yet)';
      this.live.phone.scrollTop = this.live.phone.scrollHeight;
    }
    if (this.live.files) {
      const key = JSON.stringify(s.files);
      if (key !== this.live.filesKey) {
        this.live.filesKey = key;
        this.live.files.innerHTML = '';
        if (!Array.isArray(s.files)) this.live.files.append(el('div', { class: 'muted' }, (s.files && s.files.error) || 'no files'));
        else if (!s.files.length) this.live.files.append(el('div', { class: 'muted' }, 'empty'));
        else for (const f of s.files) {
          const d = el('details', { class: 'sd-file' }, el('summary', {}, `${f.path}  `, el('span', { class: 'muted' }, f.path.endsWith('/') ? '' : `${f.size} B`)));
          if (f.preview) d.append(el('pre', {}, f.preview + (f.size > f.preview.length ? '…' : '')));
          this.live.files.append(d);
        }
      }
    }
    for (const [name, sl] of Object.entries(this.live.sliders)) {
      if (name.startsWith('gyro')) { const i = 'xyz'.indexOf(name[4]); if (s.gyro) sl.set(s.gyro[i]); continue; }
      if (s[name] !== undefined) sl.set(s[name]);
    }
  }

  renderWire() {
    const r = this.root;
    const conn = this.circuit.project.connections[this.sel.index];
    if (!conn) return;
    const eps = Array.isArray(conn) ? conn : typeof conn === 'string' ? conn.split(/\s*(?:->| - )\s*/) : [conn.from, conn.to];
    r.append(el('div', { class: 'insp-title' }, 'Wire'), el('div', { class: 'insp-kind' }, eps.join('  ↔  ')));
    const colors = ['', '#3a3a3a', '#e53935', '#ff7043', '#42a5f5', '#fdd835', '#66bb6a', '#ab47bc', '#26c6da', '#ffa726', '#ec407a', '#f5f5f5'];
    const row = el('div', { class: 'btn-row' });
    for (const col of colors) {
      row.append(el('button', {
        title: col || 'automatic', style: { background: col || 'transparent', width: '24px', height: '22px' }, onclick: () => {
          let c = conn;
          if (!c.from) { c = { from: eps[0], to: eps[eps.length - 1] }; this.circuit.project.connections[this.sel.index] = c; }
          if (col) c.color = col; else delete c.color;
          this.circuit.renderWires(); this.circuit.changed(false);
        },
      }, col ? '' : 'A'));
    }
    r.append(el('div', { class: 'section-title' }, 'Colour'), row);
    r.append(el('div', { class: 'btn-row' }, el('button', { class: 'danger', onclick: () => this.circuit.deleteSelected() }, '🗑 Delete wire')));
  }

  renderPins(board) {
    const r = this.root;
    board = board || this.state.board;
    r.innerHTML = '';
    r.append(el('div', { class: 'insp-title' }, 'ESP32'), el('div', { class: 'insp-kind' }, 'Select a part to inspect it. Live pin state:'));
    if (this.circuit && this.circuit.project && (this.circuit.boards || []).length) {
      const cur = this.circuit.boardDef();
      const sel = el('select', { onchange: () => { this.circuit.project.board = sel.value; this.circuit.render(); this.circuit.changed(true); } },
        ...this.circuit.boards.map((b) => el('option', { value: b.id, selected: cur && b.id === cur.id }, b.name)));
      r.append(el('div', { class: 'field' }, el('label', {}, 'Board'), sel));
    }
    const st = this.state.status || {};
    if (st.cpu_mhz) r.append(el('div', { class: 'summary' }, `CPU ${st.cpu_mhz} MHz · ${(st.mips || 0).toFixed(0)} MIPS\nresets: ${st.resets || 0}\n${st.firmware ? 'firmware: ' + st.firmware.split('/').slice(-3).join('/') : ''}`));
    const pads = (board && board.pads) || {};
    const rows = Object.entries(pads).sort((a, b) => a[0] - b[0]);
    if (!rows.length) { r.append(el('p', { class: 'muted' }, 'No pins connected yet. Add parts from the 🧩 Parts panel and click pins to wire them.')); return; }
    const t = el('div', { class: 'kv' });
    for (const [g, p] of rows) {
      const v = p.v === null || p.v === undefined ? 'float' : p.v.toFixed(2) + ' V';
      t.append(el('span', {}, 'GPIO' + g), el('span', {}, `${p.mode.padEnd(4)} ${v}${p.pwm ? `  pwm ${Math.round(p.pwm.freq)}Hz ${(p.pwm.duty * 100).toFixed(0)}%` : ''}`));
    }
    r.append(t);
  }
}
