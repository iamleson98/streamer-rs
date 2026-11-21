use replay_model::pack::expression::{Failure, Params, Variable, compile};
use replay_model::pack::format::format_number;

use super::expressions_data::{PARAMS, cases, numbers};

fn same(a: f64, b: f64) -> bool {
    if a.is_nan() || b.is_nan() {
        return a.is_nan() && b.is_nan();
    }
    if a.is_infinite() || b.is_infinite() {
        return a == b;
    }
    (a - b).abs() <= 1e-9 * (1.0 + b.abs())
}

#[test]
fn formulas_read_and_evaluate_as_the_web_player_does() {
    let params: Params = PARAMS.iter().copied().collect();
    for (src, t, degrees, expected) in cases() {
        let variable = if t { Variable::T } else { Variable::X };
        let got = compile(src, variable, degrees);
        match (got, expected) {
            (Ok(e), Ok((names, points))) => {
                let found: String = e.params.iter().collect();
                assert_eq!(found, names, "parameters of {src:?}");
                for &(x, y) in points {
                    let v = e.eval(x, &params);
                    assert!(same(v, y), "{src:?} at {x}: {v} instead of {y}");
                }
            }
            (Err(Failure { error, at }), Err((want, want_at))) => {
                assert_eq!((error, at), (want, want_at), "error of {src:?}");
            }
            (Ok(_), Err((want, _))) => panic!("{src:?} should fail with {want:?}"),
            (Err(f), Ok(_)) => panic!("{src:?} failed with {f:?}"),
        }
    }
}

#[test]
fn numbers_are_written_as_the_web_player_writes_them() {
    for (value, digits, text) in numbers() {
        assert_eq!(format_number(value, digits), text, "{value} to {digits}");
    }
}
