import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import net from 'node:net';
import { dirname, resolve } from 'node:path';

import { chromium } from 'playwright';

const root = resolve(import.meta.dirname, '..');
const outputPath = resolve(root, 'demo/output/portalshield-demo.webm');
const executable = resolve(
  root,
  process.platform === 'win32'
    ? 'target/release/portalshield-rpc.exe'
    : 'target/release/portalshield-rpc',
);

if (!existsSync(executable)) {
  throw new Error('Release binary not found. Run cargo build --release --locked first.');
}

const port = await getFreePort();
const baseUrl = `http://127.0.0.1:${port}`;
const gateway = spawn(
  executable,
  [
    '--listen', `127.0.0.1:${port}`,
    '--upstreams', 'wss://mainnet.portaldot.io',
    '--expected-chain', 'Portaldot Mainnet',
    '--probe-interval', '60',
  ],
  { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] },
);

let gatewayLogs = '';
gateway.stdout.on('data', (chunk) => { gatewayLogs += chunk.toString(); });
gateway.stderr.on('data', (chunk) => { gatewayLogs += chunk.toString(); });

await waitForHealthyGateway(`${baseUrl}/health`, 30_000);

const browserCandidates = [
  process.env.PLAYWRIGHT_EXECUTABLE_PATH,
  chromium.executablePath(),
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
].filter(Boolean);
const browserExecutable = browserCandidates.find(existsSync);

if (!browserExecutable) {
  gateway.kill();
  throw new Error('No Chromium-compatible browser found.');
}

await mkdir(dirname(outputPath), { recursive: true });
let browser;
let context;
let page;
let video;

try {
  browser = await chromium.launch({
    headless: true,
    executablePath: browserExecutable,
    args: ['--window-size=1440,900', '--hide-scrollbars', '--force-device-scale-factor=1'],
  });
  context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
    colorScheme: 'dark',
    recordVideo: {
      dir: dirname(outputPath),
      size: { width: 1440, height: 900 },
    },
  });
  page = await context.newPage();
  await page.setViewportSize({ width: 1440, height: 900 });
  video = page.video();

  await page.goto(`${baseUrl}/dashboard`, { waitUntil: 'networkidle' });
  await page.getByText('Portaldot Mainnet').waitFor({ timeout: 10_000 });
  await page.waitForTimeout(19_000);

  await page.locator('#preset-selector').selectOption('chain');
  await page.waitForTimeout(1_200);
  await page.getByRole('button', { name: 'Transmit to PortalShield Gateway' }).click();
  await page.locator('#rpc-output').getByText('Portaldot Mainnet').waitFor();
  await page.waitForTimeout(7_000);

  await page.locator('#preset-selector').selectOption('forbidden');
  await page.waitForTimeout(1_200);
  await page.getByRole('button', { name: 'Transmit to PortalShield Gateway' }).click();
  await page.locator('#rpc-output').getByText('BLOCKED').waitFor();
  await page.waitForTimeout(8_000);

  await page.getByText('Active Security Policies').scrollIntoViewIfNeeded();
  await page.waitForTimeout(7_000);

  await page.goto(`${baseUrl}/metrics`, { waitUntil: 'networkidle' });
  await page.getByText('portalshield_requests_total').waitFor();
  await page.waitForTimeout(9_000);

  await page.goto(`${baseUrl}/dashboard`, { waitUntil: 'networkidle' });
  await page.getByText('Portaldot Mainnet').waitFor({ timeout: 10_000 });
  await page.waitForTimeout(7_000);
} finally {
  if (page) await page.close();
  if (video) await video.saveAs(outputPath);
  if (context) await context.close();
  if (browser) await browser.close();
  gateway.kill();
}

console.log(`Saved raw demo to ${outputPath}`);

async function getFreePort() {
  return await new Promise((resolvePort, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const selectedPort = address.port;
      server.close((error) => error ? reject(error) : resolvePort(selectedPort));
    });
  });
}

async function waitForHealthyGateway(url, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (gateway.exitCode !== null) {
      throw new Error(`PortalShield exited before startup:\n${gatewayLogs}`);
    }
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      // The listener may not be bound yet.
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 500));
  }
  gateway.kill();
  throw new Error(`PortalShield did not become healthy:\n${gatewayLogs}`);
}
