// The part of gen.ts that runs the player's code (bundled with the Svelte compiler by gen.ts).
import { gzipSync } from 'node:zlib';
import { QuizReplay, quizRecords } from '../../../lesson-replay/src/lib/replay/player/quiz-replay.svelte';
import { QuestReplay } from '../../../lesson-replay/src/lib/replay/player/quest-replay.svelte';
import { replayStrings } from '../../../lesson-replay/src/lib/replay/i18n/strings.svelte';
import { en } from '../../../lesson-replay/src/lib/replay/i18n/en';
import { vi } from '../../../lesson-replay/src/lib/replay/i18n/vi';
import { avatarOf } from '../../../lesson-replay/src/lib/replay/quiz/avatar';
import { fallbackCharacter } from '../../../lesson-replay/src/lib/replay/quiz/quest/characters';
import { accuracy, clock } from '../../../lesson-replay/src/lib/replay/quiz/quest/format';
import { questionSize, optionSize, tileColumns } from '../../../lesson-replay/src/lib/replay/quiz/fit';
import { remainingMs } from '../../../lesson-replay/src/lib/replay/quiz/clock';
import { scenes } from './scenes';

/** The class a size function gives, as the wide (`sm:`) variant the render always uses. */
const wide = (cls: string) => cls.match(/sm:(text-\S+)/)?.[1] ?? cls;
const TONES = ['red', 'blue', 'amber', 'green', 'violet', 'cyan', 'pink'];
const avatar = (name: string) => {
	const a = avatarOf(name);
	return { emoji: a.emoji, disc: TONES.indexOf(a.disc.match(/^bg-(\w+)-/)![1]) };
};

const json = (v: unknown) => JSON.stringify(v, null, 1) + '\n';

/** The countdown of the screens (`quiz/countdown.svelte.ts`), the same arithmetic on the replay's clock. */
function countdown(endsAt: number, limitMs: number, now: number) {
	const left = remainingMs(endsAt, now, 0);
	const seconds = Math.ceil(left / 1000);
	return {
		left,
		seconds,
		fraction: limitMs > 0 ? Math.min(1, left / limitMs) : 0,
		urgent: left > 0 && seconds <= 5,
		over: left === 0
	};
}

export async function files(): Promise<Map<string, Uint8Array | string>> {
	const out = new Map<string, Uint8Array | string>();
	const index: unknown[] = [];
	for (const scene of scenes()) {
		await replayStrings.use(scene.locale);
		const log = scene.records.map((r) => JSON.stringify(r)).join('\n') + '\n';
		out.set(`${scene.name}.ndjson.gz`, new Uint8Array(gzipSync(Buffer.from(log), { level: 9 })));
		const replay = new QuizReplay(quizRecords(scene.records), () => time);
		const quest = new QuestReplay(replay.records);
		let time = -1;
		const views: unknown[] = [];
		for (const t of scene.times) {
			time = t;
			replay.seek(t);
			quest.seek(t);
			const g = replay.game;
			const q = quest.game;
			const question = g.question;
			const names = [
				...g.joined,
				...g.ranking.map((l) => l.name),
				...g.standings.map((l) => l.name),
				...(g.reveal?.top ?? []).map((l) => l.name),
				...(g.podium?.top ?? []).map((l) => l.name),
				...(g.podium?.all ?? []).map((l) => l.name),
				...q.board.top.map((r) => r.name),
				...(q.podium?.top ?? []).map((l) => l.name),
				...(q.podium?.all ?? []).map((l) => l.name)
			];
			views.push({
				t,
				mode: replay.mode,
				on: replay.on,
				has: replay.has,
				race: {
					phase: g.phase,
					closing: g.closing,
					stage: g.stage,
					title: g.title,
					questionCount: g.questionCount,
					players: g.players,
					answered: g.answered,
					counts: g.counts,
					ranking: g.ranking,
					standings: g.standings,
					joined: g.joined,
					prevTop: g.prevTop,
					question,
					reveal: g.reveal,
					podium: g.podium,
					endedReason: g.endedReason
				},
				quest: {
					phase: q.phase,
					title: q.title,
					questionCount: q.questionCount,
					seconds: q.seconds,
					friendly: q.friendly,
					players: q.players,
					endsAt: q.endsAt,
					endedReason: q.endedReason,
					roster: q.roster,
					board: q.board,
					podium: q.podium,
					characters: [...new Set(names)].map((n) => [n, q.characterOf(n)])
				},
				countdown: question ? countdown(question.endsAt, question.limitMs, t) : null,
				questCountdown: q.endsAt ? countdown(q.endsAt, q.seconds * 1000, t) : null,
				derived: {
					questionSize: question ? wide(questionSize(question.text.length)) : null,
					optionSizes: question ? question.options.map((o) => wide(optionSize(o.length))) : [],
					columns: question
						? [200, 400, 560, 640].map((w) =>
								tileColumns(w, Math.max(0, ...question.options.map((o) => o.length)))
							)
						: [],
					accuracy: accuracy(q.board.correct, q.board.answers),
					clock: clock(countdown(q.endsAt, q.seconds * 1000, t).left)
				},
				avatars: [...new Set(names)].map((n) => ({
					name: n,
					...avatar(n),
					fallback: fallbackCharacter(n)
				}))
			});
		}
		out.set(`expected/${scene.name}.json`, json(views));
		index.push({
			name: scene.name,
			group: scene.group,
			about: scene.about,
			log: scene.name,
			locale: scene.locale,
			width: scene.width,
			height: scene.height,
			// The times at which a game is on the stage: the others are not drawn as a game.
			times: scene.times.filter((_, i) => (views[i] as { on: boolean }).on)
		});
		replay.game.dispose();
	}
	out.set('index.json', json({ groups: ['Quiz', 'Quest'], scenes: index }));
	const only = (dict: Record<string, string>) =>
		Object.fromEntries(
			Object.entries(dict).filter(([k]) => /^(quiz|quest|replay\.quiz)\./.test(k))
		);
	out.set('../../src/strings.json', json({ en: only(en), vi: only(vi) }));
	return out;
}
