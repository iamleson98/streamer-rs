//! The maths drawings, call for call: `ops.json` holds what the web player's `drawGraph` and
//! `drawSolid` ask a canvas to do for forty graphs and solids (made by running them on a recording
//! context, once); the Rust drawing, run on a recording canvas, must ask for the same, in the same
//! order and the same state, to 1e-9. That covers the sampling, the marks and shading, the ticks,
//! the fitting and the label places of both, without a browser.

mod recorder;

use board_draw::maths::draw_pack;
use board_draw::render::{BoardTheme, ink};
use board_draw::{Canvas, LineCap, LineJoin};
use replay_model::ElementKind;
use replay_model::element::decode_element;
use serde_json::Value;

use recorder::Recorder;

fn near(a: &Value, b: &Value, at: &str) -> Result<(), String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            if (x - y).abs() <= 1e-9 * (1.0 + y.abs()) {
                Ok(())
            } else {
                Err(format!("{at}: {x} instead of {y}"))
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Err(format!("{at}: {} items instead of {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .try_for_each(|(i, (p, q))| near(p, q, &format!("{at}[{i}]")))
        }
        (Value::Object(x), Value::Object(y)) => {
            if x.len() != y.len() || x.keys().any(|k| !y.contains_key(k)) {
                return Err(format!(
                    "{at}: keys {:?} instead of {:?}",
                    x.keys().collect::<Vec<_>>(),
                    y.keys().collect::<Vec<_>>()
                ));
            }
            x.iter()
                .try_for_each(|(k, p)| near(p, &y[k], &format!("{at}.{k}")))
        }
        _ if a == b => Ok(()),
        _ => Err(format!("{at}: {a} instead of {b}")),
    }
}

#[test]
fn graphs_and_solids_make_the_web_players_drawing_calls() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("ops.json")).expect("ops.json");
    assert!(cases.len() >= 40);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let theme = if case["theme"] == "dark" {
            BoardTheme::Dark
        } else {
            BoardTheme::Light
        };
        let el = decode_element(case["el"].as_object().unwrap()).expect("the element decodes");
        let ElementKind::Pack(pack) = &el.kind else {
            panic!("a pack element")
        };
        let mut ctx = Recorder::new(case["scale"].as_f64().unwrap());
        // What the board sets before it draws an element.
        ctx.set_fill_style(ink(el.color, theme));
        ctx.set_stroke_style(ink(el.color, theme));
        ctx.set_line_width(el.size);
        ctx.set_line_cap(LineCap::Round);
        ctx.set_line_join(LineJoin::Round);
        draw_pack(&mut ctx, &el, pack, theme);
        let expected = case["events"].as_array().unwrap();
        for (i, (got, want)) in ctx.events.iter().zip(expected).enumerate() {
            if let Err(why) = near(got, want, &format!("{name} call {i} ({})", want["op"])) {
                panic!("{why}");
            }
        }
        assert_eq!(
            ctx.events.len(),
            expected.len(),
            "{name}: the number of calls"
        );
    }
}
