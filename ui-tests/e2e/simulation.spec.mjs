// Running firmware: serial monitor, live circuit state, run controls and the
// analysis panels, all against the mock firmware.
import { test, expect } from '@playwright/test';
import { resetProject, openApp, serialText, brightness, sendSerial } from './helpers.mjs';

test.beforeEach(async ({ request, page }) => {
  await resetProject(request);
  await openApp(page);
});

test('serial monitor round trip', async ({ page }) => {
  await sendSerial(page, 'ping');
  await expect.poll(() => serialText(page)).toContain('pong');
  await sendSerial(page, 'hello from the test');
  await expect.poll(() => serialText(page)).toContain('echo:hello from the test');
  // the clear button empties the terminal
  await page.locator('#serial-clear').click();
  expect(await serialText(page)).toBe('');
});

test('typing directly in the terminal sends keys', async ({ page }) => {
  await page.locator('#terminal').click();
  await page.keyboard.type('ping\r');
  await expect.poll(() => serialText(page)).toContain('pong');
});

test('firmware output drives the LED in the circuit', async ({ page }) => {
  expect(await brightness(page, 'led')).toBeLessThan(0.01);
  await sendSerial(page, 'led on');
  await expect.poll(() => brightness(page, 'led')).toBeGreaterThan(0.2);
  await sendSerial(page, 'led off');
  await expect.poll(() => brightness(page, 'led')).toBeLessThan(0.01);
  // the heartbeat LED blinks on its own
  await expect.poll(() => brightness(page, 'beat'), { timeout: 5000 }).toBeGreaterThan(0.2);
});

test('pressing the push button in the circuit', async ({ page }) => {
  const btn = page.locator('g.part', { has: page.locator('[data-ep="btn.1"]') }).locator('.interactive').first();
  const box = await btn.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await expect.poll(() => brightness(page, 'board_led')).toBeGreaterThan(0.05);
  await page.mouse.up();
  await expect.poll(() => brightness(page, 'board_led')).toBeLessThan(0.01);
  await sendSerial(page, 'count');
  await expect.poll(() => serialText(page)).toContain('count=1');
});

test('pause, resume, reset and speed', async ({ page }) => {
  await page.locator('#btn-pause').click();
  await expect(page.locator('#status-text')).toHaveText('paused');
  const t = await page.evaluate(() => window.esp32sim.status.time_ns);
  await page.waitForTimeout(400);
  expect(await page.evaluate(() => window.esp32sim.status.time_ns)).toBe(t);
  await page.locator('#btn-run').click();
  await expect(page.locator('#status-text')).toHaveText('running');
  const boots = (await serialText(page)).split('mockfw ready').length;
  await page.locator('#btn-reset').click();
  await expect.poll(async () => (await serialText(page)).split('mockfw ready').length).toBeGreaterThan(boots);
  await page.locator('#speed').selectOption('4');
  await expect.poll(() => page.evaluate(() => window.esp32sim.status.speed)).toBe(4);
  // keyboard shortcuts
  await page.locator('#circuit').click({ position: { x: 5, y: 5 } });
  await page.keyboard.press('F6');
  await expect(page.locator('#status-text')).toHaveText('paused');
  await page.keyboard.press('F5');
  await expect(page.locator('#status-text')).toHaveText('running');
});

test('plotter graphs labelled values from the serial output', async ({ page }) => {
  for (let i = 0; i < 3; i++) await sendSerial(page, 'count');
  await page.locator('#bottom-tabs button[data-bottom="plot"]').click();
  await expect(page.locator('#bview-plot .plot-legend')).toContainText('count');
});

test('logic analyzer records the heartbeat on GPIO18', async ({ page }) => {
  await page.locator('#bottom-tabs button[data-bottom="logic"]').click();
  await expect(page.locator('#bview-logic .la-pads')).toContainText('GPIO18', { timeout: 5000 });
});

test('bus monitor lists UART traffic', async ({ page, request }) => {
  await page.locator('#bottom-tabs button[data-bottom="bus"]').click();
  await request.post('/api/control', { data: { cmd: 'serial', data: 'ping\n' } });
  await expect(page.locator('#bview-bus .bm-table')).toContainText('UART');
});

test('emulator log shows boot messages', async ({ page }) => {
  await sendSerial(page, 'reboot');
  await page.locator('#bottom-tabs button[data-bottom="log"]').click();
  await expect(page.locator('#emu-log')).toContainText('reset');
});

test('status bar reports firmware halts', async ({ page, request }) => {
  // replace the firmware with garbage: the project reload reports the error
  const r = await request.put('/api/file', { data: { path: 'mockfw.bin', content: 'not firmware' } });
  expect(r.ok()).toBeTruthy();
  await request.post('/api/project/reload', { data: {} });
  await expect(page.locator('#status-text')).toHaveText(/no firmware|halted|error/, { timeout: 10000 });
  await expect(page.locator('#toast')).toContainText(/firmware|error/i);
});
