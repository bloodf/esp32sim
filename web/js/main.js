import { api, $, $$, el, toast, modal, bus } from './api.js';
import { Circuit } from './circuit.js';
import { partIcon, soundEnabled } from './parts.js';
import { Serial } from './serial.js';
import { Plotter } from './plotter.js';
import { LogicAnalyzer } from './logic.js';
import { BusMonitor } from './busmon.js';
import { Inspector } from './inspector.js';
import { Editor } from './editor.js';
import { GitPanel } from './git.js';
import { AiPanel } from './ai.js';
import { DebugPanel } from './debug.js';
import { openSettings, newProjectDialog, openProjectDialog } from './settings.js';

const state = { status: {}, board: null, catalog: [], boards: [], info: null, project: null, build: { lines: [], diagnostics: [] } };
window.esp32sim = state;

const circuit = new Circuit($('#circuit'));
const serial = new Serial($('#terminal'));
state.serialView = serial;
const plotter = new Plotter($('#bview-plot'));
const logic = new LogicAnalyzer($('#bview-logic'));
const busmon = new BusMonitor($('#bview-bus'));
const inspector = new Inspector($('#inspector-body'), state);
inspector.circuit = circuit;
const editor = new Editor(state);
const git = new GitPanel($('#git-body'), editor);
const ai = new AiPanel($('#ai-body'), editor);
document.addEventListener('ai-ask', (e) => { showPanel('ai'); ai.ask(e.detail); });
const debug = new DebugPanel($('#debug-body'), editor, state);

// ------------------------------------------------------------------ websocket
let ws = null;
function connect() {
  ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`);
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.type === 'reset') { serial.clear(); $('#emu-log').textContent = ''; return; }
    if (m.status) { state.status = m.status; updateStatus(); debug.onStatus(m.status); }
    if (m.board) { state.board = m.board; circuit.updateStates(m.board); inspector.update(m.board); }
    if (m.serial) { serial.write(m.serial); plotter.feed(atob(m.serial)); }
    if (m.messages) for (const x of m.messages) logLine(x);
    if (m.build) onBuild(m.build);
  };
  ws.onclose = () => setTimeout(connect, 1000);
}
function send(o) { if (ws && ws.readyState === 1) ws.send(JSON.stringify(o)); }
bus.on('input', (id, action) => send({ type: 'input', id, action }));
bus.on('control', (cmd, value) => send({ type: 'control', cmd, value }));
bus.on('serial', (data) => send({ type: 'serial', data }));

function logLine(x) {
  const pre = $('#emu-log');
  pre.textContent += x + '\n';
  if (pre.textContent.length > 200000) pre.textContent = pre.textContent.slice(-150000);
  pre.scrollTop = pre.scrollHeight;
  if (/halted|error|Guru|panic|exception/i.test(x)) toast(x.slice(0, 160), true);
}

function fmtTime(ns) {
  const s = ns / 1e9;
  if (s < 60) return s.toFixed(2) + ' s';
  return Math.floor(s / 60) + ':' + String(Math.floor(s % 60)).padStart(2, '0') + ' min';
}

function updateStatus() {
  const s = state.status;
  const st = s.state || 'idle';
  $('#status-text').textContent = st.length > 48 ? st.slice(0, 48) + '…' : st;
  $('#status-text').title = st;
  const dot = $('#status-dot');
  dot.className = 'dot ' + (st === 'running' ? 'running' : st.startsWith('halted') || st.startsWith('no firmware') ? 'halted' : 'paused');
  $('#status-time').textContent = s.time_ns !== undefined ? '⏱ ' + fmtTime(s.time_ns) : '';
  $('#status-mips').textContent = s.cpu_mhz ? `${s.cpu_mhz} MHz · ${(s.mips || 0).toFixed(0)} MIPS` : '';
}

// ------------------------------------------------------------------ build
let buildSeen = false;
function onBuild(b) {
  const pre = $('#build-log');
  for (const l of b.lines || []) {
    const span = el('span', { class: /error|✘|failed/i.test(l) ? 'err' : /warning/i.test(l) ? 'warn' : /✔|SUCCESS/.test(l) ? 'ok' : '' }, l + '\n');
    pre.append(span);
  }
  pre.scrollTop = pre.scrollHeight;
  state.build.diagnostics = b.diagnostics || [];
  renderProblems();
  editor.setDiagnostics(state.build.diagnostics);
  const btn = $('#btn-build');
  btn.disabled = !!b.running;
  btn.textContent = b.running ? '⏳ Building…' : '⚙ Build & Run';
  // notify only about builds that ran while this page was open (the server
  // replays the last result to every new session)
  if (b.running) buildSeen = true;
  if (buildSeen && !b.running && b.ok !== null && b.ok !== undefined) {
    buildSeen = false;
    if (b.ok) toast('Build succeeded — firmware loaded');
    else { toast('Build failed — see the Build / Problems panel', true); showBottom((b.diagnostics || []).some((d) => d.severity !== 'note') ? 'problems' : 'build'); }
  }
}

function renderProblems() {
  const list = $('#problems');
  list.innerHTML = '';
  const d = state.build.diagnostics.filter((x) => x.severity !== 'note');
  $('#problem-count').textContent = d.length ? `(${d.length})` : '';
  for (const p of d) {
    list.append(el('div', { class: 'problem', onclick: () => editor.open(p.file, p.line) },
      el('span', { class: 'sev-' + p.severity }, p.severity === 'error' ? '⛔' : '⚠'),
      el('span', {}, `${p.file}:${p.line}:${p.col}`), el('span', { class: 'muted' }, p.message)));
  }
}

async function build() {
  buildSeen = true;
  editor.saveAll();
  $('#build-log').textContent = '';
  showBottom('build');
  try { await api('/api/build', { method: 'POST', json: {} }); } catch (e) {
    if (/Convert to PlatformIO/.test(e.message) && confirm('This folder is an Arduino sketch without a platformio.ini.\n\nCreate a platformio.ini (ESP32 Dev Module, Arduino framework) so it can be built with PlatformIO?')) {
      try { await api('/api/convert-sketch', { method: 'POST', json: {} }); toast('platformio.ini created — building…'); await editor.refreshTree(); return build(); } catch (e2) { toast(e2.message, true); }
    } else toast(e.message, true);
  }
}

// ------------------------------------------------------------------ layout
function showBottom(name) {
  $$('#bottom-tabs button[data-bottom]').forEach((b) => b.classList.toggle('active', b.dataset.bottom === name));
  $$('.bview').forEach((v) => v.classList.toggle('active', v.id === 'bview-' + name));
  $('#serial-controls').style.display = name === 'serial' ? '' : 'none';
  if (name === 'serial') serial.fit();
  if (name === 'plot') plotter.draw();
  logic.show(name === 'logic');
  busmon.show(name === 'bus');
}
$$('#bottom-tabs button[data-bottom]').forEach((b) => (b.onclick = () => showBottom(b.dataset.bottom)));

function showPanel(name) {
  $$('#activity button[data-panel]').forEach((b) => b.classList.toggle('active', b.dataset.panel === name));
  $$('.panel').forEach((p) => p.classList.toggle('active', p.id === 'panel-' + name));
  if (name === 'git') git.refresh();
  if (name === 'debug') debug.refresh();
  if (name === 'ai') ai.focus();
}
$$('#activity button[data-panel]').forEach((b) => (b.onclick = () => showPanel(b.dataset.panel)));
$$('[data-act=settings]').forEach((b) => (b.onclick = () => openSettings(state)));

// bottom resizer
(() => {
  const r = $('#bottom-resizer'), bottom = $('#bottom');
  r.addEventListener('pointerdown', (e) => {
    r.setPointerCapture(e.pointerId);
    const y0 = e.clientY, h0 = bottom.offsetHeight;
    const mv = (ev) => { bottom.style.height = Math.max(80, Math.min(window.innerHeight - 200, h0 + y0 - ev.clientY)) + 'px'; serial.fit(); editor.layout(); };
    const up = () => { r.removeEventListener('pointermove', mv); r.removeEventListener('pointerup', up); };
    r.addEventListener('pointermove', mv); r.addEventListener('pointerup', up);
  });
})();

// tabs: circuit + editors
editor.onTabsChanged = renderTabs;
function renderTabs() {
  const tabs = $('#tabs');
  tabs.innerHTML = '';
  const active = editor.activeView;
  tabs.append(el('div', { class: 'tab' + (active === 'circuit' ? ' active' : ''), onclick: () => editor.showCircuit() }, el('span', { class: 'name' }, '⚡ Circuit')));
  for (const f of editor.files.values()) {
    const t = el('div', { class: 'tab' + (active === f.path ? ' active' : '') + (f.dirty ? ' dirty' : ''), title: f.path, onclick: () => editor.open(f.path) },
      el('span', { class: 'name' }, f.path.split('/').pop()),
      el('button', { class: 'close', title: 'Close', onclick: (e) => { e.stopPropagation(); editor.close(f.path); } }, '✕'));
    tabs.append(t);
  }
  if (editor.diffOpen) tabs.append(el('div', { class: 'tab' + (active === 'diff' ? ' active' : ''), onclick: () => editor.showDiff() }, el('span', { class: 'name' }, '± ' + editor.diffOpen)));
}

// ------------------------------------------------------------------ toolbar
$('#btn-build').onclick = build;
$('#btn-run').onclick = () => send({ type: 'control', cmd: 'resume' });
$('#btn-pause').onclick = () => send({ type: 'control', cmd: 'pause' });
$('#btn-reset').onclick = () => send({ type: 'control', cmd: 'reset' });
$('#speed').onchange = (e) => send({ type: 'control', cmd: 'speed', value: parseFloat(e.target.value) });
let sound = false;
$('#btn-sound').onclick = () => { sound = !sound; soundEnabled(sound); $('#btn-sound').textContent = sound ? '🔊' : '🔇'; };
$('#cz-fit').onclick = () => circuit.fit();
$('#cz-in').onclick = () => circuit.zoom(1.2);
$('#cz-undo').onclick = () => circuit.undo();
$('#cz-redo').onclick = () => circuit.redo();
$('#cz-out').onclick = () => circuit.zoom(1 / 1.2);

const menu = $('#menu-file');
$('#btn-file').onclick = (e) => { e.stopPropagation(); menu.classList.toggle('open'); renderRecent(); };
document.addEventListener('click', () => menu.classList.remove('open'));
menu.addEventListener('click', (e) => {
  const act = e.target.dataset.act;
  if (!act) return;
  menu.classList.remove('open');
  if (act === 'new') newProjectDialog(reloadAll);
  if (act === 'erase-flash' && confirm('Erase the emulated flash data partitions (NVS / Preferences, SPIFFS, LittleFS, FAT) and reset the board?')) send({ type: 'control', cmd: 'erase-flash' });
  if (act === 'open') openProjectDialog(state, reloadAll);
  if (act === 'save-all') editor.saveAll();
  if (act === 'reload') api('/api/project/reload', { method: 'POST', json: {} }).then(() => toast('Simulation reloaded'));
  if (act === 'settings') openSettings(state);
});
function renderRecent() {
  const r = $('#recent-list');
  r.innerHTML = '';
  const rec = (state.info && state.info.settings.recent) || [];
  if (!rec.length) r.append(el('div', { class: 'recent' }, 'No recent projects'));
  for (const p of rec) r.append(el('button', { class: 'recent', title: p, onclick: () => api('/api/project/open', { json: { path: p } }).then(reloadAll).catch((e) => toast(e.message, true)) }, '↺ ' + p));
}

window.addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === 'b') { e.preventDefault(); build(); }
  if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 's') { e.preventDefault(); editor.saveAll(); }
  if (e.key === 'F5') { e.preventDefault(); send({ type: 'control', cmd: 'resume' }); }
  if (e.key === 'F6') { e.preventDefault(); send({ type: 'control', cmd: 'pause' }); }
  if (e.key === 'F10') { e.preventDefault(); debug.step('over'); }
  if (e.key === 'F11') { e.preventDefault(); debug.step('into'); }
});

// ------------------------------------------------------------------ serial input
$('#serial-form').onsubmit = (e) => {
  e.preventDefault();
  const i = $('#serial-input');
  const eol = $('#serial-eol').value.replace('\\n', '\n').replace('\\r', '\r');
  send({ type: 'serial', data: i.value + eol });
  serial.echo(i.value);
  i.value = '';
};
$('#serial-clear').onclick = () => serial.clear();
let eolTouched = false;
$('#serial-eol').addEventListener('change', () => { eolTouched = true; });
serial.onMicroPython = () => { if (!eolTouched) $('#serial-eol').value = '\\r'; };
serial.onData = (d) => send({ type: 'serial', data: d });

// ------------------------------------------------------------------ parts palette
function renderParts(filter = '') {
  const list = $('#parts-list');
  list.innerHTML = '';
  const f = filter.toLowerCase().trim();
  const cats = {};
  // when searching: best matches first (type/name before description), in one list
  const score = (p) => {
    const n = (p.name + ' ' + p.type).toLowerCase();
    if (p.type === f || p.name.toLowerCase() === f) return 0;
    if (n.split(/[\s(),/-]+/).some((w) => w.startsWith(f))) return 1;
    if (n.includes(f)) return 2;
    if ((p.description + ' ' + p.category).toLowerCase().includes(f)) return 3;
    return 9;
  };
  if (f) {
    const hits = state.catalog.map((p) => [score(p), p]).filter(([sc]) => sc < 9).sort((a, b) => a[0] - b[0]);
    if (hits.length) cats[`Results for “${filter.trim()}”`] = hits.map(([, p]) => p);
  } else {
    for (const p of state.catalog) (cats[p.category] = cats[p.category] || []).push(p);
  }
  for (const [cat, items] of Object.entries(cats)) {
    list.append(el('div', { class: 'part-cat' }, cat));
    for (const p of items) {
      const item = el('div', { class: 'part-item', draggable: 'true', title: p.description + '\nClick or drag onto the circuit', onclick: () => { editor.showCircuit(); circuit.addPartCentered(p.type); } });
      try { item.append(partIcon(p.type)); } catch (e) { /* icon failed */ }
      item.append(el('div', {}, el('div', { class: 'pi-name' }, p.name), el('div', { class: 'pi-desc' }, p.description.slice(0, 60))));
      item.addEventListener('dragstart', (e) => e.dataTransfer.setData('text/part', p.type));
      list.append(item);
    }
  }
}
$('#parts-search').oninput = (e) => renderParts(e.target.value);

// ------------------------------------------------------------------ selection / inspector
bus.on('select', (sel) => inspector.select(sel, circuit));
bus.on('project-saved', (reloaded) => { if (reloaded) toast('Circuit updated — simulation restarted'); editor.reloadIfClean('esp32sim.yaml'); });
// esp32sim.yaml edited as text (or by the AI): show the new circuit
document.addEventListener('project-file-saved', async () => {
  try { state.project = await api('/api/project'); circuit.load(state.project); if (!inspector.sel) inspector.renderPins(); } catch (e) { toast(e.message, true); }
});

// ------------------------------------------------------------------ boot
async function reloadAll() {
  state.info = await api('/api/info');
  state.project = await api('/api/project');
  $('#project-name').textContent = (state.project.name || 'untitled');
  $('#project-name').title = state.info.root;
  document.title = `${state.project.name || 'esp32sim'} — esp32sim`;
  circuit.load(state.project);
  if (!inspector.sel) inspector.renderPins();
  editor.setRoot(state.info.root);
  await editor.refreshTree();
  git.refresh();
  setTimeout(() => circuit.fit(), 50);
}

async function boot() {
  [state.catalog, state.boards] = await Promise.all([api('/api/catalog'), api('/api/boards')]);
  circuit.setCatalog(state.catalog, state.boards);
  renderParts();
  await reloadAll();
  connect();
  editor.init().then(renderTabs);
  showBottom('serial');
  renderTabs();
  if (!state.info.settings.recent.length && !(state.project.components || []).length && !state.project.firmware) {
    setTimeout(() => newProjectDialog(reloadAll, true), 300);
  }
}
boot().catch((e) => { console.error(e); toast('Startup failed: ' + e.message, true); });
window.addEventListener('resize', () => { serial.fit(); editor.layout(); });
