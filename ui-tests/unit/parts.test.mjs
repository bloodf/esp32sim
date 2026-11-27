// Every part of the Rust catalog has a drawing in the GUI whose pins match
// the simulator's pin names exactly (otherwise wires cannot be attached), and
// the drawings accept the simulator's state updates.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { FIXTURE } from './setup.mjs';
const { renderPart, partIcon } = await import('../../web/js/parts.js');

const ctx = (log = []) => ({ id: 'x', input: (a) => log.push(a) });

test('fixture covers the whole catalog', () => {
  assert.ok(FIXTURE.parts.length >= 85, `${FIXTURE.parts.length} parts`);
});

for (const part of FIXTURE.parts) {
  test(`${part.type}: drawing has the simulator's pins and accepts state`, () => {
    const r = renderPart(part.type, { ...part.props }, ctx(), part);
    assert.ok(r.g && r.g.namespaceURI === 'http://www.w3.org/2000/svg', 'svg group');
    assert.ok(r.w > 0 && r.h > 0, `size ${r.w}x${r.h}`);
    const drawn = r.pins.map((p) => p.name).sort();
    const sim = part.pins.map((p) => p.name).sort();
    assert.deepEqual(drawn, sim, 'pin names');
    for (const p of r.pins) assert.ok(Number.isFinite(p.x) && Number.isFinite(p.y), `pin ${p.name} position`);
    const pos = new Set(r.pins.map((p) => `${Math.round(p.x)},${Math.round(p.y)}`));
    assert.equal(pos.size, r.pins.length, 'no two pins at the same spot');
    if (r.update) r.update(part.state || {}, { id: 'x', kind: part.type, state: part.state, summary: 'summary text' });
    assert.ok(partIcon(part.type).querySelector('g'), 'palette icon');
  });
}

test('unknown kinds fall back to a generic module with the given pins', () => {
  const r = renderPart('mystery-board', {}, ctx(), { name: 'Mystery', pins: [{ name: 'A' }, { name: 'B' }] });
  assert.deepEqual(r.pins.map((p) => p.name), ['A', 'B']);
  r.update({}, { summary: 'hello' });
  assert.match(r.g.textContent, /hello/);
});

test('LED colour follows the props and brightness the state', () => {
  const r = renderPart('led', { color: 'blue' }, ctx(), null);
  const before = r.g.innerHTML;
  r.update({ brightness: 1, current_ma: 10 }, { state: { brightness: 1 } });
  assert.notEqual(r.g.innerHTML, before, 'lit LED looks different');
  r.update({ burned: true, brightness: 0 }, { state: { burned: true } });
});

test('resistor drawing shows the colour bands of its value', () => {
  const a = renderPart('resistor', { value: '220' }, ctx(), null).g.innerHTML;
  const b = renderPart('resistor', { value: '10k' }, ctx(), null).g.innerHTML;
  assert.notEqual(a, b);
  // 220 Ω: red red brown
  assert.ok(a.includes('#e53935') && a.includes('#7b3f00'), 'red and brown bands');
});

test('interactive parts send inputs to the simulator', () => {
  const log = [];
  const r = renderPart('pushbutton', {}, ctx(log), null);
  const target = r.g.querySelector('.interactive') || r.g;
  target.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }));
  target.dispatchEvent(new MouseEvent('pointerup', { bubbles: true }));
  assert.ok(log.length >= 1, 'button press reported');
  assert.equal(log[0].action, 'press');
});
