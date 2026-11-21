// The page side of the parity check: replays a board log with the web player's own code and draws
// it as BoardPainter.paint does (export/painters.ts), then describes the layout with the same
// fields as board-draw's `layout_dump`.
import {
	elementBounds,
	textLines,
	LINE_HEIGHT,
	type BoardElement
} from '../../../lesson-replay/src/lib/replay/board/elements';
import { PictureCache } from '../../../lesson-replay/src/lib/replay/board/pictures';
import { BoardRenderer, createMeasure } from '../../../lesson-replay/src/lib/replay/board/render';
import { BoardReplay, parseBoardLog } from '../../../lesson-replay/src/lib/replay/player/board-replay.svelte';
import { boardViewport } from '../../../lesson-replay/src/lib/replay/player/board-viewport';

type Frame = { png: string; layout: unknown };

declare global {
	interface Window {
		renderFrames: (
			log: string,
			times: number[],
			width: number,
			height: number,
			deck: { id: string; url: string }
		) => Promise<Frame[]>;
	}
}

const toDataUrl = (blob: Blob) =>
	new Promise<string>((resolve) => {
		const reader = new FileReader();
		reader.onload = () => resolve(reader.result as string);
		reader.readAsDataURL(blob);
	});

window.renderFrames = async (log, times, width, height, deck) => {
	const replay = new BoardReplay(parseBoardLog(log));
	const canvas = new OffscreenCanvas(width, height);
	const ctx = canvas.getContext('2d') as unknown as CanvasRenderingContext2D;
	const target = new OffscreenCanvas(width, height);
	const tctx = target.getContext('2d')!;
	const measure = createMeasure(new OffscreenCanvas(8, 8).getContext('2d') as unknown as CanvasRenderingContext2D);
	// Only page 1 of the deck exists; asking for any other fails, as a missing page does.
	const load = async (url: string) => {
		if (url !== `${deck.id}/1`) throw new Error('no such page');
		const image = new Image();
		image.src = deck.url;
		await image.decode();
		return image;
	};
	const pictures = new PictureCache({ url: (d, n) => `${d}/${n}`, load, onChange: () => undefined });
	const renderer = new BoardRenderer(pictures);

	const describe = (id: string, el: BoardElement) => {
		const b = elementBounds(el, measure);
		const d: Record<string, unknown> = { id, type: el.type, bounds: [b.minX, b.minY, b.maxX, b.maxY] };
		if (el.type === 'text') {
			d.lines = textLines(el).map((line) => ({ text: line, width: measure(line, el.size) }));
			d.line_height = el.size * LINE_HEIGHT;
		}
		return d;
	};

	const frames: Frame[] = [];
	for (const t of times) {
		replay.seek(t);
		const { scene } = replay;
		const overlays: BoardElement[] = [...replay.remote.values()].flatMap((p) => (p.preview ? [p.preview] : []));
		const viewport = boardViewport(scene, measure, width, height);
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
		if (await pictures.settled(5000)) draw();
		tctx.setTransform(1, 0, 0, 1, 0, 0);
		tctx.fillStyle = '#ffffff';
		tctx.fillRect(0, 0, width, height);
		tctx.drawImage(canvas, 0, 0);
		const png = await toDataUrl(await target.convertToBlob({ type: 'image/png' }));
		const layout = {
			width,
			height,
			viewport: { x: viewport.x, y: viewport.y, zoom: viewport.zoom },
			elements: scene.ordered().map(({ id, el }) => describe(id, el)),
			overlays: [...replay.remote.entries()].flatMap(([u, p]) => (p.preview ? [describe(u, p.preview)] : []))
		};
		frames.push({ png, layout });
	}
	return frames;
};
