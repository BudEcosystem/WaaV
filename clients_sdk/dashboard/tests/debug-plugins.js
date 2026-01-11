/**
 * Debug script to check Plugin Explorer panel
 */

import puppeteer from 'puppeteer';

const DASHBOARD_URL = 'http://localhost:8080';

async function debug() {
  console.log('Debugging Plugin Explorer...\n');

  const browser = await puppeteer.launch({
    headless: false,
    defaultViewport: { width: 1400, height: 900 },
    args: ['--no-sandbox']
  });

  const page = await browser.newPage();

  // Listen to console
  page.on('console', msg => console.log(`[Browser] ${msg.text()}`));
  page.on('pageerror', err => console.log(`[Page Error] ${err.message}`));

  await page.goto(DASHBOARD_URL, { waitUntil: 'networkidle0' });
  console.log('Dashboard loaded');

  // Check what nav links exist
  const navLinks = await page.$$eval('.nav-link[data-tab]', links =>
    links.map(l => ({ tab: l.dataset.tab, text: l.textContent.trim() }))
  );
  console.log('\nNav links found:', navLinks);

  // Check if plugins link exists
  const pluginsLink = await page.$('a[data-tab="plugins"]');
  console.log('\nPlugins link exists:', !!pluginsLink);

  // Check all panel IDs
  const panels = await page.$$eval('.tab-panel', panels =>
    panels.map(p => ({ id: p.id, active: p.classList.contains('active') }))
  );
  console.log('\nPanels found:', panels);

  // Try clicking plugins
  if (pluginsLink) {
    console.log('\nClicking plugins link...');
    await pluginsLink.click();
    await new Promise(r => setTimeout(r, 2000));

    // Check panels again
    const panelsAfter = await page.$$eval('.tab-panel', panels =>
      panels.map(p => ({ id: p.id, active: p.classList.contains('active'), display: getComputedStyle(p).display }))
    );
    console.log('Panels after click:', panelsAfter);

    // Check if plugins-refresh button exists
    const refreshBtn = await page.$('#plugins-refresh');
    console.log('\nRefresh button exists:', !!refreshBtn);

    if (refreshBtn) {
      // Try clicking refresh
      console.log('Clicking refresh button...');
      await refreshBtn.click();
      await new Promise(r => setTimeout(r, 3000));

      // Check for plugin cards
      const cards = await page.$$('.plugin-card');
      console.log('Plugin cards found:', cards.length);

      // Check counts
      const sttCount = await page.$eval('#plugins-stt-count', el => el.textContent).catch(() => 'N/A');
      const ttsCount = await page.$eval('#plugins-tts-count', el => el.textContent).catch(() => 'N/A');
      console.log(`STT count: ${sttCount}, TTS count: ${ttsCount}`);
    }
  }

  // Screenshot
  await page.screenshot({ path: '/tmp/debug-plugins.png' });
  console.log('\nScreenshot saved to /tmp/debug-plugins.png');

  console.log('\nKeeping browser open for 10 seconds...');
  await new Promise(r => setTimeout(r, 10000));

  await browser.close();
}

debug().catch(console.error);
