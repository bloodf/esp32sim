// Source control panel (git).
import { api, el, toast, $ } from './api.js';

export class GitPanel {
  constructor(root, editor) {
    this.root = root;
    this.editor = editor;
    $('#git-refresh').onclick = () => this.refresh();
  }

  async act(action, extra = {}) {
    try {
      const r = await api('/api/git/action', { json: { action, ...extra } });
      if (r.output && r.output.trim()) toast(r.output.trim().split('\n').slice(-1)[0]);
    } catch (e) { toast(e.message, true); }
    this.refresh();
  }

  async refresh() {
    let s;
    try { s = await api('/api/git/status'); } catch (e) { this.root.textContent = e.message; return; }
    const r = this.root;
    r.innerHTML = '';
    if (!s.repo) {
      r.append(el('p', { class: 'muted' }, 'This project is not a git repository.'), el('button', { class: 'primary', onclick: () => this.act('init') }, 'Initialize repository'));
      return;
    }
    const branchSel = el('select', { onchange: (e) => { if (e.target.value === '__new') { const n = prompt('New branch name:'); if (n) this.act('branch', { name: n }); else this.refresh(); } else this.act('checkout', { name: e.target.value }); } },
      ...s.branches.map((b) => el('option', { value: b, selected: b === s.branch }, b)), el('option', { value: '__new' }, '+ new branch…'));
    if (!s.branches.length) branchSel.append(el('option', { selected: true }, s.branch));
    r.append(el('div', { class: 'btn-row' }, el('span', {}, '⎇ '), branchSel,
      s.remotes.length ? el('button', { title: 'Pull', onclick: () => this.act('pull') }, `⇣${s.behind || ''}`) : null,
      s.remotes.length ? el('button', { title: 'Push', onclick: () => this.act('push') }, `⇡${s.ahead || ''}`) : el('button', { title: 'Add remote', onclick: () => { const u = prompt('Remote URL (origin):'); if (u) this.act('remote_add', { name: u }); } }, '+ remote'),
      s.remotes.length ? el('button', { title: 'Fetch', onclick: () => this.act('fetch') }, '⟳') : null));
    const msg = el('textarea', { id: 'git-msg', placeholder: 'Commit message (Ctrl+Enter to commit)' });
    msg.addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) commit(); });
    const commit = async () => {
      if (!s.files.some((f) => f.staged)) await api('/api/git/action', { json: { action: 'stage_all' } });
      this.act('commit', { message: msg.value });
    };
    r.append(msg, el('div', { class: 'btn-row' }, el('button', { class: 'primary', onclick: commit }, '✔ Commit'), el('button', { onclick: () => this.act('stage_all') }, 'Stage all')));
    const staged = s.files.filter((f) => f.staged);
    const changes = s.files.filter((f) => !f.staged || f.worktree !== ' ');
    const section = (title, files, isStaged) => {
      if (!files.length) return;
      r.append(el('div', { class: 'section-title' }, `${title} (${files.length})`));
      for (const f of files) {
        const st = isStaged ? f.index : f.untracked ? '?' : f.worktree;
        r.append(el('div', { class: 'git-file', title: f.path, onclick: () => this.diff(f.path, isStaged) },
          el('span', { class: 'st st-' + st }, st === '?' ? 'U' : st),
          el('span', { class: 'path' }, f.path),
          isStaged ? el('button', { title: 'Unstage', onclick: (e) => { e.stopPropagation(); this.act('unstage', { paths: [f.path] }); } }, '−')
            : el('span', {}, el('button', { title: 'Discard changes', onclick: (e) => { e.stopPropagation(); if (confirm(`Discard changes to ${f.path}?`)) this.act('discard', { paths: [f.path] }); } }, '↶'),
              el('button', { title: 'Stage', onclick: (e) => { e.stopPropagation(); this.act('stage', { paths: [f.path] }); } }, '+'))));
      }
    };
    section('Staged changes', staged, true);
    section('Changes', changes, false);
    if (!s.files.length) r.append(el('p', { class: 'muted' }, 'No changes.'));
    const log = await api('/api/git/log').catch(() => []);
    if (log.length) {
      r.append(el('div', { class: 'section-title' }, 'History'));
      for (const c of log) r.append(el('div', { class: 'commit', title: c.hash }, el('span', { class: 'h' }, c.short), ' ', c.subject, el('div', { class: 'muted' }, `${c.author}, ${c.when}`)));
    }
  }

  async diff(path, staged) {
    const head = await api('/api/git/show?path=' + encodeURIComponent(path)).catch(() => ({ content: '' }));
    let cur = '';
    try { cur = (await api('/api/file?path=' + encodeURIComponent(path))).content; } catch (e) { cur = ''; }
    this.editor.showDiff(path + (staged ? ' (staged)' : ''), head.content || '', cur);
  }
}
