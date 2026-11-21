// Draws every sampled time of the quiz corpus with the player's own screens in headless Chromium.
//   node render.mjs CORPUS_DIR BUNDLE_DIR OUT_DIR FONTS_DIR FALLBACK_FONTS_DIR
// BUNDLE_DIR holds harness.js and harness.css (vite.config.mjs). Writes NAME-TIME.ts.png and
// NAME-TIME.ts.json (the layout dump) into OUT_DIR, next to the Rust side's files, .
//
// The page is served over HTTP because the painter reads the font files it embeds with `fetch`.
import { createServer } from 'node:http';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { chromium } from 'playwright-core';

const [corpus, bundle, out, fonts, fallbackDir] = process.argv.slice(2);
if (!fallbackDir) throw new Error('usage: node render.mjs CORPUS_DIR BUNDLE_DIR OUT_DIR FONTS_DIR FALLBACK_FONTS_DIR');
const index = JSON.parse(readFileSync(`${corpus}/index.json`, 'utf8'));
const fallbacks = JSON.parse(readFileSync(new URL('../../board-draw/parity/fallback-fonts.json', import.meta.url), 'utf8'));

// A face of a font collection as a font file of its own (a browser opens the first face of a
// collection only), as `single_face` in board-draw's skia.rs does.
function singleFace(bytes, index) {
	if (bytes.toString('latin1', 0, 4) !== 'ttcf') return bytes;
	const dir = bytes.readUInt32BE(12 + 4 * index);
	const count = bytes.readUInt16BE(dir + 4);
	const parts = [Buffer.from(bytes.subarray(dir, dir + 12))];
	const body = [];
	let offset = 12 + 16 * count;
	for (let i = 0; i < count; i++) {
		const record = Buffer.from(bytes.subarray(dir + 12 + 16 * i, dir + 28 + 16 * i));
		const at = record.readUInt32BE(8);
		const length = record.readUInt32BE(12);
		const padded = (length + 3) & ~3;
		record.writeUInt32BE(offset, 8);
		parts.push(record);
		const table = Buffer.alloc(padded);
		bytes.copy(table, 0, at, at + length);
		body.push(table);
		offset += padded;
	}
	return Buffer.concat([...parts, ...body]);
}

mkdirSync(`${out}/fallback-fonts`, { recursive: true });
const files = new Map();
files.set('/harness.js', [readFileSync(`${bundle}/harness.js`), 'text/javascript']);
files.set('/harness.css', [readFileSync(`${bundle}/harness.css`), 'text/css']);
const faces = fallbacks.map(({ file, index }, i) => {
	const bytes = singleFace(readFileSync(`${fallbackDir}/${file}`), index ?? 0);
	files.set(`/fallback/${i}.ttf`, [bytes, 'font/ttf']);
	return `@font-face{font-family:"Fallback ${i}";src:url('/fallback/${i}.ttf');}`;
});
const stack = ['"Inter Variable"', ...fallbacks.map((_, i) => `"Fallback ${i}"`), 'sans-serif'].join(', ');
// The fonts behind the player's own: the video has no system fonts either (see board-draw's README).
const page = `<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="/harness.css"><style>${faces.join('\n')}
html, body { font-family: ${stack}; }</style><body></body><script src="/harness.js"></script>`;
files.set('/', [Buffer.from(page), 'text/html']);

const server = createServer((req, res) => {
	const url = new URL(req.url, 'http://x').pathname;
	if (url.startsWith('/fonts/')) {
		try {
			res.setHeader('content-type', 'font/woff2');
			res.end(readFileSync(`${fonts}/${url.slice('/fonts/'.length).replace(/[^\w.-]/g, '')}`));
		} catch {
			res.statusCode = 404;
			res.end();
		}
		return;
	}
	const hit = files.get(url);
	if (!hit) {
		res.statusCode = 404;
		res.end();
		return;
	}
	res.setHeader('content-type', hit[1]);
	res.end(hit[0]);
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

// Text is laid out with fractional advances and no hinting (see board-draw's README).
const args = ['--font-render-hinting=none', '--enable-font-subpixel-positioning'];
if (process.env.PARITY_NO_SANDBOX) args.push('--no-sandbox');
const browser = await chromium.launch({ args });
const playwright = JSON.parse(readFileSync(new URL('./node_modules/playwright-core/package.json', import.meta.url), 'utf8'));
writeFileSync(`${out}/browser.txt`, `Chromium ${browser.version()} (headless shell, playwright-core ${playwright.version})`);
try {
	// The render browser emulates reduced motion; the screens depend on it.
	const context = await browser.newContext({ viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1, reducedMotion: 'reduce' });
	const page = await context.newPage();
	if (process.env.PROBE) await page.addInitScript(`window.__probe = ${process.env.PROBE}`);
	await page.exposeFunction('__viewport', async (width, height) => page.setViewportSize({ width, height }));
	page.on('pageerror', (e) => console.error('page error:', e.message));
	page.on('console', (m) => console.log(m.text()));
	await page.goto(`${origin}/`);
	await page.evaluate((count) => Promise.all(Array.from({ length: count }, (_, i) => document.fonts.load(`20px "Fallback ${i}"`))), fallbacks.length);
	for (const scene of index.scenes.filter((s) => !process.env.ONLY || s.name === process.env.ONLY)) {
		const text = gunzipSync(readFileSync(`${corpus}/${scene.log}.ndjson.gz`)).toString('utf8');
		const records = text.split('\n').filter(Boolean).map((l) => JSON.parse(l));
		const input = { records, locale: scene.locale, width: scene.width, height: scene.height, times: scene.times };
		let frames;
		if (process.env.LIVE_SHOT) {
			// Debugging: the mounted DOM itself (not the painted image) is moved into view and shot at every time.
			await page.evaluate(() => {
				window.__probe = (host) => {
					host.style.left = '0';
					window.__ready = true;
					return new Promise((resolve) => (window.__go = resolve));
				};
			});
			const run = page.evaluate((i) => window.renderQuiz(i), input);
			for (const time of scene.times) {
				await page.waitForFunction(() => window.__ready);
				await page.screenshot({ path: `${out}/live-${scene.name}-${time}.png`, clip: { x: 0, y: 0, width: scene.width, height: scene.height } });
				await page.evaluate(() => {
					window.__ready = false;
					window.__go();
				});
			}
			frames = await run;
		} else {
			frames = await page.evaluate((i) => window.renderQuiz(i), input);
		}
		frames.forEach((frame, i) => {
			if (frame.skip) {
				return;
			}
			const base = `${out}/${scene.name}-${scene.times[i]}.ts`;
			writeFileSync(`${base}.png`, Buffer.from(frame.png.split(',')[1], 'base64'));
			writeFileSync(`${base}.json`, JSON.stringify(frame.layout, null, 1));
		});
	}
	} finally {
	await browser.close();
	server.close();
}
