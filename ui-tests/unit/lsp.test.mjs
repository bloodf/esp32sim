// Built-in Arduino completions (used until clangd answers).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import './setup.mjs';
const { arduinoCompletions } = await import('../../web/js/lsp.js');

function fakeMonaco() {
  const m = {
    provider: null,
    languages: {
      CompletionItemKind: { Method: 1, Function: 2, Constant: 3, Snippet: 4 },
      CompletionItemInsertTextRule: { InsertAsSnippet: 4 },
      registerCompletionItemProvider: (lang, p) => { m.lang = lang; m.provider = p; },
    },
    editor: { getModelMarkers: () => [] },
    Range: class { constructor(a, b, c, d) { Object.assign(this, { a, b, c, d }); } },
  };
  arduinoCompletions(m);
  return m;
}

const model = (line) => ({ uri: 'file:///a.cpp', getLineContent: () => line, getWordUntilPosition: () => ({ startColumn: line.length + 1, endColumn: line.length + 1 }) });

test('registers for C++ with "." as trigger', () => {
  const m = fakeMonaco();
  assert.equal(m.lang, 'cpp');
  assert.deepEqual(m.provider.triggerCharacters, ['.']);
});

test('top level: functions, constants and the sketch skeleton', () => {
  const m = fakeMonaco();
  const s = m.provider.provideCompletionItems(model('  digi'), { lineNumber: 1, column: 7 }).suggestions;
  const labels = s.map((x) => x.label);
  for (const want of ['digitalWrite', 'pinMode', 'HIGH', 'INPUT_PULLUP', 'setup/loop']) assert.ok(labels.includes(want), want);
  assert.ok(s.find((x) => x.label === 'setup/loop').insertText.includes('void loop()'));
});

test('after "Serial." only Serial members are offered', () => {
  const m = fakeMonaco();
  const s = m.provider.provideCompletionItems(model('  Serial.'), { lineNumber: 1, column: 10 }).suggestions;
  assert.ok(s.length > 3);
  assert.ok(s.every((x) => !x.label.includes('.')), 'member names only');
  assert.ok(s.some((x) => x.label === 'println'), s.map((x) => x.label).join(','));
  assert.ok(!s.some((x) => x.label === 'HIGH'), 'no constants after a dot');
});
