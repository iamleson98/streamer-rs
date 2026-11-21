// The page side of the parity check: replays a board log with the web player's own code and draws
// it as BoardPainter.paint does (export/painters.ts), then describes the layout with the same
// fields as board-draw's `layout_dump`.
import {
	elementBounds,
	kindOf,
	textLines,
	LINE_HEIGHT,
	type BoardElement
} from '../../../lesson-replay/src/lib/replay/board/elements';
import { registerMath } from '../../../lesson-replay/src/lib/replay/board/math/pack';
import { registerPhysics } from '../../../lesson-replay/src/lib/replay/board/physics/pack';
import { PictureCache } from '../../../lesson-replay/src/lib/replay/board/pictures';
import { BoardRenderer, createMeasure, FONT_FAMILY, loadBoardFonts } from '../../../lesson-replay/src/lib/replay/board/render';
import { BoardReplay, parseBoardLog } from '../../../lesson-replay/src/lib/replay/player/board-replay.svelte';
import { boardViewport } from '../../../lesson-replay/src/lib/replay/player/board-viewport';

type Frame = { png: string; layout: unknown };

/** One scene of the corpus, as `render.mjs` hands it over. */
type SceneInput = {
	log: string;
	times: number[];
	width: number;
	height: number;
	/** Deck pictures by `deck/page`, as data URLs; any other page fails to load. */
	pictures: Record<string, string>;
	/** The camera, instead of the one the board asks for. */
	view: [number, number, number] | null;
	/** Sheets' pictures never arrive. */
	loading: boolean;
	/** Every frame from a fresh replay and renderer. */
	fresh: boolean;
};

declare global {
	interface Window {
		renderFrames: (scene: SceneInput) => Promise<Frame[]>;
	}
}

const toDataUrl = (blob: Blob) =>
	new Promise<string>((resolve) => {
		const reader = new FileReader();
		reader.onload = () => resolve(reader.result as string);
		reader.readAsDataURL(blob);
	});

// Graphs, solids and symbols read and draw only once their pack has registered them: the web player
// loads a pack when an element of its types shows up (`loadPack`, which calls these), the harness up front.
registerMath();
registerPhysics();

window.renderFrames = async ({ log, times, width, height, pictures: urls, view, loading, fresh }) => {
	// As the render page does before its first frame: every face of the stack, not only the ones a
	// sample character asked for (a face that arrives after text was measured leaves stale bounds).
	await loadBoardFonts();
	const records = parseBoardLog(log);
	const canvas = new OffscreenCanvas(width, height);
	const ctx = canvas.getContext('2d') as unknown as CanvasRenderingContext2D;
	const target = new OffscreenCanvas(width, height);
	const tctx = target.getContext('2d')!;
	const measure = createMeasure(new OffscreenCanvas(8, 8).getContext('2d') as unknown as CanvasRenderingContext2D);
	const load = async (url: string) => {
		if (loading) return new Promise<never>(() => undefined);
		if (!(url in urls)) throw new Error('no such page');
		const image = new Image();
		image.src = urls[url];
		await image.decode();
		return image;
	};
	const session = () => {
		const pictures = new PictureCache({ url: (d, n) => `${d}/${n}`, load, onChange: () => undefined });
		return { replay: new BoardReplay(records), pictures, renderer: new BoardRenderer(pictures) };
	};
	let current = session();

	// A scratch context a pack element is drawn on once to find its text: the fills of text are
	// written down with the box `measureText` and the text's alignment and baseline give (the em box,
	// widened for an italic's slant), in board units at the camera's scale.
	const scratch = new OffscreenCanvas(8, 8).getContext('2d') as unknown as CanvasRenderingContext2D;
	const inkText = (el: BoardElement, zoom: number) => {
		const boxes: { text: string; bounds: number[] }[] = [];
		const recorder = new Proxy(scratch, {
			get(target, key) {
				if (key === 'fillText') {
					return (text: string, x: number, y: number) => {
						if (text === '' || !Number.isFinite(x) || !Number.isFinite(y)) return;
						const m = target.measureText(text);
						const size = parseFloat(/([\d.]+)px/.exec(target.font)![1]);
						const left = x - { left: 0, start: 0, center: 0.5, right: 1, end: 1 }[target.textAlign] * m.width;
						// The em box: the font's ascent and descent scaled to make up one em.
						const fontAsc = m.fontBoundingBoxAscent;
						const fontDesc = m.fontBoundingBoxDescent;
						const asc = (size * fontAsc) / (fontAsc + fontDesc);
						const desc = (size * fontDesc) / (fontAsc + fontDesc);
						const baseline =
							y +
							({ top: asc, hanging: asc, middle: (asc - desc) / 2, bottom: -desc, ideographic: -desc, alphabetic: 0 } as Record<string, number>)[target.textBaseline];
						const slant = target.font.startsWith('italic') ? size * 0.25 : 0;
						boxes.push({ text, bounds: [left, baseline - asc, left + m.width + slant, baseline + desc] });
					};
				}
				const value = Reflect.get(target, key);
				return typeof value === 'function' ? value.bind(target) : value;
			},
			set(target, key, value) {
				return Reflect.set(target, key, value);
			}
		});
		scratch.setTransform(zoom, 0, 0, zoom, 0, 0);
		kindOf(el.type)!.draw(recorder, el, {
			ink: () => '#000000',
			muted: '#475569',
			font: FONT_FAMILY,
			dark: false
		});
		return boxes;
	};

	const describe = (id: string, el: BoardElement, zoom: number) => {
		const b = elementBounds(el, measure);
		const d: Record<string, unknown> = { id, type: el.type, bounds: [b.minX, b.minY, b.maxX, b.maxY] };
		if (el.type === 'text') {
			d.lines = textLines(el).map((line) => ({ text: line, width: measure(line, el.size) }));
			d.line_height = el.size * LINE_HEIGHT;
		}
		if (el.type === 'graph' || el.type === 'solid' || el.type === 'symbol') d.ink_text = inkText(el, zoom);
		return d;
	};

	const frames: Frame[] = [];
	for (const t of times) {
		if (fresh) current = session();
		const { replay, pictures, renderer } = current;
		replay.seek(t);
		const { scene } = replay;
		const overlays: BoardElement[] = [...replay.remote.values()].flatMap((p) => (p.preview ? [p.preview] : []));
		const viewport = view ? { x: view[0], y: view[1], zoom: view[2] } : boardViewport(scene, measure, width, height);
		const draw = () =>
			renderer.draw(ctx, {
				elements: scene.ordered(),
				revision: scene.revision,
				overlays,
				hidden: [],
				selected: null,
				viewport,
				width,
				height,
				dpr: 1,
				theme: 'light',
				measure
			});
		draw();
		if (!loading && (await pictures.settled(5000))) draw();
		tctx.setTransform(1, 0, 0, 1, 0, 0);
		tctx.fillStyle = '#ffffff';
		tctx.fillRect(0, 0, width, height);
		tctx.drawImage(canvas, 0, 0);
		const png = await toDataUrl(await target.convertToBlob({ type: 'image/png' }));
		const layout = {
			width,
			height,
			viewport: { x: viewport.x, y: viewport.y, zoom: viewport.zoom },
			elements: scene.ordered().map(({ id, el }) => describe(id, el, viewport.zoom)),
			overlays: [...replay.remote.entries()].flatMap(([u, p]) => (p.preview ? [describe(u, p.preview, viewport.zoom)] : []))
		};
		frames.push({ png, layout });
	}
	return frames;
};
