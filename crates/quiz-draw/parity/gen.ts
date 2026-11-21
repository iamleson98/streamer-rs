// Makes the quiz corpus from the web player's own code (the reference the Rust port is held to):
//   bun gen.ts OUT_DIR            write the corpus
//   bun gen.ts --check OUT_DIR    fail if the files in OUT_DIR differ from what the player gives
// OUT_DIR is `crates/quiz-draw/tests/corpus`. It holds, for every scene of `scenes.ts`, the board log
// (NDJSON, gzip) and `expected/NAME.json`: the state of the game at each time as the player's
// reducers give it. `index.json` lists the scenes; `strings.json` the player's quiz strings.
import { gzipSync } from 'node:zlib';
import { svelteModules } from './svelte-modules';

const here = new URL('.', import.meta.url).pathname;
const [first, second] = Bun.argv.slice(2);
const check = first === '--check';
const out = check ? second : first;
if (!out) throw new Error('usage: bun gen.ts [--check] OUT_DIR');

const built = await Bun.build({
	entrypoints: [`${here}gen-run.ts`],
	target: 'bun',
	plugins: [svelteModules],
	outdir: `${process.env.TMPDIR ?? '/tmp'}/quiz-gen`,
	naming: 'gen-run.mjs'
});
if (!built.success) {
	for (const log of built.logs) console.error(log);
	process.exit(1);
}
const { files } = (await import(built.outputs[0].path)) as {
	files: () => Promise<Map<string, Uint8Array | string>>;
};

const made = await files();
let differs = 0;
for (const [name, content] of made) {
	const path = `${out}/${name}`;
	if (check) {
		const have = (await Bun.file(path).exists()) ? Buffer.from(await Bun.file(path).bytes()) : null;
		// A gzip file is compared by what it unzips to (the compressor's bytes are not the point).
		const same = (a: Buffer | null, b: Uint8Array | string) =>
			a !== null &&
			(name.endsWith('.gz')
				? Buffer.compare(Bun.gunzipSync(a), Bun.gunzipSync(b as Uint8Array)) === 0
				: Buffer.compare(a, Buffer.from(b)) === 0);
		if (!same(have, content)) {
			console.error(`differs: ${name}`);
			differs++;
		}
	} else {
		await Bun.write(path, content);
	}
}
if (differs) process.exit(1);
console.log(`${check ? 'checked' : 'wrote'} ${made.size} files`);
void gzipSync;
