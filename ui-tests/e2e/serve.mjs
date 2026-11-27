// webServer for the Playwright run: prepares a throw-away workspace, starts a
// mock AI provider (OpenAI-compatible streaming API) and the esp32sim GUI.
import { spawn } from 'node:child_process';
import http from 'node:http';
import { cpSync, mkdirSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { TMP, PORT, AI_PORT, BIN, PROJECTS, FIXTURES } from './paths.mjs';

if (!existsSync(BIN)) {
  console.error(`esp32sim binary not found at ${BIN} — run \`cargo build --release\` first (or set ESP32SIM_BIN)`);
  process.exit(1);
}

rmSync(TMP, { recursive: true, force: true });
mkdirSync(TMP, { recursive: true });
const home = join(TMP, 'home');
mkdirSync(join(home, '.config'), { recursive: true });
mkdirSync(PROJECTS.created, { recursive: true });

// 1. mock firmware project (button, LEDs, heartbeat, serial commands)
mkdirSync(PROJECTS.mock, { recursive: true });
for (const f of ['esp32sim.yaml', 'mockfw.bin']) cpSync(join(FIXTURES, 'mockfw', f), join(PROJECTS.mock, f));
// 2. debugger project: hand-assembled ELF with DWARF for blink.c
mkdirSync(PROJECTS.debug, { recursive: true });
for (const f of ['blink.elf', 'blink.c']) cpSync(join(FIXTURES, 'debug-demo', f), join(PROJECTS.debug, f));
writeFileSync(join(PROJECTS.debug, 'esp32sim.yaml'), 'name: debug-demo\nfirmware: blink.elf\ncomponents: []\nconnections: []\n');

// mock AI provider: streams "Hello from the mock AI!"; questions mentioning
// "code" get a file proposal for src/extra.cpp
const seen = [];
const ai = http.createServer((req, res) => {
  let body = '';
  req.on('data', (c) => (body += c));
  req.on('end', () => {
    if (req.url === '/__requests') { res.setHeader('content-type', 'application/json'); return res.end(JSON.stringify(seen)); }
    const json = body ? JSON.parse(body) : {};
    seen.push({ url: req.url, body: json, auth: req.headers.authorization || '' });
    if (req.url.endsWith('/models')) { res.setHeader('content-type', 'application/json'); return res.end(JSON.stringify({ data: [{ id: 'mock-large' }, { id: 'mock-small' }] })); }
    if (!req.url.endsWith('/chat/completions')) { res.statusCode = 404; return res.end('not found'); }
    const last = (json.messages || []).filter((m) => m.role === 'user').at(-1)?.content || '';
    let text = 'Hello from the mock AI!';
    if (/code/i.test(last)) text = 'Here is a file:\n```cpp file=src/extra.cpp\nint extra() { return 42; }\n```\nDone.';
    if (/Serial\.<CURSOR>/.test(last)) text = 'println("hi");';
    res.setHeader('content-type', 'text/event-stream');
    for (const w of text.match(/.{1,7}/gs)) res.write(`data: ${JSON.stringify({ choices: [{ delta: { content: w } }] })}\n\n`);
    res.end('data: [DONE]\n\n');
  });
});
ai.listen(AI_PORT, '127.0.0.1');

const child = spawn(BIN, ['gui', PROJECTS.mock, '--port', String(PORT), '--no-browser'], {
  env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, '.config'), XDG_CACHE_HOME: join(home, '.cache') },
  stdio: 'inherit',
});
const stop = () => { child.kill('SIGTERM'); ai.close(); };
for (const s of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(s, () => { stop(); process.exit(0); });
process.on('exit', stop);
child.on('exit', (code) => { ai.close(); process.exit(code ?? 0); });
