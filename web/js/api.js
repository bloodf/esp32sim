// Small helpers shared by all modules.
export async function api(path, opts = {}) {
  const o = { ...opts };
  if (o.json !== undefined) {
    o.method = o.method || 'POST';
    o.headers = { 'Content-Type': 'application/json', ...(o.headers || {}) };
    o.body = JSON.stringify(o.json);
    delete o.json;
  }
  const r = await fetch(path, o);
  const ct = r.headers.get('content-type') || '';
  const data = ct.includes('application/json') ? await r.json() : await r.text();
  if (!r.ok) throw new Error((data && data.error) || (typeof data === 'string' ? data : r.statusText));
  return data;
}

export function $(sel, root = document) { return root.querySelector(sel); }
export function $$(sel, root = document) { return [...root.querySelectorAll(sel)]; }

export function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (k === 'class') e.className = v;
    else if (k === 'style' && typeof v === 'object') Object.assign(e.style, v);
    else if (k.startsWith('on') && typeof v === 'function') e.addEventListener(k.slice(2), v);
    else if (v !== undefined && v !== null && v !== false) e.setAttribute(k, v === true ? '' : v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    e.append(c.nodeType ? c : document.createTextNode(String(c)));
  }
  return e;
}

export const SVGNS = 'http://www.w3.org/2000/svg';
export function svg(tag, attrs = {}, ...children) {
  const e = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (k.startsWith('on') && typeof v === 'function') e.addEventListener(k.slice(2), v);
    else if (v !== undefined && v !== null) e.setAttribute(k, v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined) continue;
    e.append(c.nodeType ? c : document.createTextNode(String(c)));
  }
  return e;
}

let toastTimer = null;
export function toast(msg, error = false) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.className = 'show' + (error ? ' error' : '');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.className = ''), error ? 5000 : 2500);
}

export function modal(title, body, onClose) {
  const m = document.getElementById('modal');
  document.getElementById('modal-title').textContent = title;
  const b = document.getElementById('modal-body');
  b.innerHTML = '';
  b.append(body);
  m.classList.remove('hidden');
  const close = () => { m.classList.add('hidden'); onClose && onClose(); };
  document.getElementById('modal-close').onclick = close;
  m.onclick = (e) => { if (e.target === m) close(); };
  return close;
}

export function hexToBytes(h) {
  const out = new Uint8Array((h || '').length >> 1);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(h.substr(i * 2, 2), 16);
  return out;
}

export function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// A tiny event bus
const listeners = {};
export const bus = {
  on(ev, f) { (listeners[ev] = listeners[ev] || []).push(f); },
  emit(ev, ...a) { (listeners[ev] || []).forEach((f) => { try { f(...a); } catch (e) { console.error(e); } }); },
};
