// Test environment for the GUI modules: a jsdom window exposed as globals,
// plus small stubs for browser APIs jsdom does not implement (canvas 2D,
// pointer capture, SVG geometry, audio).  Import this before the modules.
import { JSDOM } from 'jsdom';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
export const ROOT = join(here, '..', '..');
export const WEB = join(ROOT, 'web');
export const FIXTURE = JSON.parse(readFileSync(join(here, '..', 'fixtures', 'catalog.json'), 'utf8'));

const html = `<!doctype html><html><body>
  <div id="toast"></div>
  <div id="modal" class="hidden"><div id="modal-title"></div><div id="modal-body"></div><button id="modal-close"></button></div>
  <span id="circuit-hint"></span>
  <button id="ai-clear"></button><button id="ai-settings"></button>
  <svg id="canvas" width="800" height="600"></svg>
</body></html>`;
const dom = new JSDOM(html, { url: 'http://127.0.0.1:8732/', pretendToBeVisual: true });
const w = dom.window;

for (const k of ['window', 'document', 'navigator', 'HTMLElement', 'HTMLCanvasElement', 'SVGElement', 'Element', 'Node', 'Event', 'MouseEvent', 'KeyboardEvent', 'getComputedStyle', 'requestAnimationFrame', 'cancelAnimationFrame', 'localStorage', 'DOMParser', 'CustomEvent']) {
  if (w[k] === undefined) continue;
  const v = typeof w[k] === 'function' && /^[a-z]/.test(k) ? w[k].bind(w) : w[k];
  Object.defineProperty(globalThis, k, { value: v, configurable: true, writable: true });
}

// canvas 2D context: every method is a no-op, image data is real
const ctx2d = () => new Proxy({
  createImageData: (a, b) => ({ width: a, height: b, data: new Uint8ClampedArray(a * b * 4) }),
  getImageData: (x, y, a, b) => ({ width: a, height: b, data: new Uint8ClampedArray(a * b * 4) }),
  measureText: (s) => ({ width: String(s).length * 6 }),
  canvas: null,
}, { get: (t, k) => (k in t ? t[k] : () => {}), set: () => true });
w.HTMLCanvasElement.prototype.getContext = function () { return ctx2d(); };
w.Element.prototype.setPointerCapture = function () {};
w.Element.prototype.releasePointerCapture = function () {};
w.SVGElement.prototype.getBBox = function () { return { x: 0, y: 0, width: 100, height: 50 }; };
globalThis.ResizeObserver = w.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} };
// sized canvases so drawing code runs
Object.defineProperty(w.HTMLElement.prototype, 'clientWidth', { get() { return this.tagName === 'CANVAS' ? 600 : 0; }, configurable: true });
Object.defineProperty(w.HTMLElement.prototype, 'clientHeight', { get() { return this.tagName === 'CANVAS' ? 200 : 0; }, configurable: true });
// confirm() answers from a queue the tests fill (default: yes)
export const confirmAnswers = [];
w.confirm = globalThis.confirm = () => (confirmAnswers.length ? confirmAnswers.shift() : true);

// fetch: tests install handlers; unhandled requests fail loudly
export const requests = [];
let handler = null;
export function onFetch(f) { handler = f; requests.length = 0; }
globalThis.fetch = async (url, opts = {}) => {
  requests.push({ url: String(url), method: opts.method || 'GET', body: opts.body ? JSON.parse(opts.body) : undefined, headers: opts.headers || {} });
  const r = handler ? await handler(String(url), opts) : { status: 404, json: { error: 'no fetch handler' } };
  const isJson = r.json !== undefined;
  const body = isJson ? JSON.stringify(r.json) : (r.text ?? '');
  return new Response(body, { status: r.status || 200, headers: { 'content-type': isJson ? 'application/json' : 'text/plain' } });
};

export const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));
export { dom };
