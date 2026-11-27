// AI assistant panel: request context, streamed answers, markdown safety and
// applying proposed files.
import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';
import { onFetch, requests, confirmAnswers } from './setup.mjs';
const { AiPanel } = await import('../../web/js/ai.js');

function fakeEditor() {
  const ed = {
    replaced: [], diffs: [], inserted: [],
    current: () => ({ path: 'src/main.cpp', model: { getValue: () => 'int led = 5;' } }),
    getSelection: () => 'led',
    showDiff: (title, a, b) => ed.diffs.push([title, a, b]),
    replaceFile: async (p, c) => ed.replaced.push([p, c]),
    editor: { getSelection: () => ({ startLineNumber: 1 }), executeEdits: (src, edits) => ed.inserted.push(edits[0].text) },
  };
  return ed;
}

function panel(ed = fakeEditor()) {
  window.esp32sim = { info: { settings: { ai: { provider: 'anthropic', model: '' } } } };
  return new AiPanel(document.createElement('div'), ed);
}

beforeEach(() => onFetch(async () => ({ text: 'ok' })));

test('shows the configured provider', () => {
  const p = panel();
  assert.match(p.providerLine.textContent, /anthropic · default model/);
  window.esp32sim.info.settings.ai = {};
  p.updateProvider();
  assert.match(p.providerLine.textContent, /No AI provider configured/);
});

test('asking sends the conversation and the editor context', async () => {
  const p = panel();
  onFetch(async () => ({ text: 'Use **GPIO5** and `digitalWrite`.' }));
  await p.ask('  why is my LED off?  ');
  const r = requests.find((x) => x.url === '/api/ai/chat');
  assert.equal(r.method, 'POST');
  assert.deepEqual(r.body.messages, [{ role: 'user', content: 'why is my LED off?' }]);
  assert.deepEqual(r.body.context, { file: 'src/main.cpp', content: 'int led = 5;', selection: 'led' });
  const last = p.log.lastChild;
  assert.equal(last.querySelector('b').textContent, 'GPIO5');
  assert.equal(last.querySelector('code').textContent, 'digitalWrite');
  assert.equal(p.history.length, 2);
  assert.equal(p.history[1].role, 'assistant');
});

test('only the last 12 messages are sent', async () => {
  const p = panel();
  for (let i = 0; i < 10; i++) await p.ask(`q${i}`);
  const last = requests.filter((x) => x.url === '/api/ai/chat').at(-1);
  assert.equal(last.body.messages.length, 12);
  assert.equal(last.body.messages.at(-1).content, 'q9');
});

test('model output is escaped, never injected as HTML', async () => {
  const p = panel();
  onFetch(async () => ({ text: '<img src=x onerror="alert(1)"> <script>bad()</script>' }));
  await p.ask('hi');
  const last = p.log.lastChild;
  assert.equal(last.querySelector('img'), null);
  assert.equal(last.querySelector('script'), null);
  assert.match(last.textContent, /<script>bad\(\)<\/script>/);
});

test('a file=... code block can be diffed and applied', async () => {
  const ed = fakeEditor();
  const p = panel(ed);
  const code = 'void setup() {}\nvoid loop() {}\n';
  onFetch(async (url) => (url.startsWith('/api/file') ? { json: { content: 'old' } } : { text: 'Here:\n```cpp file=src/main.cpp\n' + code + '```\nDone.' }));
  await p.ask('fix it');
  const btns = [...p.log.lastChild.querySelectorAll('button')].map((b) => b.textContent);
  assert.deepEqual(btns, ['Copy', 'Diff', 'Apply to src/main.cpp']);
  const byText = (t) => [...p.log.lastChild.querySelectorAll('button')].find((b) => b.textContent === t);
  byText('Diff').click();
  await new Promise((r) => setTimeout(r, 10));
  assert.deepEqual(ed.diffs[0], ['src/main.cpp (AI proposal)', 'old', code]);
  confirmAnswers.push(false);
  await p.apply('src/main.cpp', code);
  assert.equal(ed.replaced.length, 0, 'declined');
  byText('Apply to src/main.cpp').click();
  await new Promise((r) => setTimeout(r, 10));
  assert.deepEqual(ed.replaced[0], ['src/main.cpp', code]);
});

test('a plain code block can be inserted at the cursor', async () => {
  const ed = fakeEditor();
  const p = panel(ed);
  onFetch(async () => ({ text: '```cpp\ndelay(10);\n```' }));
  await p.ask('snippet');
  [...p.log.lastChild.querySelectorAll('button')].find((b) => b.textContent === 'Insert at cursor').click();
  assert.deepEqual(ed.inserted, ['delay(10);\n']);
});

test('HTTP errors are shown in the chat', async () => {
  const p = panel();
  onFetch(async () => ({ status: 500, text: 'boom' }));
  await p.ask('hi');
  assert.match(p.log.lastChild.textContent, /Error: boom/);
  assert.equal(p.busy, false);
});

test('empty questions and questions while busy are ignored', async () => {
  const p = panel();
  await p.ask('   ');
  assert.equal(requests.length, 0);
  p.busy = true;
  await p.ask('hello');
  assert.equal(requests.length, 0);
});

test('quick actions send their prompt', async () => {
  const p = panel();
  const btn = [...p.root.querySelectorAll('#ai-quick button')].find((b) => b.textContent === 'Check wiring');
  btn.click();
  await new Promise((r) => setTimeout(r, 20));
  const r = requests.find((x) => x.url === '/api/ai/chat');
  assert.match(r.body.messages[0].content, /Check the circuit wiring/);
});
