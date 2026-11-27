// The IDE loads, shows the project and fits small screens.
import { test, expect } from '@playwright/test';
import { resetProject, openApp, serialText } from './helpers.mjs';

test.beforeEach(async ({ request }) => resetProject(request));

test('loads the mock project with the simulation running', async ({ page }) => {
  const errors = [];
  await openApp(page, { errors });
  await expect(page).toHaveTitle(/mockfw — esp32sim/);
  await expect(page.locator('#project-name')).toHaveText('mockfw');
  // circuit: board pins and the project's parts
  for (const ep of ['esp32.GPIO4', 'btn.1', 'led.A', 'r1.1', 'beat.C']) await expect(page.locator(`[data-ep="${ep}"]`)).toHaveCount(1);
  await expect(page.locator('#circuit polyline.wire')).toHaveCount(10);
  // explorer lists the project files
  await expect(page.locator('#file-tree .node[data-path="esp32sim.yaml"]')).toBeVisible();
  await expect(page.locator('#file-tree .node[data-path="mockfw.bin"]')).toBeVisible();
  // emulated time advances
  const t0 = await page.evaluate(() => window.esp32sim.status.time_ns);
  await expect.poll(() => page.evaluate(() => window.esp32sim.status.time_ns)).toBeGreaterThan(t0);
  await expect(page.locator('#status-mips')).toContainText('MHz');
  expect(await serialText(page)).toContain('mockfw ready');
  // give lazy parts (editor, language server probe) time to fail, if they do
  await page.waitForTimeout(9000);
  expect(errors, errors.join('\n')).toEqual([]);
});

test('fits a 1024x768 screen without wrapping', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await openApp(page);
  const bar = await page.locator('#topbar').evaluate((e) => ({ sw: e.scrollWidth, cw: e.clientWidth, h: e.offsetHeight }));
  expect(bar.sw).toBeLessThanOrEqual(bar.cw);
  expect(bar.h).toBeLessThanOrEqual(41);
  expect(await page.locator('#btn-build').evaluate((e) => e.offsetHeight)).toBeLessThan(30);
  const tabs = await page.locator('#bottom-tabs').evaluate((e) => e.offsetHeight);
  expect(tabs).toBeLessThanOrEqual(31);
  // the circuit hint is hidden until wiring starts
  await expect(page.locator('#circuit-hint')).toBeHidden();
});

test('activity bar switches between side panels', async ({ page }) => {
  await openApp(page);
  for (const [panel, text] of [['parts', 'PARTS'], ['git', 'SOURCE CONTROL'], ['ai', 'AI ASSISTANT'], ['debug', 'DEBUG'], ['explorer', 'EXPLORER']]) {
    await page.locator(`#activity button[data-panel="${panel}"]`).click();
    await expect(page.locator(`#panel-${panel}`)).toBeVisible();
    await expect(page.locator(`#panel-${panel} .panel-head`)).toContainText(text);
  }
});

test('bottom panel tabs', async ({ page }) => {
  await openApp(page);
  for (const t of ['plot', 'logic', 'bus', 'build', 'problems', 'log', 'serial']) {
    await page.locator(`#bottom-tabs button[data-bottom="${t}"]`).click();
    await expect(page.locator(`#bview-${t}`)).toBeVisible();
  }
  await expect(page.locator('#serial-controls')).toBeVisible();
});
