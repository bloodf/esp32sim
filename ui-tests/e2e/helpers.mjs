// Helpers shared by the GUI specs.
import { expect } from '@playwright/test';
import { readFileSync, writeFileSync, copyFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { PROJECTS, FIXTURES, AI_PORT } from './paths.mjs';

export const MOCK_YAML = readFileSync(join(FIXTURES, 'mockfw', 'esp32sim.yaml'), 'utf8');

/** Put the mock project back into its initial state (and open it). */
export async function resetProject(request) {
  writeFileSync(join(PROJECTS.mock, 'esp32sim.yaml'), MOCK_YAML);
  copyFileSync(join(FIXTURES, 'mockfw', 'mockfw.bin'), join(PROJECTS.mock, 'mockfw.bin'));
  for (const extra of ['src', '.git', '.gitignore', 'notes.txt', 'platformio.ini']) rmSync(join(PROJECTS.mock, extra), { recursive: true, force: true });
  let r = await request.post('/api/project/open', { data: { path: PROJECTS.mock } });
  expect(r.ok()).toBeTruthy();
  r = await request.put('/api/settings', { data: { ai: { provider: '', model: '', base_url: '', keys: { openai: '', 'openai-compatible': '' }, inline: false } } });
  expect(r.ok()).toBeTruthy();
}

/** Open the GUI and wait until the simulation is running the mock firmware. */
export async function openApp(page, { errors } = {}) {
  if (errors) {
    page.on('pageerror', (e) => errors.push(String(e)));
    page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
  }
  await page.goto('/');
  await expect(page.locator('#status-text')).toHaveText('running');
  await expect.poll(() => serialText(page)).toContain('mockfw ready');
}

export const serialText = (page) => page.evaluate(() => window.esp32sim.serialView?.text || '');
export const component = (page, id) => page.evaluate((id) => (window.esp32sim.board?.components || []).find((c) => c.id === id), id);
export const brightness = async (page, id) => (await component(page, id))?.state?.brightness ?? 0;

/** Type a line into the serial monitor input and press Enter. */
export async function sendSerial(page, text) {
  await page.locator('#serial-input').fill(text);
  await page.locator('#serial-input').press('Enter');
}

export async function aiRequests(request) {
  const r = await request.get(`http://127.0.0.1:${AI_PORT}/__requests`);
  return r.json();
}

export const AI_BASE = `http://127.0.0.1:${AI_PORT}/v1`;
