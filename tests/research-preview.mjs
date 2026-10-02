import { createRequire } from 'node:module';
import { createHash } from 'node:crypto';
import { mkdtemp, realpath, rm, mkdir, readFile, writeFile, rename } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { tmpdir } from 'node:os';
import assert from 'node:assert/strict';

// A constructed product preview. The titles and hosts are public-site metadata
// observed in the earlier real-site demo; the visits and visit batches below are
// invented. This is never described as a fresh browser capture or user data.
const publicMetadata = [
  { site: 'www.amazon.com', title: 'ergonomic office chair', query: 'ergonomic office chair' },
  { site: 'ergonomics.ucla.edu', title: '4 Steps to Set Up Your Workstation | Ergonomics' },
  { site: 'www.ccohs.ca', title: 'CCOHS: Office Ergonomics - Ergonomic Chair' },
  { site: 'www.hse.gov.uk', title: 'Display screen equipment (DSE) workstation checklist - HSE' },
  { site: 'ergo.human.cornell.edu', title: 'CUergo: Computer Workstation Ergonomics Guidelines' },
];

const playwrightPackage = process.env.SEREIN_PLAYWRIGHT_PACKAGE;
const chromeBinary = process.env.SEREIN_CHROMIUM_BIN;
assert.ok(playwrightPackage, 'Set SEREIN_PLAYWRIGHT_PACKAGE to Playwright package.json.');
assert.ok(chromeBinary, 'Set SEREIN_CHROMIUM_BIN to Chrome for Testing.');
const require = createRequire(path.resolve(playwrightPackage));
const { chromium } = require('playwright');

const root = process.cwd();
const temp = await realpath(await mkdtemp(path.join(tmpdir(), 'serein-preview-')));
const extension = path.join(root, 'apps/extension/.output/chrome-mv3');
const screenshots = path.join(root, 'docs/screenshots');
const resultPath = path.join(root, 'docs/research-preview-results.json');
const browserEnv = {
  ...process.env,
  SEREIN_DATA_DIR: path.join(temp, 'data'),
  SEREIN_INSTALL_HOME: path.join(temp, 'home'),
};
let context;

try {
  context = await chromium.launchPersistentContext(path.join(temp, 'profile'), {
    headless: true,
    executablePath: chromeBinary,
    args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`],
    viewport: { width: 1440, height: 1050 },
    deviceScaleFactor: 1,
    env: browserEnv,
  });
  const worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker');
  const extensionId = new URL(worker.url()).host;
  const page = await context.newPage();
  const pageErrors = [];
  page.on('pageerror', error => pageErrors.push(error.message));
  const dashboardUrl = `chrome-extension://${extensionId}/dashboard.html#context`;
  await page.goto(dashboardUrl);
  await page.getByRole('heading', { name: 'Your context', exact: true }).waitFor();
  const send = message => page.evaluate(value => chrome.runtime.sendMessage(value), message);
  const fresh = await send({ type: 'state' });
  assert.equal(fresh.state.policy.consent, false);

  const ticket = await send({
    type: 'ticket', adapters: ['generic'], label: 'Constructed Serein product preview',
  });
  const setup = JSON.parse(execFileSync('sh', [
    path.join(root, 'skills/serein-context/scripts/connect.sh'),
  ], { input: JSON.stringify(ticket), env: browserEnv }).toString());
  assert.equal(setup.status, 'ok');
  const manifest = await readFile(setup.manifest, 'utf8');
  const nativeManifest = JSON.parse(manifest);
  const skillRuntime = path.join(root, 'skills/serein-context/runtime/macos-arm64');
  const nativeHostSha256 = createHash('sha256').update(await readFile(nativeManifest.path)).digest('hex');
  const bundledHostSha256 = createHash('sha256').update(await readFile(path.join(skillRuntime, 'serein-host'))).digest('hex');
  const nativeRuntimeCliSha256 = createHash('sha256').update(await readFile(path.join(skillRuntime, 'serein'))).digest('hex');
  assert.equal(nativeHostSha256, bundledHostSha256,
    'the registered host must match the final bundled skill helper');
  const nativeDirectory = path.join(temp, 'profile', 'NativeMessagingHosts');
  await mkdir(nativeDirectory, { recursive: true });
  await writeFile(path.join(nativeDirectory, 'com.serein.context.json'), manifest, {
    flag: 'wx', mode: 0o600,
  });
  await page.waitForFunction(async () =>
    (await chrome.runtime.sendMessage({ type: 'state' })).state.paired,
  {}, { timeout: 20_000 });
  await send({ type: 'policy', patch: { consent: true, recall_enabled: true } });
  const connected = await send({ type: 'state' });
  assert.equal(connected.state.policy.consent, true);

  const events = [];
  for (const [visitBatchIndex, daysAgo] of [3, 1].entries()) {
    for (const [itemIndex, item] of publicMetadata.entries()) {
      events.push({
        event_id: crypto.randomUUID(),
        visit_id: crypto.randomUUID(),
        site_key: item.site,
        site_epoch: 0,
        observed_at: new Date(Date.now() - daysAgo * 86_400_000 + itemIndex * 60_000).toISOString(),
        kind: item.query ? 'search' : 'visit',
        title: item.title,
        ...(item.query ? { search_query: item.query } : {}),
        foreground_seconds: 150 + visitBatchIndex * 20 + itemIndex * 10,
      });
    }
  }
  const ingest = {
    protocol: 1,
    request_id: crypto.randomUUID(),
    source_id: ticket.source_id,
    op: 'ingest',
    capture_epoch: connected.state.policy.capture_epoch,
    payload: { events },
  };
  const acknowledgement = await page.evaluate(request =>
    chrome.runtime.sendNativeMessage('com.serein.context', request), ingest);
  assert.equal(acknowledgement.acknowledged_ids?.length, events.length,
    JSON.stringify(acknowledgement));

  const dashboard = await send({ type: 'host', op: 'dashboard' });
  assert.ok(dashboard.model_available, 'bundled semantic model is unavailable');
  assert.equal(dashboard.cards.length, publicMetadata.length);
  assert.ok(dashboard.memories.length >= 1, 'constructed research did not form a group');
  for (const item of publicMetadata) {
    assert.ok(dashboard.cards.some(card => card.site === item.site &&
      card.text === (item.query ?? item.title)), `missing ${item.site}`);
  }

  await page.evaluate(() => chrome.storage.local.set({ theme: 'light', themeMode: 'manual' }));
  await page.reload();
  await page.getByRole('heading', { name: 'Research memories', exact: true }).waitFor();
  await page.waitForFunction(expected =>
    document.querySelectorAll('article.memory-card').length === expected,
  dashboard.memories.length, { timeout: 25_000 });
  const memorySources = page.locator('article.memory-card details.memory-sources').first();
  await memorySources.locator('summary').click();
  assert.equal(await memorySources.evaluate(node => node.open), true);
  assert.equal(await page.locator('.raw-activity details').evaluate(node => node.open), false);
  assert.deepEqual(pageErrors, []);

  const stagedLight = path.join(temp, 'research-preview-light.png');
  const stagedDark = path.join(temp, 'research-preview-dark.png');
  await page.screenshot({ animations: 'disabled', path: stagedLight, fullPage: true });
  await page.evaluate(() => chrome.storage.local.set({ theme: 'dark', themeMode: 'manual' }));
  await page.reload();
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
  await page.waitForFunction(expected =>
    document.querySelectorAll('article.memory-card').length === expected,
  dashboard.memories.length, { timeout: 25_000 });
  await page.locator('article.memory-card details.memory-sources').first().locator('summary').click();
  await page.screenshot({ animations: 'disabled', path: stagedDark, fullPage: true });

  const extensionManifest = JSON.parse(await readFile(path.join(extension, 'manifest.json'), 'utf8'));

  await mkdir(screenshots, { recursive: true });
  await rename(stagedLight, path.join(screenshots, 'research-preview-light.png'));
  await rename(stagedDark, path.join(screenshots, 'research-preview-dark.png'));
  const result = {
    capture_mode: 'Constructed replay of five public-site title/host records previously observed in the v0.1.2 Chrome demo; not a fresh live-site capture or user history.',
    browser: context.browser()?.version(),
    isolated_profile: true,
    synthetic_events: events.length,
    synthetic_visit_batches: 2,
    distinct_public_sites: publicMetadata.length,
    extension_manifest_version: extensionManifest.version,
    native_runtime_cli_sha256: nativeRuntimeCliSha256,
    native_host_sha256: nativeHostSha256,
    ingress: 'Actual native host ingest request with constructed records; extension tabs observer was not used for these observations.',
    dashboard_raw_cards: dashboard.cards.length,
    dashboard_research_groups: dashboard.memories.length,
    model_available: dashboard.model_available,
    source_demo: 'docs/research-demo-results.json',
    screenshots: [
      'docs/screenshots/research-preview-light.png',
      'docs/screenshots/research-preview-dark.png',
    ],
  };
  await writeFile(resultPath, `${JSON.stringify(result, null, 2)}\n`);
  console.log(JSON.stringify(result, null, 2));
} finally {
  if (context) await context.close();
  await rm(temp, { recursive: true, force: true });
}
