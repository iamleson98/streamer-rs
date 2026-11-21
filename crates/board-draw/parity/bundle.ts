// Bundles harness.ts, with the board drawing of the lesson-replay submodule, into one browser script.
//   bun bundle.ts OUT.js
// Two things are changed on the way in, none of them in the submodule's files:
//  - `.svelte.ts` modules (runes) go through the Svelte compiler;
//  - the maths and physics packs are stubs (the Rust side does not draw them yet).
// The board's own FONT_FAMILY is used as it is; render.mjs loads the fonts it names.
import { compileModule } from 'svelte/compiler';

const here = new URL('.', import.meta.url).pathname;
const [out] = Bun.argv.slice(2);
if (!out) throw new Error('usage: bun bundle.ts OUT.js');
const ts = new Bun.Transpiler({ loader: 'ts' });

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
				build.onResolve({ filter: /\/(math|physics)\/pack(\.js)?$/ }, (args) => ({
					path: args.path,
					namespace: 'pack-stub'
				}));
				build.onLoad({ filter: /.*/, namespace: 'pack-stub' }, () => ({
					contents: 'export const registerMath = () => {}; export const registerPhysics = () => {};',
					loader: 'js'
				}));
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
