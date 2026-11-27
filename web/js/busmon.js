// Bus monitor: I2C / SPI / UART / RMT traffic at transaction level.
// I2C rows show the address, ACK/NACK and the bytes ("!" marks a NACKed byte).
import { api, el } from './api.js';

export class BusMonitor {
  constructor(root) {
    this.root = root;
    this.next = 0;
    this.rows = [];
    this.visible = false;
    this.paused = false;
    this.filter = { I2C: true, SPI: true, UART: true, RMT: true };
    this.render();
    setInterval(() => this.visible && !this.paused && this.poll(), 300);
  }

  render() {
    this.root.innerHTML = '';
    const chk = (name) => {
      const c = el('input', { type: 'checkbox', checked: this.filter[name], onchange: () => { this.filter[name] = c.checked; this.draw(); } });
      return el('label', { class: 'la-pad' }, c, name);
    };
    this.search = el('input', { placeholder: 'filter: address, device, text…', class: 'bm-search', oninput: () => this.draw() });
    this.pauseBtn = el('button', { onclick: () => { this.paused = !this.paused; this.pauseBtn.textContent = this.paused ? '▶ Resume' : '⏸ Pause'; } }, '⏸ Pause');
    this.count = el('span', { class: 'muted' });
    this.root.append(el('div', { class: 'plot-bar' }, chk('I2C'), chk('SPI'), chk('UART'), chk('RMT'), this.search, this.pauseBtn,
      el('button', { onclick: () => { this.rows = []; this.draw(); } }, '🧹 Clear'), el('div', { class: 'grow' }), this.count));
    this.table = el('div', { class: 'bm-table' });
    this.root.append(this.table);
  }

  show(v) { this.visible = v; if (v) this.poll(); }

  async poll() {
    let r;
    try { r = await api('/api/buslog?since=' + this.next); } catch (e) { return; }
    if (!r || r.next === undefined) return;
    if (r.next < this.next) { this.next = 0; this.rows = []; }
    this.next = r.next;
    if (r.events.length) {
      this.rows.push(...r.events);
      if (this.rows.length > 5000) this.rows.splice(0, this.rows.length - 5000);
      this.draw();
    }
  }

  draw() {
    const q = this.search.value.trim().toLowerCase();
    const rows = this.rows.filter((e) => {
      const bus = e.bus.startsWith('UART') ? 'UART' : e.bus;
      if (!this.filter[bus]) return false;
      return !q || JSON.stringify(e).toLowerCase().includes(q);
    });
    const atBottom = this.table.scrollTop + this.table.clientHeight >= this.table.scrollHeight - 8;
    this.table.innerHTML = '';
    const shown = rows.slice(-400);
    for (const e of shown) {
      let what = '';
      if (e.bus === 'I2C') what = `${e.addr} ${e.dir.toUpperCase()} ${e.ack ? 'ACK' : 'NACK'}  ${spaced(e.data)}`;
      else if (e.bus === 'SPI') what = `MOSI ${spaced(e.mosi)}${e.len > 48 ? '…' : ''}   MISO ${spaced(e.miso)}${e.len > 48 ? '…' : ''}`;
      else what = e.text || '';
      const dev = (e.devices || []).join(', ');
      const cls = e.bus === 'I2C' && !e.ack ? 'bm-row bm-nack' : 'bm-row';
      this.table.append(el('div', { class: cls },
        el('span', { class: 'bm-t' }, fmtT(e.t)),
        el('span', { class: 'bm-bus bm-' + e.bus.replace(/\d/, '') }, e.bus + (e.dir && e.bus.startsWith('UART') ? ' ' + e.dir : '')),
        el('span', { class: 'bm-dev' }, dev),
        el('span', { class: 'bm-what' }, what)));
    }
    this.count.textContent = `${rows.length} transactions${rows.length > shown.length ? ` (last ${shown.length} shown)` : ''}`;
    if (atBottom) this.table.scrollTop = this.table.scrollHeight;
  }
}

function spaced(hex) { return (hex || '').replace(/(..!?)/g, '$1 ').trim(); }
function fmtT(ns) { return (ns / 1e9).toFixed(6) + ' s'; }
