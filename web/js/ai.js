// AI assistant panel: chat with the configured provider, with project context.
import { el, toast, escapeHtml, $, bus } from './api.js';
import { openSettings } from './settings.js';

const QUICK = [
  ['Explain', 'Explain what the current file does, step by step, and how it uses the circuit.'],
  ['Find bugs', 'Review the current file for bugs, race conditions, wrong pins or ESP32 pitfalls. List concrete problems and fixes.'],
  ['Improve', 'Improve the current file: structure, non-blocking timing (millis instead of delay), robustness. Return the complete updated file.'],
  ['Fix build', 'Fix the build errors. Return the complete corrected file(s).'],
  ['Check wiring', 'Check the circuit wiring against the code: pins, pull-ups, voltage levels, I2C addresses, current-limiting resistors. Point out mistakes.'],
  ['Write tests', 'Write a `test:` scenario for esp32sim.yaml that exercises this firmware (serial expectations, button presses, sensor values).'],
];

export class AiPanel {
  constructor(root, editor) {
    this.root = root;
    this.editor = editor;
    this.history = [];
    this.busy = false;
    this.render();
    $('#ai-clear').onclick = () => { this.history = []; this.log.innerHTML = ''; this.welcome(); };
    $('#ai-settings').onclick = () => openSettings(window.esp32sim, 'ai');
    bus.on('settings-changed', () => this.updateProvider());
  }

  render() {
    this.root.innerHTML = '';
    this.providerLine = el('div', { class: 'ai-provider' });
    this.log = el('div', { id: 'ai-log' });
    const quick = el('div', { id: 'ai-quick' }, ...QUICK.map(([t, p]) => el('button', { onclick: () => this.ask(p) }, t)));
    this.input = el('textarea', { id: 'ai-input', placeholder: 'Ask about your code or circuit… (Enter to send, Shift+Enter for a new line)' });
    this.input.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); this.ask(this.input.value); } });
    this.root.append(this.providerLine, this.log, quick, this.input);
    this.welcome();
    this.updateProvider();
  }

  updateProvider() {
    const s = window.esp32sim && window.esp32sim.info && window.esp32sim.info.settings;
    const ai = s ? s.ai : {};
    this.providerLine.innerHTML = '';
    this.providerLine.append(el('span', {}, ai && ai.provider ? `${ai.provider} · ${ai.model || 'default model'}` : 'No AI provider configured'), el('a', { href: '#', onclick: (e) => { e.preventDefault(); openSettings(window.esp32sim, 'ai'); } }, 'settings'));
  }

  welcome() {
    this.log.append(el('div', { class: 'msg assistant' }, 'Hi! I can explain code, find bugs, fix build errors and check your wiring. I see the open file, your selection, the circuit, the serial output and build errors. Code blocks I return can be applied to files with one click.'));
  }

  focus() { this.updateProvider(); setTimeout(() => this.input.focus(), 50); }

  async ask(text) {
    text = (text || '').trim();
    if (!text || this.busy) return;
    this.input.value = '';
    this.history.push({ role: 'user', content: text });
    this.log.append(el('div', { class: 'msg user' }, text));
    const out = el('div', { class: 'msg assistant' }, '…');
    this.log.append(out);
    this.log.scrollTop = this.log.scrollHeight;
    const f = this.editor.current();
    const context = f ? { file: f.path, content: f.model.getValue(), selection: this.editor.getSelection() } : {};
    this.busy = true;
    let acc = '';
    try {
      const r = await fetch('/api/ai/chat', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ messages: this.history.slice(-12), context }) });
      if (!r.ok) throw new Error(await r.text());
      const reader = r.body.getReader();
      const dec = new TextDecoder();
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        acc += dec.decode(value, { stream: true });
        this.renderMarkdown(out, acc, false);
        this.log.scrollTop = this.log.scrollHeight;
      }
    } catch (e) {
      acc += `\n\n**Error:** ${e.message}`;
    }
    this.busy = false;
    this.history.push({ role: 'assistant', content: acc });
    this.renderMarkdown(out, acc, true);
    if (/no API key configured/.test(acc)) openSettings(window.esp32sim, 'ai');
  }

  renderMarkdown(node, text, final) {
    node.innerHTML = '';
    const parts = text.split(/```/);
    parts.forEach((part, i) => {
      if (i % 2 === 0) {
        let h = escapeHtml(part)
          .replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>')
          .replace(/`([^`]+)`/g, '<code>$1</code>')
          .replace(/^#{1,4} (.*)$/gm, '<b>$1</b>')
          .replace(/^\s*[-*] (.*)$/gm, '• $1')
          .replace(/\n/g, '<br>');
        node.append(el('span', {}, ''));
        node.lastChild.innerHTML = h;
      } else {
        const nl = part.indexOf('\n');
        const info = nl >= 0 ? part.slice(0, nl).trim() : '';
        const code = nl >= 0 ? part.slice(nl + 1) : part;
        const fileM = info.match(/file=([^\s]+)/);
        const pre = el('pre', {}, code);
        node.append(pre);
        if (final) {
          const acts = el('div', { class: 'code-actions' }, el('button', { onclick: () => { navigator.clipboard.writeText(code); toast('Copied'); } }, 'Copy'));
          if (fileM) {
            const path = fileM[1];
            acts.append(el('button', { onclick: () => this.preview(path, code) }, 'Diff'), el('button', { class: 'primary', onclick: () => this.apply(path, code) }, `Apply to ${path}`));
          } else if (this.editor.current()) {
            acts.append(el('button', { onclick: () => this.insert(code) }, 'Insert at cursor'));
          }
          node.append(acts);
        }
      }
    });
  }

  async preview(path, code) {
    let cur = '';
    try { cur = (await fetch('/api/file?path=' + encodeURIComponent(path)).then((r) => r.json())).content || ''; } catch (e) { /* new file */ }
    this.editor.showDiff(path + ' (AI proposal)', cur, code);
  }

  async apply(path, code) {
    if (!confirm(`Replace ${path} with the proposed version?`)) return;
    try { await this.editor.replaceFile(path, code.replace(/\n$/, '') + '\n'); toast(`Applied to ${path}`); } catch (e) { toast(e.message, true); }
  }

  insert(code) {
    const ed = this.editor.editor;
    const sel = ed.getSelection();
    ed.executeEdits('ai', [{ range: sel, text: code, forceMoveMarkers: true }]);
  }
}
