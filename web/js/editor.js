// Code editor (Monaco): files, tabs, diagnostics, breakpoints, IntelliSense.
import { api, $, $$, el, toast } from './api.js';
import { LspClient, arduinoCompletions } from './lsp.js';

const LANG = { c: 'cpp', cpp: 'cpp', cc: 'cpp', h: 'cpp', hpp: 'cpp', ino: 'cpp', ini: 'ini', yaml: 'yaml', yml: 'yaml', py: 'python', md: 'markdown', json: 'json', sh: 'shell', txt: 'plaintext', csv: 'plaintext' };

export class Editor {
  constructor(state) {
    this.state = state;
    this.files = new Map();
    this.activeView = 'circuit';
    this.root = '';
    this.monaco = null;
    this.editor = null;
    this.diffEditor = null;
    this.diffOpen = null;
    this.breakpoints = new Map(); // path -> Set(line)
    this.bpDecorations = new Map();
    this.execDecoration = [];
    this.onTabsChanged = () => {};
    this.lsp = null;
    this.pendingOpen = [];
    this.selectedTreePath = null;
  }

  setRoot(root) {
    if (this.root && this.root !== root) {
      for (const f of this.files.values()) f.model.dispose();
      this.files.clear();
      this.showCircuit();
    }
    this.root = root;
  }

  init() {
    return new Promise((resolve) => {
      window.MonacoEnvironment = { getWorkerUrl: () => 'vendor/monaco/vs/base/worker/workerMain.js' };
      window.require.config({ paths: { vs: 'vendor/monaco/vs' } });
      window.require(['vs/editor/editor.main'], () => {
        this.monaco = window.monaco;
        const m = this.monaco;
        m.editor.defineTheme('esp32sim', { base: 'vs-dark', inherit: true, rules: [], colors: { 'editor.background': '#1e1f22' } });
        this.editor = m.editor.create($('#editor'), {
          theme: 'esp32sim', automaticLayout: true, glyphMargin: true, fontSize: 13, minimap: { enabled: true, scale: 1 },
          fontFamily: '"JetBrains Mono", "Fira Code", Menlo, Consolas, monospace', tabSize: 2, renderWhitespace: 'selection',
          fixedOverflowWidgets: true, smoothScrolling: true, inlineSuggest: { enabled: true },
        });
        this.diffEditor = m.editor.createDiffEditor($('#diff-editor'), { theme: 'esp32sim', automaticLayout: true, readOnly: false, renderSideBySide: true });
        this.editor.onDidChangeModelContent(() => {
          const f = this.current();
          if (f && !f.dirty) { f.dirty = true; this.onTabsChanged(); }
          if (f && this.lsp) this.lsp.change(f);
        });
        this.editor.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, () => this.save());
        // AI actions in the editor's context menu (handled by the AI panel)
        const aiAction = (id, label, prompt, key) => this.editor.addAction({
          id, label, contextMenuGroupId: '9_ai', keybindings: key ? [key] : [],
          run: () => document.dispatchEvent(new CustomEvent('ai-ask', { detail: prompt })),
        });
        aiAction('ai.explain', 'AI: Explain selection', 'Explain the selected code: what it does, how it interacts with the hardware/circuit, and any pitfalls.', m.KeyMod.CtrlCmd | m.KeyMod.Shift | m.KeyCode.KeyE);
        aiAction('ai.fix', 'AI: Find bugs in selection', 'Review the selected code for bugs and ESP32 pitfalls. Return a corrected version of the complete file if changes are needed.');
        aiAction('ai.improve', 'AI: Improve selection', 'Improve the selected code (readability, non-blocking timing, robustness). Return the complete updated file.');
        aiAction('ai.comment', 'AI: Document selection', 'Add clear comments to the selected code. Return the complete updated file.');
        this.editor.onMouseDown((e) => {
          if (e.target.type === m.editor.MouseTargetType.GUTTER_GLYPH_MARGIN || e.target.type === m.editor.MouseTargetType.GUTTER_LINE_NUMBERS && e.event.leftButton && e.event.shiftKey) {
            this.toggleBreakpoint(this.current().path, e.target.position.lineNumber);
          }
        });
        arduinoCompletions(m);
        // AI inline completions (ghost text), when enabled in Settings
        const aiInline = () => { const st = window.esp32sim && window.esp32sim.info && window.esp32sim.info.settings; return st && st.ai && st.ai.inline && st.ai.provider; };
        for (const lang of ['cpp', 'c', 'python']) {
          m.languages.registerInlineCompletionsProvider(lang, {
            provideInlineCompletions: async (model, pos, ctx, token) => {
              if (!aiInline()) return { items: [] };
              // debounce: only ask once typing pauses
              await new Promise((r) => setTimeout(r, 700));
              if (token.isCancellationRequested) return { items: [] };
              const full = model.getValue();
              const off = model.getOffsetAt(pos);
              const line = model.getLineContent(pos.lineNumber);
              if (line.slice(pos.column - 1).trim().length > 0) return { items: [] }; // only at end of line
              try {
                const r = await fetch('/api/ai/complete', { method: 'POST', headers: { 'Content-Type': 'application/json' },
                  body: JSON.stringify({ prefix: full.slice(0, off), suffix: full.slice(off), path: (this.current() || {}).path || '' }) });
                if (!r.ok || token.isCancellationRequested) return { items: [] };
                const { text } = await r.json();
                if (!text) return { items: [] };
                return { items: [{ insertText: text, range: new m.Range(pos.lineNumber, pos.column, pos.lineNumber, pos.column) }] };
              } catch (e) { return { items: [] }; }
            },
            freeInlineCompletions() {},
          });
        }
        this.lsp = new LspClient(m, this.root, () => this.files);
        this.lsp.connect();
        resolve();
        for (const [p, l] of this.pendingOpen) this.open(p, l);
        this.pendingOpen = [];
      });
    });
  }

  layout() { if (this.editor) this.editor.layout(); }

  current() { return this.files.get(this.activeView); }

  uri(path) { return this.monaco.Uri.file(this.root + '/' + path); }

  showView(id) {
    $$('#views .view').forEach((v) => v.classList.toggle('active', v.id === id));
  }

  showCircuit() {
    this.saveViewState();
    this.activeView = 'circuit';
    this.showView('view-circuit');
    this.onTabsChanged();
  }

  saveViewState() {
    const f = this.current();
    if (f && this.editor) f.viewState = this.editor.saveViewState();
  }

  async open(path, line) {
    if (!this.monaco) { this.pendingOpen.push([path, line]); return; }
    path = path.replace(/^\.\//, '');
    // absolute paths inside the project root
    if (path.startsWith(this.root + '/')) path = path.slice(this.root.length + 1);
    let f = this.files.get(path);
    if (!f) {
      let content;
      try { content = (await api('/api/file?path=' + encodeURIComponent(path))).content; } catch (e) { toast(`${path}: ${e.message}`, true); return; }
      const ext = path.split('.').pop().toLowerCase();
      const model = this.monaco.editor.createModel(content, LANG[ext] || 'plaintext', this.uri(path));
      f = { path, model, dirty: false, viewState: null, version: 1 };
      this.files.set(path, f);
      if (this.lsp) this.lsp.open(f);
      this.applyDiagnostics();
      this.renderBreakpoints(path);
    }
    this.saveViewState();
    this.activeView = path;
    this.showView('view-editor');
    this.editor.setModel(f.model);
    if (f.viewState) this.editor.restoreViewState(f.viewState);
    if (line) { this.editor.revealLineInCenter(line); this.editor.setPosition({ lineNumber: line, column: 1 }); }
    this.editor.focus();
    this.renderBreakpoints(path);
    this.onTabsChanged();
    this.highlightTree(path);
  }

  close(path) {
    const f = this.files.get(path);
    if (!f) return;
    if (f.dirty && !confirm(`${path} has unsaved changes. Close anyway?`)) return;
    if (this.lsp) this.lsp.close(f);
    f.model.dispose();
    this.files.delete(path);
    if (this.activeView === path) {
      const next = [...this.files.keys()].pop();
      if (next) this.open(next); else this.showCircuit();
    }
    this.onTabsChanged();
  }

  async save(path) {
    const f = path ? this.files.get(path) : this.current();
    if (!f) return;
    try {
      await api('/api/file', { method: 'PUT', json: { path: f.path, content: f.model.getValue() } });
      f.dirty = false;
      this.onTabsChanged();
      if (this.lsp) this.lsp.save(f);
      toast(`Saved ${f.path}`);
      if (f.path === 'esp32sim.yaml') document.dispatchEvent(new CustomEvent('project-file-saved'));
    } catch (e) { toast(`Save failed: ${e.message}`, true); }
  }

  /** Re-read an open file from disk if it has no unsaved edits (e.g. after the circuit editor rewrote it). */
  async reloadIfClean(path) {
    const f = this.files.get(path);
    if (!f) return;
    if (f.dirty) { toast(`${path} was changed by the circuit editor — your unsaved edits in the tab will overwrite it when saved`, true); return; }
    try {
      const r = await api('/api/file?path=' + encodeURIComponent(path));
      if (r.content !== undefined && r.content !== f.model.getValue()) {
        f.model.setValue(r.content);
        f.dirty = false;
        this.onTabsChanged();
      }
    } catch (e) { /* ignore */ }
  }

  async saveAll() {
    for (const f of this.files.values()) if (f.dirty) await this.save(f.path);
  }

  // ------------------------------------------------------------ diff view
  async showDiff(path, original, modified) {
    if (path !== undefined) {
      const m = this.monaco;
      const ext = path.split('.').pop();
      if (this.diffModels) { this.diffModels.forEach((x) => x.dispose()); }
      const a = m.editor.createModel(original ?? '', LANG[ext] || 'plaintext');
      const b = m.editor.createModel(modified ?? '', LANG[ext] || 'plaintext');
      this.diffModels = [a, b];
      this.diffEditor.setModel({ original: a, modified: b });
      this.diffOpen = path;
    }
    this.saveViewState();
    this.activeView = 'diff';
    this.showView('view-diff');
    this.onTabsChanged();
  }

  // ------------------------------------------------------------ diagnostics
  setDiagnostics(diags) {
    this.diags = diags || [];
    this.applyDiagnostics();
  }

  applyDiagnostics() {
    if (!this.monaco) return;
    const by = {};
    for (const d of this.diags || []) (by[d.file] = by[d.file] || []).push(d);
    for (const f of this.files.values()) {
      const list = by[f.path] || by[this.root + '/' + f.path] || [];
      this.monaco.editor.setModelMarkers(f.model, 'build', list.map((d) => ({
        startLineNumber: d.line, startColumn: d.col, endLineNumber: d.line, endColumn: d.col + 1000,
        message: d.message, severity: d.severity === 'error' ? this.monaco.MarkerSeverity.Error : d.severity === 'warning' ? this.monaco.MarkerSeverity.Warning : this.monaco.MarkerSeverity.Info,
      })));
    }
  }

  // ------------------------------------------------------------ breakpoints
  toggleBreakpoint(path, line) {
    const set = this.breakpoints.get(path) || new Set();
    if (set.has(line)) set.delete(line); else set.add(line);
    this.breakpoints.set(path, set);
    this.renderBreakpoints(path);
    this.syncBreakpoints();
  }

  async syncBreakpoints() {
    const list = [];
    for (const [file, lines] of this.breakpoints) for (const line of lines) list.push({ file, line });
    try {
      const r = await api('/api/debug/breakpoints', { json: { breakpoints: list } });
      this.verified = new Set(r.breakpoints.filter((b) => b.verified).map((b) => b.file + ':' + b.line));
      for (const p of this.breakpoints.keys()) this.renderBreakpoints(p);
      const bad = r.breakpoints.filter((b) => !b.verified);
      if (bad.length) toast(`Breakpoint not resolved (${bad[0].file}:${bad[0].line}) — build with debug info (-g) and load the ELF`, true);
      document.dispatchEvent(new CustomEvent('breakpoints-changed'));
    } catch (e) { toast(e.message, true); }
  }

  renderBreakpoints(path) {
    const f = this.files.get(path);
    if (!f || !this.monaco) return;
    const lines = [...(this.breakpoints.get(path) || [])];
    const old = this.bpDecorations.get(path) || [];
    const decos = lines.map((l) => ({ range: new this.monaco.Range(l, 1, l, 1), options: { isWholeLine: false, glyphMarginClassName: this.verified && !this.verified.has(path + ':' + l) ? 'bp-glyph-unverified' : 'bp-glyph', glyphMarginHoverMessage: { value: 'Breakpoint' } } }));
    this.bpDecorations.set(path, f.model.deltaDecorations(old, decos));
  }

  allBreakpoints() {
    const out = [];
    for (const [file, lines] of this.breakpoints) for (const line of lines) out.push({ file, line });
    return out;
  }

  async showExecution(file, line) {
    // file from DWARF is absolute; map into the project
    let path = file;
    if (this.root && path.startsWith(this.root + '/')) path = path.slice(this.root.length + 1);
    else if (path.startsWith('/')) { toast(`Stopped in ${file}:${line} (outside the project)`); return; }
    await this.open(path, line);
    const f = this.files.get(path);
    if (!f) return;
    this.clearExecution();
    this.execModel = f.model;
    this.execDecoration = f.model.deltaDecorations([], [{ range: new this.monaco.Range(line, 1, line, 1), options: { isWholeLine: true, className: 'current-line', glyphMarginClassName: 'exec-glyph' } }]);
  }

  clearExecution() {
    if (this.execModel && !this.execModel.isDisposed()) this.execModel.deltaDecorations(this.execDecoration, []);
    this.execDecoration = [];
  }

  getSelection() {
    if (!this.editor || this.activeView === 'circuit') return '';
    const sel = this.editor.getSelection();
    return sel ? this.editor.getModel().getValueInRange(sel) : '';
  }

  async replaceFile(path, content) {
    const f = this.files.get(path);
    if (f) {
      f.model.pushEditOperations([], [{ range: f.model.getFullModelRange(), text: content }], () => null);
      await this.save(path);
    } else {
      await api('/api/file', { method: 'PUT', json: { path, content } });
      await this.refreshTree();
      await this.open(path);
      if (path === 'esp32sim.yaml') document.dispatchEvent(new CustomEvent('project-file-saved'));
    }
  }

  // ------------------------------------------------------------ file tree
  async refreshTree() {
    let data;
    try { data = await api('/api/files'); } catch (e) { return; }
    const tree = $('#file-tree');
    tree.innerHTML = '';
    const walk = (nodes, depth, parent) => {
      for (const n of nodes) {
        const row = el('div', { class: 'node', style: { paddingLeft: 8 + depth * 12 + 'px' }, 'data-path': n.path, title: n.path },
          el('span', { class: 'icon' }, n.dir ? '▾' : fileIcon(n.name)), el('span', {}, n.name));
        parent.append(row);
        if (n.dir) {
          const kids = el('div');
          parent.append(kids);
          let open = depth < 1 || ['src', 'include', 'lib'].includes(n.name);
          kids.style.display = open ? '' : 'none';
          row.firstChild.textContent = open ? '▾' : '▸';
          row.onclick = () => { open = !open; kids.style.display = open ? '' : 'none'; row.firstChild.textContent = open ? '▾' : '▸'; this.selectedTreePath = n.path; };
          walk(n.children || [], depth + 1, kids);
        } else {
          row.onclick = () => { this.selectedTreePath = n.path; this.open(n.path); };
        }
        row.oncontextmenu = (e) => { e.preventDefault(); this.treeMenu(n); };
      }
    };
    walk(data.tree, 0, tree);
    if (!data.tree.length) tree.append(el('p', { class: 'muted', style: { padding: '8px' } }, 'Empty project folder.'));
    $('#new-file').onclick = () => this.newFile(false);
    $('#new-folder').onclick = () => this.newFile(true);
    $('#refresh-files').onclick = () => this.refreshTree();
  }

  highlightTree(path) {
    $$('#file-tree .node').forEach((n) => n.classList.toggle('sel', n.dataset.path === path));
  }

  async newFile(dir) {
    const base = this.selectedTreePath && !this.selectedTreePath.includes('.') ? this.selectedTreePath + '/' : 'src/';
    const name = prompt(dir ? 'New folder (relative to the project):' : 'New file (relative to the project):', base);
    if (!name) return;
    await api('/api/file', { method: 'PUT', json: { path: name, content: '', dir } }).catch((e) => toast(e.message, true));
    await this.refreshTree();
    if (!dir) this.open(name);
  }

  async treeMenu(n) {
    const act = prompt(`${n.path}\nType "rename" or "delete":`, 'rename');
    if (act === 'delete') {
      if (!confirm(`Delete ${n.path}?`)) return;
      await api('/api/file?path=' + encodeURIComponent(n.path), { method: 'DELETE' }).catch((e) => toast(e.message, true));
      if (this.files.has(n.path)) { this.files.get(n.path).dirty = false; this.close(n.path); }
    } else if (act === 'rename') {
      const to = prompt('New path:', n.path);
      if (!to || to === n.path) return;
      await api('/api/file/rename', { json: { from: n.path, to } }).catch((e) => toast(e.message, true));
      if (this.files.has(n.path)) { this.files.get(n.path).dirty = false; this.close(n.path); this.open(to); }
    }
    this.refreshTree();
  }
}

function fileIcon(name) {
  const ext = name.split('.').pop().toLowerCase();
  return { cpp: '⚙', c: '⚙', ino: '∞', h: 'ℎ', hpp: 'ℎ', ini: '⚑', yaml: '⚡', yml: '⚡', md: '¶', json: '{}', py: '🐍', bin: '▦', elf: '▦' }[ext] || '·';
}
