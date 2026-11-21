// Bundles harness.ts, with the board drawing of the lesson-replay submodule, into one browser script.
//   bun bundle.ts OUT.js
// Three things are changed on the way in, none of them in the submodule's files:
//  - `.svelte.ts` modules (runes) go through the Svelte compiler;
//  - the physics pack is a stub (the Rust side does not draw it yet; the maths pack is the real one);
//  - the font stack of board/render.ts is extended with the fallback families (below).
// The board's FONT_FAMILY gets the fallback families ("Fallback 0" ... in the order of
// fallback-fonts.json) before its `sans-serif`, so that Chromium draws a character the board's fonts
// lack from the same files as the Rust side; render.mjs defines them (@font-face) and loads them.
import { compileModule } from 'svelte/compiler';

const here = new URL('.', import.meta.url).pathname;
const [out] = Bun.argv.slice(2);
if (!out) throw new Error('usage: bun bundle.ts OUT.js');
const ts = new Bun.Transpiler({ loader: 'ts' });
const fallbacks: unknown[] = JSON.parse(await Bun.file(`${here}fallback-fonts.json`).text());
const STACK = '"Inter Variable", "Noto Sans", "Noto Sans Math", "Noto Sans Symbols 2", sans-serif';

const svelteEntry = (spec: string): string => {
	const exports = JSON.parse(
		require('node:fs').readFileSync(`${here}node_modules/svelte/package.json`, 'utf8')
	).exports;
	const entry = exports[spec === 'svelte' ? '.' : `.${spec.slice('svelte'.length)}`];
	const file = entry?.browser ?? entry?.default;
	if (!file) throw new Error(`svelte has no export ${spec}`);
	return `${here}node_modules/svelte/${file.slice(2)}`;
};

const result = await Bun.build({
	entrypoints: [`${here}harness.ts`],
	format: 'iife',
	target: 'browser',
	plugins: [
		{
			name: 'parity',
			setup(build) {
				build.onResolve({ filter: /^svelte(\/.*)?$/ }, (args) => ({ path: svelteEntry(args.path) }));
				build.onResolve({ filter: /^perfect-freehand$/ }, () => ({
					path: `${here}node_modules/perfect-freehand/dist/esm/index.mjs`
				}));
				build.onResolve({ filter: /\/physics\/pack(\.js)?$/ }, (args) => ({
					path: args.path,
					namespace: 'pack-stub'
				}));
				build.onLoad({ filter: /.*/, namespace: 'pack-stub' }, () => ({
					contents: 'export const registerPhysics = () => {};',
					loader: 'js'
				}));
				build.onLoad({ filter: /\/replay\/board\/render\.ts$/ }, async (args) => {
					const source = await Bun.file(args.path).text();
					if (!source.includes(STACK)) throw new Error('the board font stack changed: update bundle.ts');
					const extended = fallbacks.map((_, i) => `"Fallback ${i}"`).join(', ');
					return {
						contents: ts.transformSync(source.replace(STACK, STACK.replace(', sans-serif', `, ${extended}, sans-serif`))),
						loader: 'js'
					};
				});
				build.onLoad({ filter: /\.svelte\.ts$/ }, async (args) => {
					const source = ts.transformSync(await Bun.file(args.path).text());
					const compiled = compileModule(source, { generate: 'client', filename: args.path });
					return { contents: compiled.js.code, loader: 'js' };
				});
			}
		}
	]
});
if (!result.success) {
	for (const log of result.logs) console.error(log);
	process.exit(1);
}
await Bun.write(out, await result.outputs[0].text());
