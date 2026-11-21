// Builds harness.ts, with the player's quiz screens (the Svelte components of lesson-replay), into one
// browser script and one style sheet. The compiler, Vite, Tailwind and the Svelte plugin are the ones
// lesson-replay pins (its `node_modules`); the player's own `app.css` is imported, so the screens
// carry the same CSS as the render app's.
//   bun ../../../lesson-replay/node_modules/vite/bin/vite.js build --config vite.config.mjs
//   (OUT_DIR is where harness.js and harness.css go)
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const lesson = path.resolve(here, '../../../lesson-replay');
const require = createRequire(path.join(lesson, 'package.json'));
const load = async (name) => import(pathToFileURL(require.resolve(name)).href);
const { svelte } = await load('@sveltejs/vite-plugin-svelte');
const tailwindcss = (await load('@tailwindcss/vite')).default;

export default {
	root: lesson,
	logLevel: 'warn',
	resolve: { dedupe: ['@lucide/svelte', 'bits-ui', 'cn', 'tailwind-variants'] },
	tsconfig: path.join(lesson, 'tsconfig.json'),
	plugins: [
		tailwindcss(),
		svelte({
			compilerOptions: {
				runes: ({ filename }) => (filename.split(/[/\\]/).includes('node_modules') ? undefined : true)
			}
		})
	],
	build: {
		outDir: process.env.OUT_DIR,
		emptyOutDir: false,
		minify: false,
		lib: {
			entry: path.join(here, 'harness.ts'),
			formats: ['iife'],
			name: 'QuizHarness',
			fileName: () => 'harness.js',
			cssFileName: 'harness'
		}
	}
};
