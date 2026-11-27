// Settings and project dialogs.
import { bus, api, el, modal, toast } from './api.js';

const PROVIDERS = [
  ['openai', 'OpenAI', 'gpt-4o-mini'],
  ['anthropic', 'Anthropic (Claude)', 'claude-sonnet-4-5'],
  ['gemini', 'Google Gemini', 'gemini-2.5-flash'],
  ['ollama', 'Ollama (local, no key)', 'qwen2.5-coder'],
  ['openai-compatible', 'OpenAI-compatible (OpenRouter, Groq, LM Studio…)', ''],
];

export async function openSettings(state, focus) {
  const s = await api('/api/settings');
  const ai = s.ai || {};
  const body = el('div');
  const prov = el('select', {}, ...PROVIDERS.map(([v, n]) => el('option', { value: v, selected: v === (ai.provider || 'openai') }, n)));
  const model = el('input', { value: ai.model || '', placeholder: 'model name (blank = default)' });
  const models = el('datalist', { id: 'model-list' });
  model.setAttribute('list', 'model-list');
  const base = el('input', { value: ai.base_url || '', placeholder: 'custom endpoint (optional), e.g. https://openrouter.ai/api/v1' });
  const key = el('input', { type: 'password', placeholder: 'API key for the selected provider (stored locally, never shown again)' });
  const keyState = el('span', { class: 'muted' });
  const updKeyState = () => { keyState.textContent = (ai.keys_set || {})[prov.value] ? '✔ a key is saved for this provider' : 'no key saved'; const d = PROVIDERS.find((p) => p[0] === prov.value); model.placeholder = `model (default: ${d[2] || 'required'})`; };
  prov.onchange = updKeyState;
  updKeyState();
  const temp = el('input', { type: 'number', step: '0.1', min: '0', max: '2', value: ai.temperature ?? 0.2 });
  const inline = el('input', { type: 'checkbox', checked: !!ai.inline });
  const listBtn = el('button', { onclick: async () => {
    await save(false);
    try { const r = await api('/api/ai/models'); models.innerHTML = ''; r.models.forEach((m) => models.append(el('option', { value: m }))); toast(`${r.models.length} models available — pick one in the model field`); } catch (e) { toast('Could not list models: ' + e.message, true); }
  } }, 'List models');
  body.append(
    el('div', { class: 'section-title' }, 'AI assistant'),
    el('div', { class: 'field' }, el('label', {}, 'Provider'), prov),
    el('div', { class: 'field' }, el('label', {}, 'Model'), el('div', { class: 'row' }, model, listBtn), models),
    el('div', { class: 'field' }, el('label', {}, 'API key'), key, keyState),
    el('div', { class: 'field' }, el('label', {}, 'Endpoint'), base),
    el('div', { class: 'field' }, el('label', {}, 'Temperature'), temp),
    el('div', { class: 'field' }, el('label', {}, el('span', {}, inline, ' AI inline completions in the editor (ghost text, Tab to accept)'))),
  );
  const pio = el('input', { value: s.pio || '', placeholder: s.tools.pio || 'not found — pip install platformio' });
  const acli = el('input', { value: s.arduino_cli || '', placeholder: s.tools.arduino_cli || 'not found (optional)' });
  const clangd = el('input', { value: s.clangd || '', placeholder: s.tools.clangd || 'not found — pip install clangd' });
  body.append(
    el('div', { class: 'section-title' }, 'Tools'),
    el('div', { class: 'field' }, el('label', {}, 'PlatformIO (pio)'), pio),
    el('div', { class: 'field' }, el('label', {}, 'arduino-cli'), acli),
    el('div', { class: 'field' }, el('label', {}, 'clangd (IntelliSense)'), clangd),
    el('p', { class: 'muted' }, `Detected: pio ${s.tools.pio ? '✔' : '✘'} · arduino-cli ${s.tools.arduino_cli ? '✔' : '✘'} · clangd ${s.tools.clangd ? '✔' : '✘'} · git ${s.tools.git ? '✔' : '✘'}`),
  );
  const save = async (close = true) => {
    const payload = { ai: { provider: prov.value, model: model.value, base_url: base.value, temperature: parseFloat(temp.value), inline: inline.checked }, pio: pio.value, arduino_cli: acli.value, clangd: clangd.value };
    if (key.value) payload.ai.keys = { [prov.value]: key.value };
    try {
      await api('/api/settings', { method: 'PUT', json: payload });
      if (state && state.info) state.info = await api('/api/info');
      bus.emit('settings-changed');
      toast('Settings saved');
      if (close) closeFn();
    } catch (e) { toast(e.message, true); }
  };
  body.append(el('div', { class: 'btn-row', style: { marginTop: '12px' } }, el('button', { class: 'primary', onclick: () => save(true) }, 'Save'),
    el('button', { onclick: async () => { await api('/api/settings', { method: 'PUT', json: { ai: { keys: { [prov.value]: '' } } } }); toast('Key removed'); closeFn(); } }, 'Remove key')));
  const closeFn = modal('Settings', body);
  if (focus === 'ai') setTimeout(() => key.focus(), 50);
}

export async function newProjectDialog(done, welcome = false) {
  const templates = await api('/api/templates');
  let chosen = 'blink';
  const body = el('div');
  if (welcome) body.append(el('p', {}, 'Welcome to esp32sim! Create a project to get started — you get a PlatformIO sketch, a wired circuit and a test scenario. You can also open an existing PlatformIO project or a firmware .bin from the Project menu.'));
  const grid = el('div', { class: 'tpl-grid' });
  for (const t of templates) {
    const c = el('div', { class: 'tpl' + (t.name === chosen ? ' sel' : ''), onclick: () => { chosen = t.name; [...grid.children].forEach((x) => x.classList.remove('sel')); c.classList.add('sel'); if (!dirEdited) dir.value = `~/esp32-projects/${t.name}`; } }, el('b', {}, t.name), el('span', { class: 'muted' }, t.description));
    grid.append(c);
  }
  let dirEdited = false;
  const dir = el('input', { value: '~/esp32-projects/blink' });
  dir.addEventListener('input', () => (dirEdited = true));
  const dirRow = el('div', { class: 'row' }, dir, browseButton('Folder for the new project', (p) => { dir.value = p; dirEdited = true; }));
  body.append(el('div', { class: 'field' }, el('label', {}, 'Template'), grid), el('div', { class: 'field' }, el('label', {}, 'Folder'), dirRow));
  body.append(el('div', { class: 'btn-row' }, el('button', { class: 'primary', onclick: async () => {
    try { await api('/api/project/new', { json: { path: dir.value, template: chosen } }); close(); await done(); toast('Project created — press ⚙ Build & Run to compile it with PlatformIO'); } catch (e) { toast(e.message, true); }
  } }, 'Create project')));
  const close = modal(welcome ? 'Welcome to esp32sim' : 'New project', body);
}

export async function openProjectDialog(state, done) {
  const body = el('div');
  const inp = el('input', { placeholder: 'path to a project folder, esp32sim.yaml, firmware .bin or .elf', value: (state.info && state.info.settings.recent[0]) || '' });
  body.append(el('div', { class: 'field' }, el('label', {}, 'Path'), el('div', { class: 'row' }, inp, browseButton('Open project folder', (p) => (inp.value = p)))));
  const rec = (state.info && state.info.settings.recent) || [];
  if (rec.length) {
    body.append(el('div', { class: 'section-title' }, 'Recent'));
    for (const r of rec) body.append(el('div', { class: 'frame', onclick: () => (inp.value = r) }, r));
  }
  body.append(el('div', { class: 'btn-row', style: { marginTop: '10px' } }, el('button', { class: 'primary', onclick: async () => {
    try { await api('/api/project/open', { json: { path: inp.value } }); close(); await done(); } catch (e) { toast(e.message, true); }
  } }, 'Open')));
  const close = modal('Open project', body);
  setTimeout(() => inp.focus(), 50);
}

/** "Browse…" button using the desktop app's native folder dialog (hidden in the browser). */
function browseButton(title, onPick) {
  const info = window.esp32sim && window.esp32sim.info;
  if (!info || !info.desktop) return null;
  return el('button', { title, onclick: async () => {
    try { const r = await api('/api/native/pick-folder', { json: { title } }); if (r.path) onPick(r.path); } catch (e) { toast(e.message, true); }
  } }, 'Browse…');
}
