// IDE features: build errors, settings, AI assistant (mock provider), git,
// project creation and source-level debugging.
import { test, expect } from '@playwright/test';
import { readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { resetProject, openApp, aiRequests, AI_BASE, serialText } from './helpers.mjs';
import { PROJECTS } from './paths.mjs';

test.beforeEach(async ({ request, page }) => {
  await resetProject(request);
  await openApp(page);
});

test('Build & Run explains what is missing', async ({ page }) => {
  await page.locator('#btn-build').click();
  await expect(page.locator('#bview-build')).toBeVisible();
  await expect(page.locator('#build-log')).toContainText('no platformio.ini');
  await expect(page.locator('#toast')).toContainText(/no platformio.ini|Build failed/);
  // a new page does not re-announce the old build result
  await page.reload();
  await expect(page.locator('#bview-serial')).toBeVisible();
  await expect(page.locator('#toast')).not.toContainText('Build failed');
});

test('settings dialog configures an AI provider and keeps the key secret', async ({ page, request }) => {
  await page.locator('#activity button[data-act="settings"]').click();
  const modal = page.locator('#modal');
  await expect(modal).toBeVisible();
  await modal.locator('.field', { hasText: 'Provider' }).locator('select').selectOption('openai-compatible');
  await modal.locator('.field', { hasText: 'Endpoint' }).locator('input').fill(AI_BASE);
  await modal.locator('.field', { hasText: 'API key' }).locator('input').fill('sk-e2e-secret');
  await modal.getByRole('button', { name: 'List models' }).click();
  await expect(page.locator('#toast')).toContainText('2 models available');
  await modal.getByRole('button', { name: 'Save' }).click();
  await expect(page.locator('#toast')).toContainText('Settings saved');
  const s = await (await request.get('/api/settings')).json();
  expect(s.ai.provider).toBe('openai-compatible');
  expect(s.ai.keys_set['openai-compatible']).toBe(true);
  expect(JSON.stringify(s)).not.toContain('sk-e2e-secret');
  await expect(page.locator('#ai-body .ai-provider')).toContainText('openai-compatible');
});

async function useMockAi(request) {
  const r = await request.put('/api/settings', { data: { ai: { provider: 'openai-compatible', base_url: AI_BASE, model: 'mock-large', keys: { 'openai-compatible': 'k' } } } });
  expect(r.ok()).toBeTruthy();
}

test('AI assistant answers with project context', async ({ page, request }) => {
  await useMockAi(request);
  await page.reload();
  await page.locator('#activity button[data-panel="ai"]').click();
  await page.locator('#ai-input').fill('why does the LED blink?');
  await page.locator('#ai-input').press('Enter');
  await expect(page.locator('#ai-log .msg.assistant').last()).toContainText('Hello from the mock AI!');
  const reqs = await aiRequests(request);
  const last = reqs.filter((r) => r.url.endsWith('/chat/completions')).at(-1);
  expect(last.body.model).toBe('mock-large');
  expect(last.auth).toBe('Bearer k');
  expect(last.body.messages[0].content).toContain('[esp32.D5, r1.1]');
  expect(last.body.messages[0].content).toContain('mockfw ready');
});

test('AI file proposals can be applied with one click', async ({ page, request }) => {
  await useMockAi(request);
  await page.reload();
  page.on('dialog', (d) => d.accept());
  await page.locator('#activity button[data-panel="ai"]').click();
  await page.locator('#ai-input').fill('write code for me');
  await page.locator('#ai-input').press('Enter');
  const apply = page.getByRole('button', { name: 'Apply to src/extra.cpp' });
  await expect(apply).toBeVisible();
  await apply.click();
  await expect(page.locator('#toast')).toContainText('Applied to src/extra.cpp');
  await expect.poll(() => existsSync(join(PROJECTS.mock, 'src', 'extra.cpp'))).toBe(true);
  expect(readFileSync(join(PROJECTS.mock, 'src', 'extra.cpp'), 'utf8')).toBe('int extra() { return 42; }\n');
});

test('AI quick actions', async ({ page, request }) => {
  await useMockAi(request);
  await page.reload();
  await page.locator('#activity button[data-panel="ai"]').click();
  await page.locator('#ai-quick button', { hasText: 'Check wiring' }).click();
  await expect(page.locator('#ai-log .msg.user').last()).toContainText('Check the circuit wiring');
  await expect(page.locator('#ai-log .msg.assistant').last()).toContainText('Done.');
  const last = (await aiRequests(request)).filter((r) => r.url.endsWith('/chat/completions')).at(-1);
  expect(last.body.messages.at(-1).content).toContain('Check the circuit wiring');
});

test('AI without configuration opens the settings', async ({ page }) => {
  await page.locator('#activity button[data-panel="ai"]').click();
  await page.locator('#ai-input').fill('hi');
  await page.locator('#ai-input').press('Enter');
  await expect(page.locator('#ai-log .msg.assistant').last()).toContainText('no API key configured');
  await expect(page.locator('#modal')).toBeVisible();
  await expect(page.locator('#modal-title')).toHaveText('Settings');
});

test('source control: initialise, commit and see the history', async ({ page }) => {
  await page.locator('#activity button[data-panel="git"]').click();
  await page.getByRole('button', { name: 'Initialize repository' }).click();
  await expect(page.locator('#git-body')).toContainText('esp32sim.yaml');
  await page.locator('#git-msg').fill('first circuit');
  await page.getByRole('button', { name: '✔ Commit' }).click();
  await expect(page.locator('#git-body .commit')).toContainText('first circuit');
  await expect(page.locator('#git-body')).toContainText('No changes.');
});

test('new project from a template', async ({ page }) => {
  await page.locator('#btn-file').click();
  await page.locator('#menu-file button[data-act="new"]').click();
  const modal = page.locator('#modal');
  await modal.locator('.tpl', { hasText: 'button' }).first().click();
  const dir = join(PROJECTS.created, `btn-${Date.now()}`);
  await modal.locator('.field', { hasText: 'Folder' }).locator('input').fill(dir);
  await modal.getByRole('button', { name: 'Create project' }).click();
  await expect(page.locator('#toast')).toContainText('Project created');
  await expect(page.locator('#project-name')).toHaveText(/btn-/);
  await expect(page.locator('#file-tree .node[data-path="platformio.ini"]')).toBeVisible();
  expect(existsSync(join(dir, 'src', 'main.cpp'))).toBe(true);
});

test('open project dialog and source-level debugging', async ({ page }) => {
  await page.locator('#btn-file').click();
  await page.locator('#menu-file button[data-act="open"]').click();
  await page.locator('#modal .field input').fill(PROJECTS.debug);
  await page.locator('#modal').getByRole('button', { name: 'Open' }).click();
  await expect(page.locator('#project-name')).toHaveText('debug-demo');
  await expect.poll(() => serialText(page)).toContain('...');
  // set a breakpoint on line 11 of blink.c by clicking the editor gutter
  await page.locator('#file-tree .node[data-path="blink.c"]').click();
  await expect(page.locator('#editor .view-lines')).toContainText('app_main');
  const ln = page.locator('#editor .line-numbers', { hasText: /^11$/ });
  const lb = await ln.boundingBox();
  const gm = await page.locator('#editor .glyph-margin').first().boundingBox();
  await page.mouse.click(gm.x + gm.width / 2, lb.y + lb.height / 2);
  await expect(page.locator('#status-text')).toHaveText(/^breakpoint/);
  await expect.poll(() => page.evaluate(() => window.esp32sim.status.location?.line)).toBe(11);
  await page.locator('#activity button[data-panel="debug"]').click();
  await page.locator('#debug-body select.dbg-core').selectOption('0');
  await expect(page.locator('#debug-body')).toContainText('app_main');
  // watch the global counter, step, continue
  const watch = page.locator('#debug-body input[placeholder^="variable name"]');
  await watch.fill('counter');
  await watch.press('Enter');
  await expect(page.locator('#debug-body .kv:not(.locals)').first()).toContainText('counter');
  await page.locator('#debug-body button', { hasText: 'over' }).click();
  await expect.poll(() => page.evaluate(() => window.esp32sim.status.location?.line ?? null)).not.toBe(11);
  await page.locator('#debug-body button[title^="Continue"]').click();
  await expect(page.locator('#status-text')).toHaveText(/^breakpoint/);
  // removing the breakpoint lets it run
  await page.mouse.click(gm.x + gm.width / 2, lb.y + lb.height / 2);
  await page.locator('#debug-body button[title^="Continue"]').click();
  await expect(page.locator('#status-text')).toHaveText('running');
});
