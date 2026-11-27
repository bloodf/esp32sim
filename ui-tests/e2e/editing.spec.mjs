// Building the circuit in the GUI: palette, wiring, selection, inspector,
// undo/redo, the YAML view and the file explorer.
import { test, expect } from '@playwright/test';
import { resetProject, openApp } from './helpers.mjs';

test.beforeEach(async ({ request, page }) => {
  await resetProject(request);
  await openApp(page);
});

const project = async (request) => (await request.get('/api/project')).json();
const ids = async (request) => (await project(request)).components.map((c) => c.id);

async function addFromPalette(page, query) {
  await page.locator('#activity button[data-panel="parts"]').click();
  await page.locator('#parts-search').fill(query);
  await page.locator('#parts-list .part-item').first().click();
}

test('adding a part from the palette, then undo and redo', async ({ page, request }) => {
  await addFromPalette(page, 'oled');
  await expect(page.locator('[data-ep="oled1.SDA"]')).toHaveCount(1);
  await expect.poll(() => ids(request)).toContain('oled1');
  await page.locator('#cz-undo').click();
  await expect(page.locator('[data-ep="oled1.SDA"]')).toHaveCount(0);
  await expect.poll(() => ids(request)).not.toContain('oled1');
  await page.locator('#cz-redo').click();
  await expect.poll(() => ids(request)).toContain('oled1');
});

test('parts search ranks the best matches first', async ({ page }) => {
  await page.locator('#activity button[data-panel="parts"]').click();
  await page.locator('#parts-search').fill('pcf8575');
  await expect(page.locator('#parts-list .part-item .pi-name').first()).toContainText('PCF8575');
  await page.locator('#parts-search').fill('keypad');
  await expect(page.locator('#parts-list .part-item')).toHaveCount(2);
  await page.locator('#parts-search').fill('');
  expect(await page.locator('#parts-list .part-cat').count()).toBeGreaterThan(5);
});

test('wiring two pins by clicking them, then deleting the wire', async ({ page, request }) => {
  await addFromPalette(page, 'buzzer');
  let id;
  await expect.poll(async () => (id = (await ids(request)).find((x) => x.startsWith('buzzer')))).toBeTruthy();
  await page.locator(`[data-ep="${id}.+"]`).dispatchEvent('pointerdown');
  await expect(page.locator('#circuit-hint')).toContainText(`Wiring from ${id}.+`);
  await page.locator('[data-ep="esp32.GPIO25"]').dispatchEvent('pointerdown');
  await expect.poll(async () => JSON.stringify((await project(request)).connections)).toContain(`"from":"${id}.+","to":"esp32.GPIO25"`);
  const before = await page.locator('#circuit polyline.wire').count();
  await page.locator('#circuit polyline.wire').last().dispatchEvent('pointerdown');
  await page.keyboard.press('Delete');
  await expect(page.locator('#circuit polyline.wire')).toHaveCount(before - 1);
  await expect.poll(async () => JSON.stringify((await project(request)).connections)).not.toContain(`${id}.+`);
});

test('Escape cancels a wire in progress', async ({ page }) => {
  await page.locator('[data-ep="led.A"]').dispatchEvent('pointerdown');
  await expect(page.locator('#circuit-hint')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.locator('#circuit-hint')).toHaveText(/Click a pin to start a wire/);
});

test('inspector: live state, properties and delete', async ({ page, request }) => {
  await page.locator('g.part', { has: page.locator('[data-ep="led.A"]') }).dispatchEvent('pointerdown');
  const insp = page.locator('#inspector-body');
  await expect(insp.locator('.insp-title')).toHaveText('led');
  await expect(insp.locator('.insp-kind')).toContainText('LED');
  await insp.locator('.field', { hasText: 'color' }).locator('select').selectOption('blue');
  await expect.poll(async () => (await project(request)).components.find((c) => c.id === 'led').color).toBe('blue');
  await insp.getByRole('button', { name: /Delete/ }).click();
  await expect.poll(() => ids(request)).not.toContain('led');
  await expect(page.locator('[data-ep="led.A"]')).toHaveCount(0);
});

test('inspector controls drive inputs of the selected part', async ({ page }) => {
  await page.locator('g.part', { has: page.locator('[data-ep="btn.1"]') }).dispatchEvent('pointerdown');
  await expect(page.locator('#inspector-body .insp-title')).toHaveText('btn');
  await expect(page.locator('#inspector-body .summary')).not.toHaveText('…');
});

test('editing esp32sim.yaml as text updates the circuit', async ({ page, request }) => {
  await page.locator('#file-tree .node[data-path="esp32sim.yaml"]').click();
  await expect(page.locator('#tabs .tab.active')).toContainText('esp32sim.yaml');
  await expect(page.locator('#editor .view-lines')).toContainText('mockfw');
  // replace the text through Monaco, then save with Ctrl+S
  await page.evaluate(() => {
    const m = window.monaco.editor.getModels().find((x) => x.uri.path.endsWith('esp32sim.yaml'));
    m.setValue(m.getValue().replace('components:\n', 'components:\n  - {id: bz, type: buzzer, x: 700, y: 60}\n'));
  });
  await page.locator('#editor .view-lines').click();
  await page.keyboard.press('Control+s');
  await expect.poll(() => ids(request)).toContain('bz');
  await page.locator('#tabs .tab', { hasText: 'Circuit' }).click();
  await expect(page.locator('[data-ep="bz.+"]')).toHaveCount(1);
});

test('explorer: new file, open in the editor, delete', async ({ page, request }) => {
  page.on('dialog', async (d) => {
    if (d.message().startsWith('New file')) await d.accept('src/main.cpp');
    else if (d.message().includes('Type "rename" or "delete"')) await d.accept('delete');
    else await d.accept();
  });
  await page.locator('#new-file').click();
  await expect(page.locator('#file-tree .node[data-path="src/main.cpp"]')).toBeVisible();
  await expect(page.locator('#tabs .tab.active')).toContainText('main.cpp');
  await page.evaluate(() => window.monaco.editor.getModels().find((x) => x.uri.path.endsWith('src/main.cpp')).setValue('void setup() {}\nvoid loop() {}\n'));
  await page.locator('#editor .view-lines').click();
  await page.keyboard.press('Control+s');
  await expect.poll(async () => (await (await request.get('/api/file?path=src/main.cpp')).json()).content).toContain('void loop');
  await page.locator('#file-tree .node[data-path="src/main.cpp"]').click({ button: 'right' });
  await expect(page.locator('#file-tree .node[data-path="src/main.cpp"]')).toHaveCount(0);
});

test('zoom and fit keep the circuit visible', async ({ page }) => {
  const tr = () => page.locator('#circuit > g').first().getAttribute('transform');
  const a = await tr();
  await page.locator('#cz-in').click();
  const b = await tr();
  expect(b).not.toBe(a);
  await page.locator('#cz-fit').click();
  await expect(page.locator('[data-ep="btn.1"]')).toBeInViewport();
});
