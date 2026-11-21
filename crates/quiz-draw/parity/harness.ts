// The page side of the quiz parity check: the player's own screens, mounted and painted as the render
// does it (`export/quiz-dom.svelte.ts`: the screens in a clipped, out-of-view box, serialised into an
// SVG image and drawn on a canvas), for every sampled time of a scene. Next to each picture it dumps
// where the text and the boxes sit, in the fields quiz-draw's `layout_dump` writes.
import '../../../lesson-replay/src/app.css';
import { flushSync, mount, unmount } from 'svelte';
import QuizFrame from '../../../lesson-replay/src/lib/replay/export/quiz-frame.svelte';
import { replayStrings } from '../../../lesson-replay/src/lib/replay/i18n/strings.svelte';
import { createQuizDom, cutPartialRows } from '../../../lesson-replay/src/lib/replay/export/quiz-dom.svelte';
import { QuizStageSource } from '../../../lesson-replay/src/lib/replay/export/quiz-stage.svelte';
import { quizRecords } from '../../../lesson-replay/src/lib/replay/player/quiz-replay.svelte';

type Input = {
	records: { t: number; k: 'quiz'; q: unknown[] }[];
	locale: string;
	width: number;
	height: number;
	times: number[];
};
type Frame = { skip: true } | { skip: false; png: string; layout: unknown };

type Motion = {
	records: Input['records'];
	locale: string;
	width: number;
	height: number;
	/** Each sample: the time the state changed (the animation's start) and the time to draw. */
	samples: { at: number; t: number }[];
};

declare global {
	interface Window {
		/** Takes a screenshot of the stage (set by render.mjs); the file's name is the argument. */
		__shot: (name: string) => Promise<void>;
		/** Writes the layout dump of the frozen stage (set by render.mjs). */
		__dump: (name: string, json: string) => Promise<void>;
		renderMotion: (scene: Motion) => Promise<void>;
		/** Resizes the browser's viewport (set by render.mjs). */
		__viewport: (width: number, height: number) => Promise<void>;
		renderQuiz: (scene: Input) => Promise<Frame[]>;
	}
}

type Line = { color: number[]; text: string; x: number; y: number; w: number; h: number; clipped: boolean };
type BoxOut = { x: number; y: number; w: number; h: number; kind: string };

const round = (n: number) => Math.round(n * 100) / 100;

/** Whether the element is painted at all (not hidden, not the screen readers' text). */
function shown(el: Element): boolean {
	for (let e: Element | null = el; e; e = e.parentElement) {
		const s = getComputedStyle(e);
		if (s.display === 'none' || s.visibility === 'hidden' || s.opacity === '0') return false;
		if (s.position === 'absolute' && s.width === '1px' && s.height === '1px') return false;
	}
	return true;
}

const colours = new Map<string, number[]>();
/** A computed colour as Chromium paints it: sRGB bytes and an alpha (read through a canvas, as tokens.mjs does). */
function rgba(css: string): number[] {
	const hit = colours.get(css);
	if (hit) return hit;
	const m = css.match(/\/\s*([\d.]+)(%?)\s*\)$/);
	const alpha = m ? (m[2] ? Number(m[1]) / 100 : Number(m[1])) : 1;
	const opaque = m ? css.replace(/\s*\/\s*[\d.]+%?\s*\)$/, ')') : css;
	const ctx = document.createElement('canvas').getContext('2d', { willReadFrequently: true })!;
	ctx.canvas.width = ctx.canvas.height = 1;
	ctx.fillStyle = opaque;
	ctx.fillRect(0, 0, 1, 1);
	const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
	const out = [r, g, b, Math.round(alpha * 255)];
	colours.set(css, out);
	return out;
}

/** The lines of a text node as the browser laid them out: the boxes of its characters, grouped by line. */
function linesOf(node: Text, origin: DOMRect, clipped: boolean): Line[] {
	const color = rgba(getComputedStyle(node.parentElement!).color);
	const text = node.data;
	const range = document.createRange();
	const lines: Line[] = [];
	let i = 0;
	for (const ch of text) {
		const n = ch.length;
		range.setStart(node, i);
		range.setEnd(node, i + n);
		const r = range.getClientRects()[0];
		i += n;
		if (!r || (r.width === 0 && /\s/.test(ch))) {
			// A collapsed space at a line's end has no width: it extends the line's text only.
			const last = lines.at(-1);
			if (last && ch.trim() === '' && !last.text.endsWith(' ')) last.text += ' ';
			continue;
		}
		const y = r.top - origin.top;
		const last = lines.at(-1);
		if (last && Math.abs(last.y - y) < r.height / 2 && r.left - origin.left >= last.x - 0.01) {
			last.text += /\s/.test(ch) ? ' ' : ch;
			last.w = r.right - origin.left - last.x;
			last.h = Math.max(last.h, r.height);
		} else {
			lines.push({ color, text: /\s/.test(ch) ? ' ' : ch, x: r.left - origin.left, y, w: r.width, h: r.height, clipped });
		}
	}
	return lines
		.map((l) => ({ ...l, text: l.text.replace(/\s+$/, ''), x: round(l.x), y: round(l.y), w: round(l.w), h: round(l.h) }))
		.filter((l) => l.text !== '');
}

function dump(host: HTMLElement) {
	const frame = host.querySelector('[data-render-frame]')!;
	const origin = frame.getBoundingClientRect();
	const texts: Line[] = [];
	const boxes: BoxOut[] = [];
	const walker = document.createTreeWalker(frame, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
	for (let n = walker.nextNode(); n; n = walker.nextNode()) {
		if (n.nodeType === Node.TEXT_NODE) {
			const parent = n.parentElement!;
			if (!(n as Text).data.trim() || !shown(parent)) continue;
			const s = getComputedStyle(parent);
			const clipped = s.textOverflow === 'ellipsis' && parent.scrollWidth > parent.clientWidth;
			texts.push(...linesOf(n as Text, origin, clipped));
			continue;
		}
		const el = n as Element;
		if (!shown(el)) continue;
		const s = getComputedStyle(el);
		const r = el.getBoundingClientRect();
		const painted =
			el.tagName.toLowerCase() === 'svg' ||
			(s.backgroundColor !== 'rgba(0, 0, 0, 0)' && s.backgroundColor !== 'transparent') ||
			s.backgroundImage !== 'none' ||
			parseFloat(s.borderTopWidth) > 0 ||
			s.boxShadow !== 'none';
		if (painted && r.width > 0 && r.height > 0)
			boxes.push({
				kind: el.tagName.toLowerCase(),
				x: round(r.left - origin.left),
				y: round(r.top - origin.top),
				w: round(r.width),
				h: round(r.height)
			});
	}
	return { texts, boxes };
}

window.renderQuiz = async ({ records, locale, width, height, times }) => {
	await replayStrings.use(locale);
	// As the render page does before its first frame: every face the screens use is in.
	await document.fonts.load('700 20px "Inter Variable"', 'aạ');
	await document.fonts.load('400 20px "Inter Variable"', 'aạ');
	const source = new QuizStageSource(quizRecords(records));
	const dom = await createQuizDom(source, { x: 0, y: 0, w: width, h: height });
	const canvas = document.createElement('canvas');
	canvas.width = width;
	canvas.height = height;
	const ctx = canvas.getContext('2d')!;
	const host = document.querySelector('[data-render-frame]')!.parentElement as HTMLElement;
	const frames: Frame[] = [];
	try {
		for (const time of times) {
			dom.seek(time);
			if (!source.on) {
				frames.push({ skip: true });
				continue;
			}
			dom.begin();
			ctx.clearRect(0, 0, width, height);
			await dom.painter.draw(ctx);
			const png = canvas.toDataURL('image/png');
			// The picture was made in the page's tall viewport; the layout it is checked against is the one the
			// image has (the stage's own height, where the height-based media queries differ), what the cut hid
			// still hidden, and the host moved into view (the page skips what is off screen).
			await window.__viewport(width, height);
			const at = host.style.left;
			host.style.left = '0px';
			await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
			// A row the page skips off screen (`content-visibility: auto`) has the size of its placeholder there, and the
			// image lays it out whole: the dump is of the whole row.
			const whole = document.createElement('style');
			whole.textContent = '[data-render-frame] * { content-visibility: visible !important; }';
			document.head.append(whole);
			await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
			const layout = dump(host);
			whole.remove();
			host.style.left = at;
			await window.__viewport(1280, 720);
			frames.push({ skip: false, png, layout });
			await (window as unknown as { __probe?: (host: HTMLElement) => Promise<void> | void }).__probe?.(host);
		}
	} finally {
		dom.dispose();
		source.dispose();
	}
	return frames;
};

// ── motion ──
// The screens as they animate (the player's CSS without reduced motion): every sample mounts the stage, changes
// the game's state at `at` (which starts the animations), moves to `t` and freezes everything at `t - at`:
// CSS animations and transitions through `document.getAnimations()`, the script-driven ones (a count that
// runs up, rows that wait two frames) through a clock and a frame queue this file owns.
const fake = { now: 0, queue: [] as FrameRequestCallback[] };
const realRaf = window.requestAnimationFrame.bind(window);
const realNow = performance.now.bind(performance);
const settle = (ms: number) => {
	fake.now = ms;
	// Frames requested while running are run too (a row asks for a second one), up to a few.
	for (let i = 0; i < 4 && fake.queue.length; i++) {
		const run = fake.queue.splice(0);
		for (const cb of run) cb(ms);
	}
};

window.renderMotion = async ({ records, locale, width, height, samples }) => {
	await replayStrings.use(locale);
	await document.fonts.load('700 20px "Inter Variable"', 'aạ');
	const style = document.createElement('style');
	// The ring's and the tiles' bars' transitions follow the clock a few hundred ms behind it; the render's clock is
	// continuous, so the drawing is of the value itself. The rows' slides (700 ms, 650 ms) stay.
	style.textContent =
		'[data-render-frame] [class*="duration-300"], [data-render-frame] [class*="duration-500"] { transition: none !important; }';
	document.head.append(style);
	window.requestAnimationFrame = (cb) => (fake.queue.push(cb), fake.queue.length);
	performance.now = () => fake.now;
	try {
		for (const [i, { at, t }] of samples.entries()) {
			const source = new QuizStageSource(quizRecords(records));
			const host = document.createElement('div');
			host.setAttribute('style', `position:fixed;top:0;left:0;width:${width}px;height:${height}px;z-index:9`);
			document.body.append(host);
			fake.queue = [];
			fake.now = 0;
			const app = mount(QuizFrame, { target: host, props: { replay: source.replay } });
			source.seek(at - 1);
			flushSync();
			settle(0);
			flushSync();
			// What came before the change has long finished: its animations and counts are at their ends.
			for (const a of document.getAnimations()) {
				try {
					a.finish();
				} catch {
					// an animation that never ends
				}
			}
			settle(100_000);
			flushSync();
			const before = new Set(document.getAnimations());
			const base = fake.now;
			source.seek(at);
			flushSync();
			getComputedStyle(host).transform;
			settle(base);
			flushSync();
			host.getBoundingClientRect();
			const started = new Set(document.getAnimations().filter((a) => !before.has(a)));
			source.seek(t);
			flushSync();
			host.getBoundingClientRect();
			settle(base + (t - at));
			flushSync();
			host.getBoundingClientRect();
			for (const a of document.getAnimations()) {
				a.pause();
				if (started.has(a)) a.currentTime = t - at;
				else if (!before.has(a)) a.currentTime = 0;
			}
			host.getBoundingClientRect();
			// What the render's cut leaves out is decided in the page's tall viewport, the picture is the stage's own.
			// The render's host is out of view, where the page skips what is off screen: rows have their placeholder size.
			await window.__viewport(1280, 720);
			host.style.left = '-10000px';
			await new Promise((resolve) => realRaf(() => realRaf(resolve)));
			const uncut = cutPartialRows(host);
			host.style.left = '0px';
			await window.__viewport(width, height);
			await new Promise((resolve) => realRaf(() => realRaf(resolve)));
			const whole = document.createElement('style');
			whole.textContent = '[data-render-frame] * { content-visibility: visible !important; }';
			document.head.append(whole);
			await window.__dump(`${i}`, JSON.stringify(dump(host)));
			whole.remove();
			await window.__shot(`${i}`);
			uncut();
			void unmount(app);
			host.remove();
			source.dispose();
		}
	} finally {
		window.requestAnimationFrame = realRaf;
		performance.now = realNow;
		style.remove();
	}
};
