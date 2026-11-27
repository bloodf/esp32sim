// Serial plotter: which serial lines become samples, and how series grow.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tick } from './setup.mjs';
const { Plotter } = await import('../../web/js/plotter.js');

const mk = () => new Plotter(document.createElement('div'));
const vals = (p, name) => p.series.get(name)?.values;

test('space, comma and tab separated numbers become value series', () => {
  const p = mk();
  p.feed('1 2 3\n4,5,6\n7\t8\t9\n');
  assert.deepEqual(vals(p, 'value 1'), [1, 4, 7]);
  assert.deepEqual(vals(p, 'value 3'), [3, 6, 9]);
  assert.equal(p.count, 3);
});

test('labelled values: name:value and name=value', () => {
  const p = mk();
  p.feed('temp:21.5 hum:40\r\ntemp=22.25, hum=41\n');
  assert.deepEqual(vals(p, 'temp'), [21.5, 22.25]);
  assert.deepEqual(vals(p, 'hum'), [40, 41]);
  p.feed('x:-1.5e3\n');
  assert.deepEqual(vals(p, 'x'), [-1500]);
});

test('ordinary log text is not plotted', () => {
  const p = mk();
  p.feed('Booting 3 times\nWiFi connected: 192.168.1.2\nreading temp:21 from sensor\n\n   \n');
  assert.equal(p.series.size, 0);
  assert.equal(p.count, 0);
});

test('lines split across serial chunks are joined', () => {
  const p = mk();
  p.feed('1');
  p.feed('0 2');
  assert.equal(p.series.size, 0, 'incomplete line waits');
  p.feed('0\n');
  assert.deepEqual(vals(p, 'value 1'), [10]);
  assert.deepEqual(vals(p, 'value 2'), [20]);
});

test('pause, clear and history limit', async () => {
  const p = mk();
  p.paused = true;
  p.feed('1 2\n');
  assert.equal(p.series.size, 0);
  p.paused = false;
  for (let i = 0; i < 4100; i++) p.line(String(i));
  assert.equal(vals(p, 'value 1').length, 4000);
  assert.equal(vals(p, 'value 1')[0], 100);
  await tick(30); // the throttled redraw runs
  p.draw();
  assert.match(p.legend.textContent, /value 1/);
  const n = p.series.size;
  assert.equal(n, 1);
  [...p.root.querySelectorAll('button')].find((b) => /Clear/.test(b.textContent)).click();
  assert.equal(p.series.size, 0);
});

test('a very long line without newline is dropped', () => {
  const p = mk();
  p.feed('9'.repeat(1200));
  p.feed('\n1 2\n');
  assert.deepEqual(vals(p, 'value 1'), [1]);
});
