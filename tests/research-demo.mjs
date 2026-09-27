import { createRequire } from 'node:module';
import { mkdtemp, realpath, rm, mkdir, readFile, writeFile, unlink } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { tmpdir } from 'node:os';
import assert from 'node:assert/strict';

const playwrightPackage = process.env.SEREIN_PLAYWRIGHT_PACKAGE;
const chromeBinary = process.env.SEREIN_CHROMIUM_BIN;
assert.ok(playwrightPackage, 'Set SEREIN_PLAYWRIGHT_PACKAGE to the Playwright package.json path.');
assert.ok(chromeBinary, 'Set SEREIN_CHROMIUM_BIN to the Chrome for Testing executable.');
const require = createRequire(path.resolve(playwrightPackage));
const { chromium } = require('playwright');

const root = process.cwd();
const fixturePath = path.join(root, 'tests/fixtures/research-journey.json');
const fixture = JSON.parse(await readFile(fixturePath, 'utf8'));
const seedFingerprint = fixture.seedFingerprint;
const expectedFingerprint = 'a0bc9607ddce3923';
assert.equal(seedFingerprint, expectedFingerprint);

// The committed fingerprint is the first 64 bits of SHA-256 over the supplied
// 96-character shell seed. This xorshift stream changes only the public-page
// order and dwell times; all page titles and events still come from Chrome.
const mask64 = (1n << 64n) - 1n;
let randomState = BigInt(`0x${seedFingerprint}`);
function random() {
  randomState ^= (randomState << 13n) & mask64;
  randomState ^= randomState >> 7n;
  randomState ^= (randomState << 17n) & mask64;
  randomState &= mask64;
  return Number(randomState >> 11n) / 9007199254740992;
}
function choose(items) {
  return items[Math.floor(random() * items.length)];
}
function shuffle(items) {
  const output = [...items];
  for (let i = output.length - 1; i > 0; i--) {
    const j = Math.floor(random() * (i + 1));
    [output[i], output[j]] = [output[j], output[i]];
  }
  return output;
}
const search = choose(fixture.searches);
const journey = [search, ...shuffle(fixture.pages)];
const dwellMs = () => Math.round(5_200 + random() * 650);

const temp = await realpath(await mkdtemp(path.join(tmpdir(), 'serein-research-demo-')));
const dataDir = path.join(temp, 'serein-data');
const installHome = path.join(temp, 'serein-install');
const extension = path.join(root, 'apps/extension/.output/chrome-mv3');
const screenshotDir = path.join(root, 'docs/screenshots');
await mkdir(screenshotDir, { recursive: true });
let context;
let registeredPath;

try {
  const browserEnv = {
    ...process.env,
    SEREIN_DATA_DIR: dataDir,
    SEREIN_INSTALL_HOME: installHome,
  };
  const launchOptions = {
    headless: false,
    args: [
      `--disable-extensions-except=${extension}`,
      `--load-extension=${extension}`,
      '--no-first-run',
      '--disable-sync',
    ],
    viewport: { width: 1440, height: 1050 },
    deviceScaleFactor: 1,
    env: browserEnv,
    executablePath: chromeBinary,
  };
  context = await chromium.launchPersistentContext(temp, launchOptions);
  const worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker');
  const extensionId = new URL(worker.url()).host;
  const controlPage = await context.newPage();
  const pageErrors = [];
  controlPage.on('pageerror', error => pageErrors.push(error.message));
  const extensionUrl = `chrome-extension://${extensionId}/dashboard.html`;
  await controlPage.goto(`${extensionUrl}#connections`);
  await controlPage.getByRole('heading', { name: 'Connections', exact: true }).waitFor();

  const send = message => controlPage.evaluate(
    value => chrome.runtime.sendMessage(value), message,
  );
  const initial = await send({ type: 'state' });
  assert.equal(initial.state.policy.consent, false, 'fresh Chrome profile must start without consent');

  const ticket = await send({
    type: 'ticket',
    adapters: ['generic'],
    label: 'Synthetic public-site research demo',
  });
  const setup = JSON.parse(execFileSync(
    'sh', [path.join(root, 'skills/serein-context/scripts/connect.sh')],
    { input: JSON.stringify(ticket), env: browserEnv },
  ).toString());
  assert.equal(setup.status, 'ok');

  const manifest = JSON.parse(await readFile(setup.manifest, 'utf8'));
  const invocationLog = path.join(temp, 'native-host-invocations.txt');
  const wrapper = path.join(temp, 'native-host-wrapper');
  const quote = value => `'${value.replaceAll("'", "'\\''")}'`;
  await writeFile(wrapper,
    `#!/bin/sh\nprintf '1\\n' >> ${quote(invocationLog)}\nexec ${quote(manifest.path)} "$@"\n`,
    { mode: 0o700 },
  );
  manifest.path = wrapper;
  const nativeDirectory = path.join(temp, 'NativeMessagingHosts');
  await mkdir(nativeDirectory, { recursive: true });
  registeredPath = path.join(nativeDirectory, 'com.serein.context.json');
  await writeFile(registeredPath, JSON.stringify(manifest), { mode: 0o600 });

  await controlPage.waitForFunction(async () =>
    (await chrome.runtime.sendMessage({ type: 'state' })).state.paired,
  {}, { timeout: 20_000 });
  await send({ type: 'policy', patch: { consent: true, recall_enabled: true } });
  const consented = await send({ type: 'state' });
  assert.equal(consented.state.policy.consent, true);
  assert.equal(consented.state.policy.recall_enabled, true);

  const capturePage = await context.newPage();
  const visits = [];
  for (const target of journey) {
    const plannedDwellMs = dwellMs();
    await capturePage.bringToFront();
    let response;
    let navigationError;
    try {
      response = await capturePage.goto(target.url, {
        waitUntil: 'domcontentloaded',
        timeout: 18_000,
      });
    } catch (error) {
      navigationError = error.message;
    }

    if (!navigationError) {
      await controlPage.waitForFunction(expectedHost => chrome.tabs.query(
        { active: true, lastFocusedWindow: true },
      ).then(([tab]) => {
        if (!tab?.url) return false;
        try {
          return new URL(tab.url).hostname === expectedHost && Boolean(tab.title?.trim());
        } catch {
          return false;
        }
      }), target.host, { timeout: 6_000 }).catch(() => {});
      const foreground = await controlPage.evaluate(async () => ({
        focused: (await chrome.windows.getLastFocused()).focused,
        idle: await chrome.idle.queryState(60),
      }));
      assert.equal(foreground.focused, true, `Chrome was not focused during ${target.name}`);
      assert.equal(foreground.idle, 'active', `Chrome was idle during ${target.name}`);
    }
    const snapshot = await controlPage.evaluate(async () => {
      const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
      if (!tab) return null;
      return { url: tab.url, title: tab.title || '', incognito: Boolean(tab.incognito) };
    });
    assert.ok(snapshot, `Chrome did not report an active tab for ${target.name}`);
    assert.equal(snapshot.incognito, false);

    let observedHost = '';
    try {
      observedHost = new URL(snapshot.url).hostname;
    } catch {
      // Chromium can replace an unreachable navigation with an internal error URL.
    }
    const status = response?.status() ?? null;
    let observedQuery;
    if (target.queryParam) {
      try {
        const values = new URL(snapshot.url).searchParams.getAll(target.queryParam);
        if (values.length === 1) observedQuery = values[0].replaceAll('+', ' ');
      } catch {
        // An internal browser error page has no public search query to record.
      }
    }
    const loaded = !navigationError && observedHost === target.host && status !== null &&
      status < 400 && Boolean(snapshot.title.trim());
    const titleMatches = target.titleContains
      ? snapshot.title.toLowerCase().includes(target.titleContains.toLowerCase())
      : true;
    const actualSearch = Boolean(target.queryParam) && observedHost === target.host &&
      observedQuery === target.query;
    const actualPage = loaded && titleMatches;
    const captured = actualSearch || actualPage;

    if (actualPage) {
      await capturePage.waitForTimeout(plannedDwellMs);
    }
    if (captured) {
      const safeTitle = snapshot.title === snapshot.url ? null : snapshot.title;
      visits.push({
        name: target.name,
        url: target.url,
        host: target.host,
        ...(safeTitle ? { chromeTitle: safeTitle } : {}),
        status,
        ...(actualPage ? { dwellMs: plannedDwellMs } : {}),
        source: 'normal Chrome tabs observer',
        ...(actualSearch ? { observedQuery } : {}),
      });
    } else {
      // A page with a failed response, challenge, or unexpected title is left
      // for less than the five-second retention threshold. No fixture event is
      // synthesized for it; the run records the limitation and moves on.
      visits.push({
        name: target.name,
        url: target.url,
        host: target.host,
        status,
        source: 'not retained',
      });
    }
  }

  // Bringing the real extension document forward finalizes the last public
  // visit through the normal tabs.onActivated observer.
  await controlPage.bringToFront();
  await controlPage.goto(`${extensionUrl}#context`);
  await controlPage.getByRole('heading', { name: 'Your context', exact: true }).waitFor();
  const expectedRetained = visits.filter(item => item.source === 'normal Chrome tabs observer');
  await controlPage.waitForFunction(async expected => {
    const state = await chrome.runtime.sendMessage({ type: 'state' });
    return state.queued >= expected;
  }, expectedRetained.length, { timeout: 20_000 });

  // The records are still the extension's captured outbox. Accelerate only
  // the real batch alarm so this short demo can complete without waiting a
  // full minute; the alarm still invokes normal extension flush/native ingest.
  await controlPage.evaluate(async () => {
    await chrome.alarms.create('batch', { when: Date.now() + 750 });
  });
  await controlPage.waitForFunction(async () => {
    const state = await chrome.runtime.sendMessage({ type: 'state' });
    return state.queued === 0 && state.controls === 0 && state.state.paired;
  }, {}, { timeout: 20_000 });

  const nativeCalls = Number((await readFile(invocationLog, 'utf8').catch(() => ''))
    .split('\n').filter(Boolean).length);
  assert.ok(nativeCalls >= 3, `expected native hello, policy, and ingest calls; saw ${nativeCalls}`);
  const dashboard = await send({ type: 'host', op: 'dashboard' });
  const retainedSites = new Set(dashboard.cards.map(card => card.site));
  assert.ok(expectedRetained.length >= 4, `only ${expectedRetained.length} actual public observations were retained`);
  for (const visit of expectedRetained) {
    assert.ok(retainedSites.has(visit.host), `native dashboard lacks captured site ${visit.host}`);
    const expectedText = visit.observedQuery ?? visit.chromeTitle;
    assert.ok(expectedText, `Chrome tabs API did not expose safe display metadata for ${visit.host}`);
    assert.ok(
      dashboard.cards.some(card => card.site === visit.host && card.text === expectedText),
      `native dashboard lacks actual captured title/query for ${visit.host}`,
    );
  }
  assert.equal(pageErrors.length, 0, `extension page errors: ${pageErrors.join('; ')}`);

  await controlPage.evaluate(() => chrome.storage.local.set({ theme: 'light', themeMode: 'manual' }));
  await controlPage.reload();
  await controlPage.getByRole('heading', { name: 'Your context', exact: true }).waitFor();
  await controlPage.waitForFunction(() => document.documentElement.dataset.theme === 'light');
  await controlPage.waitForFunction(expected =>
    document.querySelectorAll('article.context-card').length === expected,
  dashboard.cards.length, { timeout: 25_000 });
  const modelStatusText = dashboard.model_available
    ? 'Semantic indexing is available.'
    : 'Lexical index active. A verified semantic model pack is not installed; multilingual semantic recall is not enabled.';
  await controlPage.getByText(modelStatusText, { exact: true }).waitFor();
  for (const card of dashboard.cards) {
    await controlPage.locator('article.context-card')
      .filter({ hasText: card.text }).first().waitFor();
  }
  const lightPath = path.join(screenshotDir, 'research-demo-light.png');
  await controlPage.screenshot({ animations: 'disabled', path: lightPath, fullPage: true });
  await controlPage.getByRole('button', { name: 'Toggle color theme' }).click();
  await controlPage.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
  const darkPath = path.join(screenshotDir, 'research-demo-dark.png');
  await controlPage.screenshot({ animations: 'disabled', path: darkPath, fullPage: true });

  const result = {
    scenario: fixture.topic,
    browser: context.browser()?.version(),
    isolatedProfile: true,
    identity: 'Synthetic research activity; no account, private browsing, or existing browser profile',
    seedFingerprint,
    capture: 'Normal Chrome tabs metadata observer only; no fixture records injected',
    batchDelivery: 'Real extension batch alarm accelerated to 750 ms; native helper handled actual captured outbox',
    nativeMessaging: 'PASS: pairing hello, consent policy update, captured-event ingest, dashboard read',
    nativeHostInvocations: nativeCalls,
    retainedPublicPages: expectedRetained.length,
    dashboardCards: dashboard.cards.length,
    modelAvailable: dashboard.model_available,
    modelStatus: modelStatusText,
    visits,
    screenshots: [
      'docs/screenshots/research-demo-light.png',
      'docs/screenshots/research-demo-dark.png',
    ],
    checks: [
      'fresh-profile consent defaults off',
      'pairing completed through the actual extension/native host',
      'capture began after explicit test consent',
      'tab metadata was read via chrome.tabs from the extension only',
      'public page title or search query appeared in native dashboard',
      'no page body or DOM text was read',
      'no extension observation fixture was injected',
      'real browser alarm delivered captured outbox to native helper',
      'light and dark dashboard screenshots captured',
      'no extension page errors',
    ],
  };
  console.log(JSON.stringify(result, null, 2));
} finally {
  if (context) await context.close();
  if (registeredPath) await unlink(registeredPath).catch(() => {});
  await rm(temp, { recursive: true, force: true });
}
