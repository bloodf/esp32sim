// Circuit editor model: pin naming shared with the simulator, wiring,
// editing, undo/redo, persistence and live state.
import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';
import { FIXTURE, onFetch, requests, tick } from './setup.mjs';
const { Circuit, padNumber } = await import('../../web/js/circuit.js');
const { bus } = await import('../../web/js/api.js');

test('padNumber agrees with every labelled pin of every board', () => {
  for (const b of FIXTURE.boards) {
    for (const p of [...b.left, ...b.right]) {
      if (p.gpio < 0) continue;
      assert.equal(padNumber(p.label), p.gpio, `${b.id}: ${p.label}`);
    }
  }
});

test('padNumber matches the simulator on every label it knows', () => {
  const table = FIXTURE.pad_numbers;
  assert.ok(Object.keys(table).length > 400);
  for (const [label, gpio] of Object.entries(table)) assert.equal(padNumber(label), gpio, JSON.stringify(label));
});

test('padNumber understands the usual aliases', () => {
  const cases = { D4: 4, GPIO13: 13, IO0: 0, P25: 25, G5: 5, '21': 21, VP: 36, VN: 39, TX2: 17, RX0: 3, SDA: 21, SCL: 22, MOSI: 23, MISO: 19, SCK: 18, SS: 5, LED: 2, BOOT: 0, A0: 36, DAC1: 25, T0: 4, T9: 32, sd2: 9, CMD: 11 };
  for (const [k, v] of Object.entries(cases)) assert.equal(padNumber(k), v, k);
  for (const bad of ['X', 'GPIOX', '', 'banana', 'GPIO20', 'D29', 'GPIO40', 'IO24']) assert.equal(padNumber(bad), null, bad);
});

function newCircuit() {
  const el = document.getElementById('canvas');
  const c = new Circuit(el);
  c.setCatalog(FIXTURE.parts, FIXTURE.boards);
  return c;
}

const PROJECT = () => ({
  name: 't',
  board: 'esp32-devkit-v1',
  components: [
    { id: 'led1', type: 'led', color: 'red', x: 400, y: 100 },
    { id: 'r1', type: 'resistor', value: '220', x: 320, y: 100 },
    { id: 'oled1', type: 'ssd1306', x: 500, y: 250 },
  ],
  connections: [['esp32.D4', 'r1.1'], 'r1.2 -> led1.A', { from: 'led1.C', to: 'GND', color: '#123456' }, ['esp32.D21', 'oled1.SDA'], ['esp32.D22', 'oled1.SCL']],
  layout: {},
});

beforeEach(() => onFetch(async () => ({ json: { ok: true } })));

test('loading draws the board, every part and every wire', () => {
  const c = newCircuit();
  c.load(PROJECT());
  assert.equal(c.parts.size, 3);
  const wires = document.querySelectorAll('#canvas polyline.wire');
  assert.equal(wires.length, 5, 'one polyline per wire segment');
  assert.equal(wires[2].getAttribute('stroke'), '#123456', 'explicit wire colour');
  assert.equal(wires[0].getAttribute('stroke'), wires[0].getAttribute('stroke'));
  // pin dots carry their endpoint for tests and tooling
  for (const ep of ['esp32.GPIO4', 'r1.1', 'led1.A', 'oled1.SDA']) {
    assert.ok(document.querySelector(`[data-ep="${ep}"]`), ep);
  }
});

test('connection formats are normalised', () => {
  const c = newCircuit();
  c.load({ ...PROJECT(), connections: [['a.1', 'b.1', 'c.1'], 'x.1 - y.1', 'p.1 <-> q.1', { from: 'm.1', to: 'n.1', points: [[1, 2]] }] });
  const l = c.connList();
  assert.deepEqual(l.map((w) => [w.a, w.b]), [['a.1', 'b.1'], ['b.1', 'c.1'], ['x.1', 'y.1'], ['p.1', 'q.1'], ['m.1', 'n.1']]);
  assert.deepEqual(l[4].points, [[1, 2]]);
});

test('endpoint positions: board pads, rails, part pins, case-insensitive', () => {
  const c = newCircuit();
  c.load(PROJECT());
  const d4 = c.endpointPos('esp32.D4');
  assert.deepEqual(c.endpointPos('esp32.GPIO4'), d4);
  assert.deepEqual(c.endpointPos('ESP32.gpio4'), d4);
  assert.ok(c.endpointPos('GND') && c.endpointPos('3V3') && c.endpointPos('5V'));
  const gnds = c.boardPins.get('GND');
  assert.ok(gnds.length >= 2, 'GND exists on several header pins');
  // the rail pin nearest to the other wire end is used
  const far = { x: 10000, y: 0 };
  const near = c.endpointPos('GND', far);
  assert.ok(gnds.every((g) => Math.abs(g.x - far.x) >= Math.abs(near.x - far.x)));
  assert.ok(c.endpointPos('LED1.a'));
  assert.equal(c.endpointPos('led1.Z'), null);
  assert.equal(c.endpointPos('ghost.1'), null);
  assert.equal(c.endpointPos('esp32.D99'), null);
});

test('wires to the left header are routed around the board', () => {
  const c = newCircuit();
  c.load({ ...PROJECT(), connections: [['esp32.D13', 'led1.A']] });
  const pts = document.querySelector('#canvas polyline.wire').getAttribute('points').split(' ').map((p) => p.split(',').map(Number));
  const bb = c.boardBox;
  // no segment goes straight through the board body
  for (let i = 1; i < pts.length; i++) {
    const [x0, y0] = pts[i - 1], [x1, y1] = pts[i];
    if (y0 === y1 && y0 > bb.y0 && y0 < bb.y1) assert.ok(Math.max(x0, x1) <= bb.x0 + 1 || Math.min(x0, x1) >= bb.x1 - 1, `segment ${pts[i - 1]} -> ${pts[i]} crosses the board`);
  }
});

test('adding parts picks friendly unique ids and catalog defaults', async () => {
  const c = newCircuit();
  c.load(PROJECT());
  c.addPart('resistor', 201, 99);
  c.addPart('resistor', 300, 300);
  c.addPart('ads1115', 0, 0);
  c.addPart('7segment', 0, 0);
  const ids = c.project.components.map((x) => x.id);
  assert.deepEqual(ids.slice(3), ['r2', 'r3', 'ads11151', 'seg1']);
  const r2 = c.project.components[3];
  assert.equal(r2.value, '220', 'catalog default props');
  assert.deepEqual([r2.x, r2.y], [200, 100], 'snapped to the 5 px grid');
  assert.equal(c.selected.id, 'seg1');
  await tick(200);
  const put = requests.find((r) => r.method === 'PUT' && r.url === '/api/project');
  assert.ok(put, 'project saved');
  assert.equal(put.body.reload, true);
  assert.equal(put.body.components.length, 7);
});

test('new parts are placed beside the board without overlapping', () => {
  const c = newCircuit();
  c.load({ ...PROJECT(), components: [] });
  for (let i = 0; i < 4; i++) c.addPartCentered('ssd1306');
  const boxes = [...c.parts.values()].map((p) => ({ x0: p.cfg.x + p.r.ox, y0: p.cfg.y + p.r.oy, x1: p.cfg.x + p.r.ox + p.r.w, y1: p.cfg.y + p.r.oy + p.r.h }));
  const bb = c.boardBox;
  for (const [i, a] of boxes.entries()) {
    assert.ok(a.x0 >= bb.x1 || a.y0 >= bb.y1, `part ${i} not on the board`);
    for (const b of boxes.slice(i + 1)) assert.ok(a.x1 <= b.x0 || b.x1 <= a.x0 || a.y1 <= b.y0 || b.y1 <= a.y0, 'no overlap');
  }
});

test('drawing a wire by clicking two pins', async () => {
  const c = newCircuit();
  c.load(PROJECT());
  const down = (ep) => document.querySelector(`[data-ep="${ep}"]`).dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }));
  down('oled1.VCC');
  assert.ok(c.wiring, 'wiring started');
  assert.ok(document.getElementById('circuit-hint').classList.contains('active'));
  down('esp32.GPIO5');
  assert.equal(c.wiring, null);
  assert.deepEqual(c.project.connections.at(-1), { from: 'oled1.VCC', to: 'esp32.GPIO5' });
  // Esc-style cancel and same-pin clicks add nothing
  down('led1.A');
  c.cancelWire();
  down('led1.A');
  down('led1.A');
  assert.equal(c.project.connections.length, 6);
});

test('deleting a part removes its wires; undo and redo restore them', async () => {
  const c = newCircuit();
  c.load(PROJECT());
  c.select({ type: 'part', id: 'r1' });
  c.deleteSelected();
  assert.deepEqual(c.project.components.map((x) => x.id), ['led1', 'oled1']);
  assert.equal(c.project.connections.length, 3, 'r1 wires removed');
  c.undo();
  assert.equal(c.project.components.length, 3);
  assert.equal(c.project.connections.length, 5);
  c.redo();
  assert.equal(c.project.components.length, 2);
  // deleting a single wire
  c.undo();
  c.select({ type: 'wire', index: 0 });
  c.deleteSelected();
  assert.deepEqual(c.project.connections[0], 'r1.2 -> led1.A');
  await tick(200);
  assert.ok(requests.filter((r) => r.method === 'PUT').length >= 1);
});

test('ids with regex characters are handled when deleting', () => {
  const c = newCircuit();
  c.load({ ...PROJECT(), components: [{ id: 'a.b+c', type: 'led', x: 400, y: 300 }], connections: [['a.b+c.A', 'GND'], ['esp32.D2', 'a.b+c.C']] });
  c.select({ type: 'part', id: 'a.b+c' });
  c.deleteSelected();
  assert.equal(c.project.connections.length, 0);
});

test('rotating a part turns its pins', () => {
  const c = newCircuit();
  c.load(PROJECT());
  const before = c.endpointPos('r1.2');
  c.select({ type: 'part', id: 'r1' });
  c.rotateSelected();
  const after = c.endpointPos('r1.2');
  assert.equal(c.project.components[1].rotate, 90);
  assert.notDeepEqual(before, after);
});

test('undo history is bounded and redo is cleared by new edits', () => {
  const c = newCircuit();
  c.load(PROJECT());
  for (let i = 0; i < 210; i++) { c.project.components[0].x = 400 + i * 5; c.changed(false); }
  assert.equal(c.history.length, 200);
  c.undo();
  assert.equal(c.future.length, 1);
  c.project.components[0].x = 1; c.changed(false);
  assert.equal(c.future.length, 0);
});

test('live state reaches the part drawings and the board LED', () => {
  const c = newCircuit();
  c.load(PROJECT());
  c.updateStates({
    components: [
      { id: 'led1', kind: 'led', state: { brightness: 0.8, current_ma: 5 }, summary: '' },
      { id: 'board_led', kind: 'led', state: { brightness: 1 } },
      { id: 'ghost', kind: 'led', state: {} },
    ],
    pads: { 4: { v: 3.3, mode: 'out' }, 21: { v: null, mode: 'in' } },
  });
  assert.equal(c.ioLed.getAttribute('fill'), '#40a0ff');
  assert.equal(c.pinVoltage('esp32.GPIO4'), 'out, 3.30 V');
  assert.equal(c.pinVoltage('esp32.GPIO21'), 'in, floating');
  assert.equal(c.pinVoltage('esp32.GPIO5'), 'not connected');
  assert.equal(c.pinVoltage('GND'), '0 V');
  assert.equal(c.pinVoltage('led1.A'), null);
});

test('board buttons: EN resets, BOOT presses GPIO0', () => {
  const c = newCircuit();
  c.load(PROJECT());
  const got = [];
  bus.on('control', (x) => got.push(['control', x]));
  bus.on('input', (id, a) => got.push(['input', id, a.action]));
  const buttons = [...document.querySelectorAll('#canvas .interactive')].filter((g) => /EN|BOOT/.test(g.textContent));
  for (const b of buttons) {
    b.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }));
    b.dispatchEvent(new MouseEvent('pointerup', { bubbles: true }));
  }
  assert.deepEqual(got, [['control', 'reset'], ['input', 'boot_button', 'press'], ['input', 'boot_button', 'release']]);
});

test('every board variant renders all its header pins', () => {
  for (const b of FIXTURE.boards) {
    const c = newCircuit();
    c.load({ ...PROJECT(), board: b.id, components: [], connections: [] });
    for (const p of [...b.left, ...b.right]) {
      if (p.gpio >= 0) assert.ok(document.querySelector(`[data-ep="esp32.GPIO${p.gpio}"]`), `${b.id} GPIO${p.gpio}`);
    }
    assert.equal(c.boardDef().id, b.id);
  }
});

test('save failures are reported, not thrown', async () => {
  const c = newCircuit();
  c.load(PROJECT());
  onFetch(async () => ({ status: 400, json: { error: "unknown pin 'x.9'" } }));
  await c.save(true);
  assert.match(document.getElementById('toast').textContent, /Circuit not saved: unknown pin 'x.9'/);
});
