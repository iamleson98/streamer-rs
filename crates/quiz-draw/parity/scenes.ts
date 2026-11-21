// The scenes of the quiz parity check: made-up games in the shapes of a recording's board log
// (`{t, k:"quiz", q:[...]}`, the quiz service's frames), each with the times at which both sides
// draw them. Every screen a game can show, in both languages, with the cases that move the layout
// (long texts, many players, anonymised names, the last seconds, a short stage).

export type Scene = {
	name: string;
	group: 'Quiz' | 'Quest';
	locale: 'en' | 'vi';
	width: number;
	height: number;
	/** The recording's clock (ms) at which the screen is drawn, ascending. */
	times: number[];
	records: { t: number; k: 'quiz'; q: unknown[] }[];
};

/** The quiz service's clock at the first frame (the recording's own clock replaces it). */
const SERVICE = 5_000_000;
const rec = (t: number, q: unknown[]) => ({ t, k: 'quiz' as const, q });

const NAMES = ['Anna', 'Bình', 'Chi', '', 'Đức', 'Em', 'Phương Thảo', 'Giang', 'Hà', '', 'Khánh', 'Lan'];
const lines = (n: number, step = 120, top = 2400): [string, number][] =>
	Array.from({ length: n }, (_, i) => [NAMES[i % NAMES.length], top - i * step]);

const lobby = (t: number, title: string, questions: number, players: number) =>
	rec(t, [1, SERVICE, title, questions, players]);
const question = (
	t: number,
	q: number,
	count: number,
	limitMs: number,
	kind: number,
	text: string,
	options: string[]
) => rec(t, [2, SERVICE + t, q, count, SERVICE + t + limitMs, kind, text, options]);

const long = (n: number, seed: string) =>
	Array.from({ length: n }, (_, i) => seed.split(' ')[i % seed.split(' ').length]).join(' ');

const VI = 'Thủ đô của nước Việt Nam là thành phố nào, nơi có Hồ Gươm và Văn Miếu Quốc Tử Giám';
const OPTIONS4 = ['Hà Nội', 'Huế', 'Đà Nẵng', 'Cần Thơ'];

function raceQuestionScenes(): Scene[] {
	const base = (
		name: string,
		text: string,
		options: string[],
		kind: number,
		extra: Scene['records'] = [],
		times = [3_000, 12_000, 17_000, 22_500],
		locale: Scene['locale'] = 'en',
		height = 624
	): Scene => ({
		name,
		group: 'Quiz',
		locale,
		width: 1280,
		height,
		times,
		records: [
			lobby(1_000, 'Geography', 5, 12),
			question(2_000, 2, 5, 20_000, kind, text, options),
			...extra
		]
	});
	const live = [
		rec(5_000, [4, 7, 12, [3, 2, 1, 1], lines(6, 150, 1800)]),
		rec(9_000, [4, 9, 12, [4, 3, 1, 1], lines(10, 110, 2100)])
	];
	return [
		base('race-question-four', 'What is the capital of Vietnam?', OPTIONS4, 0),
		base('race-question-four-live', 'What is the capital of Vietnam?', OPTIONS4, 0, live, [
			6_000, 10_000, 17_000, 21_800
		]),
		base('race-question-vi-live', VI, OPTIONS4, 0, live, [10_000, 17_000], 'vi'),
		base(
			'race-question-multiple',
			'Which of these are prime numbers?',
			['2', '3', '4', '5', '9', '11'],
			1,
			live,
			[10_000, 20_000]
		),
		base('race-question-true-false', 'The sun is a star.', ['True', 'False'], 2, [], [3_000, 17_000]),
		base(
			'race-question-text-70',
			long(14, 'Which planet in the solar system is known as the red planet'),
			OPTIONS4,
			0,
			[],
			[3_000]
		),
		base(
			'race-question-text-140',
			long(26, 'Which planet in the solar system is known as the red planet'),
			['Mars', 'Venus', 'Jupiter', 'Mercury'],
			0,
			[],
			[3_000]
		),
		base(
			'race-question-text-260',
			long(44, 'Which planet in the solar system is known as the red planet'),
			['Mars', 'Venus', 'Jupiter', 'Mercury'],
			0,
			[],
			[3_000]
		),
		base(
			'race-question-long-options',
			'Choose the best description of photosynthesis.',
			[
				long(12, 'Plants turn sunlight water and carbon dioxide into sugar and oxygen'),
				long(24, 'Animals break down food with the help of oxygen to release energy'),
				long(8, 'Fungi grow on dead wood and recycle it into soil'),
				long(40, 'Bacteria in the soil change nitrogen from the air into forms plants can use')
			],
			0,
			[],
			[3_000, 15_000]
		),
		base(
			'race-question-six-short-stage',
			'Pick a colour.',
			['Red', 'Blue', 'Orange', 'Green', 'Purple', 'Cyan'],
			0,
			live,
			[10_000],
			'en',
			664
		),
		base('race-question-closing', 'What is the capital of Vietnam?', OPTIONS4, 0, [
			rec(21_000, [11, 2])
		], [21_500]),
		base(
			'race-question-anonymous-live',
			'What is the capital of Vietnam?',
			OPTIONS4,
			0,
			[rec(6_000, [4, 3, 12, [1, 1, 1, 0], [['', 900], ['Anna', 700], ['', 400]]])],
			[10_000]
		)
	];
}

function raceOtherScenes(): Scene[] {
	const top5 = lines(5, 150, 1900);
	const reveal = (explanation: string | null, top: [string, number][], correct = 1) => [
		lobby(1_000, 'Geography', 5, 12),
		question(2_000, 1, 5, 20_000, 0, 'What is the capital of Vietnam?', OPTIONS4),
		rec(5_000, [4, 8, 12, [6, 1, 1, 0], lines(10, 110, 1500)]),
		rec(22_000, [5, 1, correct, [9, 1, 1, 1], explanation, top])
	];
	const scene = (
		name: string,
		records: Scene['records'],
		times: number[],
		locale: Scene['locale'] = 'en',
		height = 624
	): Scene => ({ name, group: 'Quiz', locale, width: 1280, height, times, records });
	return [
		scene('race-lobby-waiting', [lobby(1_000, 'Geography', 5, 0)], [1_500]),
		scene(
			'race-lobby-names',
			[
				lobby(1_000, 'Geography', 5, 12),
				rec(2_000, [10, NAMES.slice(0, 7)]),
				rec(3_000, [10, NAMES.slice(7)])
			],
			[1_500, 3_500]
		),
		scene(
			'race-lobby-names-vi',
			[lobby(1_000, 'Địa lý', 5, 12), rec(2_000, [10, NAMES])],
			[2_500],
			'vi'
		),
		scene(
			'race-lobby-crowd',
			[
				lobby(1_000, 'Geography', 20, 140),
				...Array.from({ length: 14 }, (_, i) =>
					rec(2_000 + i * 100, [10, Array.from({ length: 10 }, (_, j) => `Student ${i * 10 + j + 1}`)])
				)
			],
			[4_000]
		),
		scene('race-reveal', reveal('Hà Nội has been the capital since 1010.', top5), [23_000]),
		scene('race-reveal-no-explanation', reveal(null, top5), [23_000]),
		scene('race-reveal-vi', reveal('Hà Nội là thủ đô từ năm 1010.', top5), [23_000], 'vi'),
		scene(
			'race-reveal-close-race',
			reveal('Close finish.', [['Anna', 1900], ['Bình', 1850], ['Chi', 1500], ['', 1200]]),
			[23_000]
		),
		scene(
			'race-reveal-multiple-correct',
			reveal(null, top5, 0b0101),
			[23_000]
		),
		scene(
			'race-podium',
			[...reveal(null, top5), rec(30_000, [6, lines(3, 200, 2600), lines(12, 150, 2600)])],
			[31_000]
		),
		scene(
			'race-podium-vi',
			[...reveal(null, top5), rec(30_000, [6, lines(3, 200, 2600), lines(12, 150, 2600)])],
			[31_000],
			'vi'
		),
		scene(
			'race-podium-two',
			[...reveal(null, top5), rec(30_000, [6, lines(2, 200, 2600), lines(2, 200, 2600)])],
			[31_000]
		),
		scene(
			'race-podium-crowd',
			[
				...reveal(null, top5),
				rec(30_000, [6, lines(3, 200, 2600), lines(40, 50, 2600)])
			],
			[31_000],
			'en',
			664
		)
	];
}

function questScenes(): Scene[] {
	const rows = (n: number): [string, number, number][] =>
		Array.from({ length: n }, (_, i) => [
			NAMES[i % NAMES.length] || '',
			(i * 5) % 12,
			4_200 - i * 310
		]);
	const quest = (
		name: string,
		extra: Scene['records'],
		times: number[],
		locale: Scene['locale'] = 'en',
		height = 624
	): Scene => ({
		name,
		group: 'Quest',
		locale,
		width: 1280,
		height,
		times,
		records: [rec(1_000, [20, SERVICE, 'Quest', 5, 3, 300, 0]), ...extra]
	});
	const running = [
		rec(2_000, [21, [['Anna', 1], ['Bình', 4]]]),
		rec(3_000, [22, SERVICE + 3_000, SERVICE + 303_000])
	];
	const podiumRows = (n: number): [string, number][] =>
		Array.from({ length: n }, (_, i) => [NAMES[i % NAMES.length], 3_900 - i * 420]);
	return [
		quest('quest-lobby', [], [1_500]),
		quest('quest-lobby-vi', [rec(1_200, [3, 24])], [1_500], 'vi'),
		quest('quest-board-empty', running, [3_500]),
		quest('quest-board', [...running, rec(4_000, [29, rows(8), 9, 40, 31, 290_000])], [4_500, 250_000, 290_000]),
		quest(
			'quest-board-ten',
			[...running, rec(4_000, [29, rows(10), 12, 120, 61, 290_000])],
			[4_500]
		),
		quest(
			'quest-board-vi',
			[...running, rec(4_000, [29, rows(6), 12, 0, 0, 290_000])],
			[4_500],
			'vi'
		),
		quest(
			'quest-board-short-stage',
			[...running, rec(4_000, [29, rows(10), 12, 120, 61, 290_000])],
			[4_500],
			'en',
			664
		),
		quest(
			'quest-board-long-names',
			[
				...running,
				rec(4_000, [
					29,
					[
						['A very long player name that never fits', 2, 900],
						['Nguyễn Thị Phương Thảo Nguyên', 7, 800]
					],
					2,
					10,
					5,
					290_000
				])
			],
			[4_500]
		),
		quest(
			'quest-podium',
			[...running, rec(4_000, [29, rows(8), 9, 40, 31, 290_000]), rec(300_000, [6, podiumRows(3), podiumRows(8)])],
			[301_000]
		),
		quest(
			'quest-podium-vi',
			[...running, rec(4_000, [29, rows(8), 9, 40, 31, 290_000]), rec(300_000, [6, podiumRows(3), podiumRows(8)])],
			[301_000],
			'vi'
		),
		quest(
			'quest-podium-one',
			[...running, rec(4_000, [29, rows(3), 3, 4, 2, 290_000]), rec(300_000, [6, podiumRows(1), podiumRows(1)])],
			[301_000]
		)
	];
}

/** A race then a Quest then a race: the routing between them and the reset a lobby makes. */
function routingScenes(): Scene[] {
	return [
		{
			name: 'mixed-race-then-quest',
			group: 'Quest',
			locale: 'en',
			width: 1280,
			height: 624,
			times: [1_500, 5_000, 12_000, 30_000, 41_000, 52_000, 54_000],
			records: [
				lobby(1_000, 'Geography', 5, 12),
				question(2_000, 1, 5, 20_000, 0, 'What is the capital of Vietnam?', OPTIONS4),
				rec(22_000, [5, 1, 1, [9, 1, 1, 1], null, lines(5, 150, 1900)]),
				rec(30_000, [6, lines(3, 200, 2600), lines(5, 150, 2600)]),
				rec(40_000, [7, 0]),
				rec(50_000, [20, SERVICE, 'Quest', 5, 3, 300, 0]),
				rec(51_000, [22, SERVICE + 51_000, SERVICE + 351_000]),
				rec(53_000, [29, [['Anna', 1, 400], ['', 3, 300]], 8, 40, 31, 290_000]),
				rec(400_000, [6, lines(3, 200, 2600), lines(3, 200, 2600)])
			]
		}
	];
}

export const scenes = (): Scene[] => [
	...raceQuestionScenes(),
	...raceOtherScenes(),
	...questScenes(),
	...routingScenes()
];
