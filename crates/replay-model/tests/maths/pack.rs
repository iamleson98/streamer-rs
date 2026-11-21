use replay_model::pack::box_of;
use replay_model::pack::graph::Graph;
use replay_model::pack::sampling::{axis_ticks, nice_step, pi_label, sample_count};
use replay_model::pack::solid::Solid;
use replay_model::pack::solid_flat::{flat_bounds, flatten_solid};
use serde_json::{Map, Value};

use super::pack_data::{
    axis_tick_cases, boxes, flat_cases, graph_cases, nice_steps, pi_labels, sample_counts,
    solid_cases,
};

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * (1.0 + b.abs())
}

fn object(raw: &str) -> Map<String, Value> {
    serde_json::from_str(raw).expect("a JSON object")
}

#[test]
fn graphs_decode_with_the_web_players_clamps_and_limits() {
    for (raw, expected) in graph_cases() {
        assert_eq!(Graph::decode(&object(raw)), expected, "{raw}");
    }
}

#[test]
fn solids_decode_with_the_web_players_clamps_and_limits() {
    for (raw, expected) in solid_cases() {
        assert_eq!(Solid::decode(&object(raw)), expected, "{raw}");
    }
}

#[test]
fn boxes_are_normalized_and_kept_between_the_limits() {
    for ((x1, y1, x2, y2), (x, y, w, h)) in boxes() {
        let b = box_of(x1, y1, x2, y2, 80.0);
        assert_eq!((b.x, b.y, b.w, b.h), (x, y, w, h));
    }
}

#[test]
fn tick_steps_labels_and_sample_counts_match() {
    for (span, target, step) in nice_steps() {
        assert!(same(nice_step(span, target), step), "{span} {target}");
    }
    for (k, quarters, label) in pi_labels() {
        assert_eq!(pi_label(k, quarters), label, "{k}/{quarters}");
    }
    for (width, n) in sample_counts() {
        assert_eq!(sample_count(width), n, "{width}");
    }
    for (min, max, length, pi, expected) in axis_tick_cases() {
        let ticks = axis_ticks(min, max, length, pi);
        assert_eq!(ticks.len(), expected.len(), "{min} {max} {length} {pi}");
        for (t, (value, label)) in ticks.iter().zip(&expected) {
            assert!(same(t.value, *value), "{min} {max}: {} vs {value}", t.value);
            assert_eq!(t.label, *label, "{min} {max}");
        }
    }
}

fn summary(points: &[f64]) -> (usize, f64, f64, f64, f64, f64, f64) {
    let (mut sx, mut sy) = (0.0, 0.0);
    for p in points.as_chunks::<2>().0 {
        sx += p[0];
        sy += p[1];
    }
    let n = points.len();
    (
        n / 2,
        sx,
        sy,
        points[0],
        points[1],
        points[n - 2],
        points[n - 1],
    )
}

fn same_summary(
    a: (usize, f64, f64, f64, f64, f64, f64),
    b: (usize, f64, f64, f64, f64, f64, f64),
) -> bool {
    a.0 == b.0
        && same(a.1, b.1)
        && same(a.2, b.2)
        && same(a.3, b.3)
        && same(a.4, b.4)
        && same(a.5, b.5)
        && same(a.6, b.6)
}

#[test]
fn solids_flatten_to_the_web_players_lines_faces_labels_and_dots() {
    for (raw, want) in flat_cases() {
        let solid = Solid::decode(&object(raw)).expect("a solid");
        let flat = flatten_solid(&solid);
        assert_eq!(flat.lines.len(), want.lines.len(), "lines of {raw}");
        for (line, w) in flat.lines.iter().zip(&want.lines) {
            assert_eq!(line.hidden, w.0, "{raw}");
            let got = summary(&line.points);
            assert!(
                same_summary(got, (w.1, w.2, w.3, w.4, w.5, w.6, w.7)),
                "{raw}: {got:?} vs {w:?}"
            );
        }
        assert_eq!(flat.fills.len(), want.fills.len(), "fills of {raw}");
        for (fill, w) in flat.fills.iter().zip(&want.fills) {
            assert!(same_summary(summary(fill), *w), "{raw}");
        }
        assert_eq!(flat.labels.len(), want.labels.len(), "labels of {raw}");
        for (l, (text, x, y)) in flat.labels.iter().zip(&want.labels) {
            assert_eq!(l.text, *text, "{raw}");
            assert!(same(l.x, *x) && same(l.y, *y), "{raw}: label {text}");
        }
        assert_eq!(flat.dots.len(), want.dots.len(), "dots of {raw}");
        for (d, (x, y)) in flat.dots.iter().zip(&want.dots) {
            assert!(same(d[0], *x) && same(d[1], *y), "{raw}: dot");
        }
        for (g, w) in flat_bounds(&flat).iter().zip(want.bounds) {
            assert!(same(*g, w), "{raw}: bounds");
        }
    }
}

#[test]
fn graph_keeps_the_first_eight_parameters_in_the_order_they_arrive() {
    let raw = r#"{"x1":0,"y1":0,"x2":300,"y2":200,"view":[-5,5,-5,5],"series":[],
        "params":{"i":5,"h":5,"g":5,"f":5,"e":5,"d":5,"c":5,"b":5,"a":5}}"#;
    let graph = Graph::decode(&object(raw)).unwrap();
    let mut names: Vec<char> = graph.params.keys().copied().collect();
    names.sort_unstable();
    assert_eq!(names.iter().collect::<String>(), "bcdefghi");
}
