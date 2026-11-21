// A Bun bundler plugin that makes the player's code runnable outside Svelte's own tooling: its
// `.svelte.ts` modules (runes) go through the Svelte compiler. The compiler and the runtime are
// the ones lesson-replay pins (its `node_modules`, from `bun install --frozen-lockfile`).
import { createRequire } from 'node:module';

const lesson = new URL('../../../lesson-replay/package.json', import.meta.url).pathname;
const { compileModule } = (await import(createRequire(lesson).resolve('svelte/compiler'))) as typeof import('svelte/compiler');

const ts = new Bun.Transpiler({ loader: 'ts' });

export const svelteModules: Bun.BunPlugin = {
	name: 'svelte-modules',
	setup(build) {
		build.onLoad({ filter: /\.svelte\.ts$/ }, async (args) => {
			const source = ts.transformSync(await Bun.file(args.path).text());
			return {
				contents: compileModule(source, { generate: 'client', filename: args.path }).js.code,
				loader: 'js'
			};
		});
	}
};
