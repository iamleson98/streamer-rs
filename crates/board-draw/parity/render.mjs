// Draws every sampled time of the corpus with the bundled web player code in headless Chromium.
//   node render.mjs CORPUS_DIR BUNDLE.js OUT_DIR FONTS_DIR
// Writes NAME-TIME.ts.png and NAME-TIME.ts.json next to the Rust side's files.
import { readFileSync, writeFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { chromium } from 'playwright-core';

const [corpus, bundle, out, fonts] = process.argv.slice(2);
if (!fonts) throw new Error('usage: node render.mjs CORPUS_DIR BUNDLE.js OUT_DIR FONTS_DIR');
const index = JSON.parse(readFileSync(`${corpus}/index.json`, 'utf8'));

// The @font-face rules of the player's app.css, as they are, with the font files read from disk.
const css = [...readFileSync(`${fonts}/../../src/app.css`, 'utf8').matchAll(/@font-face\s*\{[^}]*\}/g)]
	.map((m) => m[0].replaceAll("url('/fonts/", `url('file://${fonts}/`))
	.join('\n');
writeFileSync(`${out}/page.html`, `<!doctype html><meta charset="utf-8"><style>${css}</style><body></body>`);

// Text is laid out with fractional advances and no hinting, as on a high-density screen: that is
// what the Rust side does on every machine. Chromium on Linux at density 1 would round every
// glyph's advance to whole pixels, which a text layout engine cannot be asked to copy.
// As root (a container) the browser's own sandbox cannot start.
const args = ['--font-render-hinting=none', '--enable-font-subpixel-positioning'];
if (process.env.PARITY_NO_SANDBOX) args.push('--no-sandbox');
const browser = await chromium.launch({ args });
const playwright = JSON.parse(readFileSync(new URL('./node_modules/playwright-core/package.json', import.meta.url), 'utf8'));
writeFileSync(`${out}/browser.txt`, `Chromium ${browser.version()} (headless shell, playwright-core ${playwright.version})`);
try {
	const page = await browser.newPage({ viewport: { width: 1300, height: 700 }, deviceScaleFactor: 1 });
	await page.goto(`file://${out}/page.html`);
	// Every face must be in before anything is measured (the board redraws when one arrives; a
	// frame is taken once).
	await page.evaluate(() =>
		Promise.all(
			[
				['Inter Variable', 'a'],
				['Inter Variable', 'ạ'],
				['Inter Variable', 'α'],
				['Noto Sans', 'ᴬ'],
				['Noto Sans Math', '∑'],
				['Noto Sans Symbols 2', '★']
			].map(([family, sample]) => document.fonts.load(`20px "${family}"`, sample))
		)
	);
	await page.addScriptTag({ content: readFileSync(bundle, 'utf8') });
	const pictures = Object.fromEntries(
		Object.entries(index.pictures).map(([key, file]) => [
			key,
			'data:image/png;base64,' + readFileSync(`${corpus}/${file}`).toString('base64')
		])
	);
	const logs = new Map();
	for (const scene of index.scenes) {
		if (!logs.has(scene.log)) logs.set(scene.log, gunzipSync(readFileSync(`${corpus}/${scene.log}.ndjson.gz`)).toString('utf8'));
		const frames = await page.evaluate((input) => window.renderFrames(input), {
			log: logs.get(scene.log),
			times: scene.times,
			width: scene.width,
			height: scene.height,
			pictures,
			view: scene.view ?? null,
			loading: scene.loading === true,
			fresh: scene.fresh === true
		});
		frames.forEach((frame, i) => {
			const base = `${out}/${scene.name}-${scene.times[i]}.ts`;
			writeFileSync(`${base}.png`, Buffer.from(frame.png.split(',')[1], 'base64'));
			writeFileSync(`${base}.json`, JSON.stringify(frame.layout, null, 2));
		});
	}
} finally {
	await browser.close();
}
