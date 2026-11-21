//! Physics symbols against the web player's values (`board/physics/symbols.ts` run on the same
//! inputs with bun).

use replay_model::pack::symbol::{Symbol, SymbolKind, build_symbol};
use serde_json::{Map, Value, json};

fn decode(extra: Value) -> Option<Symbol> {
    let mut d: Map<String, Value> = serde_json::from_value(
        json!({"x1": 10, "y1": 20, "x2": 110, "y2": 80, "kind": "resistor"}),
    )
    .unwrap();
    d.extend(extra.as_object().unwrap().clone());
    Symbol::decode(&d)
}

fn value(kind: &str, v: Value) -> f64 {
    decode(json!({"kind": kind, "value": v})).unwrap().value
}

#[test]
fn values_are_clamped_to_the_kinds_range_and_default_to_its_initial() {
    assert_eq!(value("spring", json!(99)), 16.0);
    assert_eq!(value("spring", json!(1)), 3.0);
    assert_eq!(value("spring", json!(null)), 8.0);
    assert_eq!(value("spring", json!("x")), 8.0);
    assert_eq!(value("pendulum", json!(-200)), -80.0);
    assert_eq!(value("switch", json!(0.2)), 0.2);
    assert_eq!(value("convexLens", json!(55)), 55.0);
    assert_eq!(value("resistor", json!(5)), 0.0);
}

#[test]
fn turns_wrap_round_and_labels_are_cut() {
    let turns = |t: Value| decode(json!({"turns": t})).unwrap().turns;
    assert_eq!(turns(json!(5)), 1);
    assert_eq!(turns(json!(-1)), 3);
    assert_eq!(turns(json!(2.5)), 3);
    assert_eq!(turns(json!(-2.5)), 2);
    assert_eq!(turns(json!("a")), 0);
    let label = decode(json!({"label": "x".repeat(50)})).unwrap().label;
    assert_eq!(label.len(), 40);
    assert_eq!(decode(json!({"label": 7})).unwrap().label, "");
}

#[test]
fn unknown_kinds_and_missing_corners_do_not_decode() {
    assert_eq!(decode(json!({"kind": "nope"})), None);
    assert_eq!(decode(json!({"x2": null})), None);
    assert_eq!(decode(json!({"x1": "1"})), None);
}

#[test]
fn every_kind_is_named_as_on_the_wire() {
    for kind in SymbolKind::ALL {
        assert_eq!(decode(json!({"kind": kind.name()})).unwrap().kind, kind);
    }
}

fn near(a: f64, b: f64) {
    assert!(
        (a - b).abs() <= 1e-9 * (1.0 + b.abs()),
        "{a} instead of {b}"
    );
}

#[test]
fn an_incline_is_built_as_the_web_player_builds_it() {
    let d = build_symbol(SymbolKind::Incline, 260.0, 180.0, 30.0);
    assert_eq!(d.lines.len(), 4);
    let want = [
        -130.0,
        75.05553499465132,
        130.0,
        75.05553499465132,
        130.0,
        -75.05553499465132,
    ];
    for (g, w) in d.lines[0].points.iter().zip(want) {
        near(*g, w);
    }
    assert!(d.lines[0].fill && d.lines[1].fill && !d.lines[2].fill);
    assert_eq!(d.lines[2].points.len(), 42);
    near(d.lines[3].points[0], 117.0);
    near(d.texts[0].x, -59.93174056099099);
    near(d.texts[0].y, 56.28080146291447);
}

#[test]
fn a_concave_mirror_is_an_arc_with_hatching() {
    let d = build_symbol(SymbolKind::ConcaveMirror, 200.0, 180.0, 0.0);
    assert_eq!(d.lines.len(), 13);
    let want = [
        -53.79999999999998,
        -90.0,
        -58.03731329031801,
        -84.99051176613831,
    ];
    for (g, w) in d.lines[1].points.iter().zip(want) {
        near(*g, w);
    }
}
