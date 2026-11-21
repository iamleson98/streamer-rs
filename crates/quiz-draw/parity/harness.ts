// The page side of the quiz parity check: the player's own screens, mounted and painted as the render
// does it (`export/quiz-dom.svelte.ts`: the screens in a clipped, out-of-view box, serialised into an
// SVG image and drawn on a canvas), for every sampled time of a scene. Next to each picture it dumps
// where the text and the boxes sit, in the fields quiz-draw's `layout_dump` writes.
import '../../../lesson-replay/src/app.css';
import { replayStrings } from '../../../lesson-replay/src/lib/replay/i18n/strings.svelte';
import { createQuizDom } from '../../../lesson-replay/src/lib/replay/export/quiz-dom.svelte';
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

declare global {
	interface Window {
		/** Resizes the browser's viewport (set by render.mjs). */
		__viewport: (width: number, height: number) => Promise<void>;
		renderQuiz: (scene: Input) => Promise<Frame[]>;
	}
}

type Line = { text: string; x: number; y: number; w: number; h: number; clipped: boolean };
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

/** The lines of a text node as the browser laid them out: the boxes of its characters, grouped by line. */
function linesOf(node: Text, origin: DOMRect, clipped: boolean): Line[] {
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
			lines.push({ text: /\s/.test(ch) ? ' ' : ch, x: r.left - origin.left, y, w: r.width, h: r.height, clipped });
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
