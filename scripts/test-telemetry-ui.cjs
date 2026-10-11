// Optional UI smoke check: install Playwright, then run `node scripts/test-telemetry-ui.cjs`.
// All Tauri commands and account data are synthetic. No provider requests or sign-ins occur.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '../src');
const server = http.createServer((req, res) => {
  const file = path.resolve(root, '.' + req.url.split('?')[0]);
  if (!file.startsWith(root + path.sep) || !fs.existsSync(file)) { res.writeHead(404); res.end(); return; }
  res.setHeader('Content-Type', ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.png': 'image/png' })[path.extname(file)] || 'application/octet-stream');
  fs.createReadStream(file).pipe(res);
});

const data = {
  updated_at: new Date().toISOString(),
  accounts: [
    { id: 'inactive:org', provider: 'claude', email: 'inactive@example.invalid', plan: 'max', active: false, monitor_connected: false, stale: true, error: 'monitor_setup', percent: 87 },
    { id: 'connected:org', provider: 'claude', email: 'connected@example.invalid', plan: 'max', active: true, monitor_connected: true, stale: false, error: null, monitor_error: 'monitor_reconnect', percent: 24 },
    { id: 'reconnect:org', provider: 'codex', email: 'reconnect@example.invalid', plan: 'plus', active: true, monitor_connected: true, stale: true, error: 'monitor_reconnect', percent: 52 },
  ].map(a => ({ ...a, alias: null, org: null, credits: null, resets: [], sources: a.active ? ['Windows'] : [],
    fetched_at: new Date(Date.now() - (a.stale ? 7200000 : 1000)).toISOString(),
    windows: [{ kind: 'session', used_percent: a.percent, resets_at: new Date(Date.now() - 3600000).toISOString(), window_seconds: 18000 }],
  })),
};

async function setup(page, label) {
  await page.addInitScript(({ data, label }) => {
    window.testCalls = [];
    const listeners = {};
    window.testEmit = (name, payload) => (listeners[name] || []).forEach(fn => fn({ payload }));
    window.__TAURI__ = {
      core: { invoke: async (command, args) => {
        window.testCalls.push({ command, args });
        if (command === 'get_usage' || command === 'refresh') return data;
        if (command === 'disconnect_monitoring') return { snapshot: data, revoked: true };
        if (command === 'export_diagnostics') { if (window.testExportFail) throw new Error('PRIVATE-export-error'); return 'C:\\test\\SlopBar-Diagnostics.json'; }
        if (command === 'get_settings') return { providers: { claude: true, codex: true }, language: 'en', aliases: {}, refresh_minutes: 5, launch_at_login: false, auto_update: true,
          notifications: { usage: true, resets: true, switches: true }, balancer: { claude: { auto: false, threshold: 90 }, codex: { auto: false, threshold: 90 } } };
        if (command === 'system_locale') return 'en-US';
        if (command === 'taskbar_theme') return 'dark';
        if (command === 'app_version') return '0.1.0';
        if (command === 'has_mica' || command === 'updates_enabled') return false;
        return null;
      } },
      event: { listen: async (name, fn) => { (listeners[name] ||= []).push(fn); return () => {}; } },
      window: { getCurrentWindow: () => ({ label, close: () => {} }) },
    };
  }, { data, label });
}

(async () => {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  const errors = [];
  try {
    const settings = await browser.newPage({ viewport: { width: 1000, height: 850 } });
    settings.on('pageerror', err => errors.push(String(err)));
    await setup(settings, 'settings');
    await settings.goto(base + '/settings.html');
    await settings.locator('[data-key="claude:inactive:org"] [data-monitor]').waitFor();
    assert.match(await settings.locator('#accounts').innerText(), /Last confirmed 2h ago/);
    assert.match(await settings.locator('[data-key="claude:inactive:org"]').innerText(), /87%/);
    assert.equal(await settings.locator('#auto-update').isDisabled(), true);
    assert.equal(await settings.locator('#auto-update').isChecked(), false);
    assert.match(await settings.locator('[data-key="claude:connected:org"]').innerText(), /Reconnect monitoring/);
    assert.match(await settings.locator('[data-key="claude:connected:org"] .meter').innerText(), /24%/);
    assert.equal(await settings.locator('[data-key="claude:connected:org"] .meter.stale').count(), 0);
    await settings.locator('[data-key="claude:inactive:org"] [data-monitor]').click();
    assert.match(await settings.locator('#add-hint').innerText(), /Finish the monitoring sign-in/);
    const called = await settings.evaluate(() => window.testCalls.filter(c => c.command === 'connect_monitoring' || c.command === 'switch_account'));
    assert.deepEqual(called, [{ command: 'connect_monitoring', args: { provider: 'claude', id: 'inactive:org' } }]);
    await settings.locator('[data-key="claude:connected:org"] [data-disconnect]').click();
    assert.match(await settings.locator('#add-hint').innerText(), /session revoked/);
    assert.deepEqual(await settings.evaluate(() => window.testCalls.filter(c => c.command === 'disconnect_monitoring')), [{ command: 'disconnect_monitoring', args: { provider: 'claude', id: 'connected:org' } }]);
    await settings.locator('[data-page="general"]').click();
    await settings.locator('#export-diagnostics').click();
    assert.match(await settings.locator('#diagnostics-status').innerText(), /Diagnostics saved: C:\\test\\SlopBar-Diagnostics.json/);
    await settings.evaluate(() => { window.testExportFail = true; });
    await settings.locator('#export-diagnostics').click();
    assert.match(await settings.locator('#diagnostics-status').innerText(), /Could not export diagnostics/);
    assert.doesNotMatch(await settings.locator('#diagnostics-status').innerText(), /PRIVATE/);
    assert.equal(await settings.locator('#export-diagnostics').isDisabled(), false);
    await settings.evaluate(() => {
      for (let i = 0; i < 25; i++) window.dispatchEvent(new ErrorEvent('error', { message: 'PRIVATE-message', filename: 'https://PRIVATE-url/token', error: new TypeError('PRIVATE-secret'), lineno: 42, colno: 3 }));
    });
    const frontend = await settings.evaluate(() => window.testCalls.filter(c => c.command === 'record_frontend_error'));
    assert.equal(frontend.length, 20);
    assert.deepEqual(frontend[0].args, { surface: 'settings', kind: 'type_error', line: 42, column: 3 });
    assert.doesNotMatch(JSON.stringify(frontend), /PRIVATE/);
    await settings.locator('[data-page="accounts"]').click();
    await settings.setViewportSize({ width: 760, height: 800 });
    assert.equal(await settings.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    if (process.env.UI_SCREENSHOT_DIR) await settings.screenshot({ path: path.join(process.env.UI_SCREENSHOT_DIR, 'slopbar-settings.png'), fullPage: true });

    const panel = await browser.newPage({ viewport: { width: 380, height: 1100 } });
    panel.on('pageerror', err => errors.push(String(err)));
    await setup(panel, 'panel');
    await panel.goto(base + '/index.html');
    await panel.locator('button.monitor').first().waitFor();
    const inactive = panel.locator('[data-key="claude:inactive:org"]');
    assert.match(await inactive.innerText(), /Last confirmed 2h ago/);
    assert.equal(await inactive.locator('.win .reset').count(), 0);
    await inactive.locator('button.monitor').click();
    assert.equal(await panel.locator('#panel-notice').isVisible(), true);
    await panel.evaluate(() => window.testEmit('monitoring-login', { error: 'monitor_wrong_account' }));
    assert.match(await panel.locator('#panel-notice').innerText(), /different account/);
    if (process.env.UI_SCREENSHOT_DIR) await panel.screenshot({ path: path.join(process.env.UI_SCREENSHOT_DIR, 'slopbar-panel.png'), fullPage: true });

    // Every supported language supplies the monitoring controls and error strings.
    const translations = await panel.evaluate(async () => {
      const { t, setLanguage, LANGUAGES } = await import('/i18n.js');
      const keys = ['monitorDisconnect', 'monitorDisconnected', 'monitorRevokeFailed', 'monitor_uncertain', 'monitorConnect', 'monitorReconnect', 'monitorConnected', 'monitorPending', 'monitor_setup', 'monitor_reconnect', 'monitor_storage', 'monitor_network', 'monitor_wrong_account', 'monitor_shared_login', 'monitor_login_failed', 'monitor_busy', 'lastObserved', 'lastReading', 'usageUnknown', 'manualUpdates'];
      setLanguage('en'); const english = keys.map(k => t(k));
      return Object.keys(LANGUAGES).filter(l => l !== 'en').map(l => {
        setLanguage(l); return { l, values: keys.map(k => t(k)), english };
      });
    });
    for (const { l, values, english } of translations) for (let i = 0; i < values.length; i++) assert.notEqual(values[i], english[i], `${l} translation ${i}`);

    const dock = await browser.newPage();
    dock.on('pageerror', err => errors.push(String(err)));
    await setup(dock, 'dock');
    await dock.goto(base + '/index.html');
    await dock.locator('.unknown-mark').waitFor();
    assert.equal(await dock.locator('#compact .ring').count(), 1); // Fresh Claude; stale Codex is unknown.
    assert.deepEqual(errors, []);
    console.log('PASS: settings, flyout, and taskbar; monitoring action, stale readings, errors, manual updates, and minimum window width.');
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(err => { console.error(err); server.close(); process.exitCode = 1; });
