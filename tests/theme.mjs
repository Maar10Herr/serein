import { createRequire } from 'node:module';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';

const require = createRequire(path.resolve(process.env.SEREIN_PLAYWRIGHT_PACKAGE));
const { chromium } = require('playwright');
const root = process.cwd();
const extension = path.join(root, 'apps/extension/.output/chrome-mv3');
const profile = await mkdtemp(path.join(tmpdir(), 'serein-theme-'));
let context;
try {
  context = await chromium.launchPersistentContext(profile, {
    headless: true,
    executablePath: process.env.SEREIN_CHROMIUM_BIN,
    args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`],
    colorScheme: 'dark',
  });
  const worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker');
  const id = new URL(worker.url()).host;
  const page = await context.newPage();
  await page.goto(`chrome-extension://${id}/dashboard.html#connections`);
  const toggle = page.getByRole('button', { name: 'Toggle color theme' });
  await toggle.waitFor();
  await page.waitForFunction(() => !document.querySelector('select[aria-label="Language"]').disabled);
  const appearance = () => page.evaluate(() => ({
    theme: document.documentElement.getAttribute('data-theme'),
    background: getComputedStyle(document.documentElement).backgroundColor,
  }));

  assert.deepEqual(await appearance(), { theme: null, background: 'rgb(24, 27, 26)' });
  await page.emulateMedia({ colorScheme: 'light' });
  assert.deepEqual(await appearance(), { theme: null, background: 'rgb(247, 246, 242)' });
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.evaluate(() => chrome.storage.local.set({ theme: 'light' }));
  await page.reload();
  await page.waitForFunction(() => !document.querySelector('select[aria-label="Language"]').disabled);
  assert.deepEqual(await appearance(), { theme: null, background: 'rgb(24, 27, 26)' });
  await page.evaluate(() => chrome.storage.local.set({ theme: 'dark' }));
  await page.reload();
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
  assert.equal((await appearance()).background, 'rgb(24, 27, 26)');
  await page.evaluate(() => chrome.storage.local.remove(['theme', 'themeMode']));
  await page.reload();
  await page.waitForFunction(() => !document.querySelector('select[aria-label="Language"]').disabled);
  assert.deepEqual(await appearance(), { theme: null, background: 'rgb(24, 27, 26)' });
  await page.goto(`chrome-extension://${id}/popup.html`);
  assert.deepEqual(await appearance(), { theme: null, background: 'rgb(24, 27, 26)' });
  await page.goto(`chrome-extension://${id}/dashboard.html#connections`);
  await page.waitForFunction(() => !document.querySelector('select[aria-label="Language"]').disabled);
  await page.emulateMedia({ colorScheme: 'light' });
  await toggle.click();
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
  assert.deepEqual(await appearance(), { theme: 'dark', background: 'rgb(24, 27, 26)' });
  assert.equal(await page.evaluate(async () => (await chrome.storage.local.get('theme')).theme), 'dark');
  await page.reload();
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
  assert.equal((await appearance()).background, 'rgb(24, 27, 26)');
  await page.getByRole('button', { name: 'Toggle color theme' }).click();
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light');
  assert.deepEqual(await appearance(), { theme: 'light', background: 'rgb(247, 246, 242)' });
  console.log('PASS: settings follows system dark/light by default and preserves explicit overrides');
} finally {
  await context?.close();
  await rm(profile, { recursive: true, force: true });
}
