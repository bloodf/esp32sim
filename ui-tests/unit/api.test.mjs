import { test } from 'node:test';
import assert from 'node:assert/strict';
import { onFetch, requests, tick } from './setup.mjs';
const { api, el, svg, hexToBytes, escapeHtml, bus, toast, modal, $, $$ } = await import('../../web/js/api.js');

test('el() builds elements with attributes, listeners and children', () => {
  let clicked = 0;
  const e = el('button', { class: 'primary big', title: 'Run', disabled: true, hidden: false, 'data-x': 0, onclick: () => clicked++, style: { color: 'red' } }, 'Go ', el('b', {}, 'now'), null, false, ['!', 2]);
  assert.equal(e.tagName, 'BUTTON');
  assert.equal(e.className, 'primary big');
  assert.equal(e.getAttribute('disabled'), '');
  assert.equal(e.hasAttribute('hidden'), false, 'false attributes are omitted');
  assert.equal(e.getAttribute('data-x'), '0');
  assert.equal(e.style.color, 'red');
  assert.equal(e.textContent, 'Go now!2');
  e.click();
  assert.equal(clicked, 0, 'a disabled button does not fire');
  e.removeAttribute('disabled');
  e.click();
  assert.equal(clicked, 1);
});

test('svg() creates namespaced elements and skips null attributes', () => {
  const g = svg('g', { transform: 'translate(1,2)', fill: null }, svg('circle', { r: 3 }), 'label');
  assert.equal(g.namespaceURI, 'http://www.w3.org/2000/svg');
  assert.equal(g.getAttribute('transform'), 'translate(1,2)');
  assert.equal(g.hasAttribute('fill'), false);
  assert.equal(g.firstChild.getAttribute('r'), '3');
  assert.equal(g.textContent, 'label');
});

test('hexToBytes and escapeHtml', () => {
  assert.deepEqual([...hexToBytes('00ff7f')], [0, 255, 127]);
  assert.deepEqual([...hexToBytes('')], []);
  assert.deepEqual([...hexToBytes(null)], []);
  assert.equal(escapeHtml(`<a href="x">Tom & Jerry's</a>`), '&lt;a href=&quot;x&quot;&gt;Tom &amp; Jerry&#39;s&lt;/a&gt;');
  assert.equal(escapeHtml(42), '42');
});

test('event bus delivers to every listener and isolates failures', () => {
  const got = [];
  bus.on('t1', (a, b) => got.push(a + b));
  bus.on('t1', () => { throw new Error('listener bug'); });
  bus.on('t1', (a) => got.push(a * 10));
  const err = console.error; console.error = () => {};
  bus.emit('t1', 2, 3);
  console.error = err;
  assert.deepEqual(got, [5, 20]);
  bus.emit('nobody-listens', 1);
});

test('api() sends JSON and returns parsed responses', async () => {
  onFetch(async (url, o) => ({ json: { ok: true, echo: JSON.parse(o.body) } }));
  const r = await api('/api/project', { method: 'PUT', json: { a: 1 } });
  assert.deepEqual(r, { ok: true, echo: { a: 1 } });
  assert.equal(requests[0].method, 'PUT');
  assert.equal(requests[0].headers['Content-Type'], 'application/json');
  onFetch(async () => ({ json: { ok: 1 } }));
  await api('/api/control', { json: { cmd: 'pause' } });
  assert.equal(requests[0].method, 'POST', 'json defaults to POST');
  onFetch(async () => ({ text: 'plain body' }));
  assert.equal(await api('/api/x'), 'plain body');
});

test('api() turns error responses into exceptions with the server message', async () => {
  onFetch(async () => ({ status: 400, json: { error: "unknown pin 'r1.9'" } }));
  await assert.rejects(api('/api/project', { json: {} }), /unknown pin 'r1.9'/);
  onFetch(async () => ({ status: 502, text: 'bad gateway body' }));
  await assert.rejects(api('/api/ai/models'), /bad gateway body/);
});

test('toast() shows and hides messages', async () => {
  toast('saved');
  const t = $('#toast');
  assert.equal(t.textContent, 'saved');
  assert.equal(t.className, 'show');
  toast('failed', true);
  assert.equal(t.className, 'show error');
});

test('modal() opens, closes on the button and on the backdrop', () => {
  let closed = 0;
  const close = modal('Settings', el('p', {}, 'body text'), () => closed++);
  const m = $('#modal');
  assert.equal(m.classList.contains('hidden'), false);
  assert.equal($('#modal-title').textContent, 'Settings');
  assert.equal($('#modal-body').textContent, 'body text');
  $('#modal-close').click();
  assert.equal(m.classList.contains('hidden'), true);
  assert.equal(closed, 1);
  modal('Again', el('p', {}, 'x'), () => closed++);
  m.onclick({ target: m });
  assert.equal(closed, 2);
  close();
  assert.equal($$('#modal').length, 1);
});
