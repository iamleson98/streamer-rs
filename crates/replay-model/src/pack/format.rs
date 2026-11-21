//! `formatNumber` of `board/math/graph.ts`: a compact number for tick labels and readouts, written
//! as JavaScript writes it (`toPrecision`, `toExponential`, `String(Number)`).

/// The first `count` significant digits of `abs` (above zero), rounded half up as `toPrecision`
/// rounds, and the exponent of the first digit.
fn significant(abs: f64, count: usize) -> (Vec<u8>, i32) {
    // Forty digits show every tie exactly: a double is a binary fraction, so a value that is not
    // a tie differs from one within the first thirty digits.
    let text = format!("{abs:.40e}");
    let (mantissa, exponent) = text.split_once('e').expect("exponent form");
    let mut exponent: i32 = exponent.parse().expect("exponent digits");
    let all: Vec<u8> = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    let mut digits = all[..count].to_vec();
    if all[count] >= 5 {
        let mut i = count;
        loop {
            if i == 0 {
                digits.insert(0, 1);
                digits.pop();
                exponent += 1;
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    (digits, exponent)
}

fn digit_text(digits: &[u8]) -> String {
    digits.iter().map(|d| char::from(b'0' + d)).collect()
}

/// `String(Number(v.toPrecision(digits)))`.
fn to_precision_number(v: f64, digits: usize) -> String {
    let (d, exponent) = significant(v.abs(), digits);
    let text = format!("{}.{}e{exponent}", digit_text(&d[..1]), digit_text(&d[1..]));
    let value: f64 = text.parse().expect("a decimal");
    let value = if v < 0.0 { -value } else { value };
    if value == 0.0 {
        "0".to_owned()
    } else {
        value.to_string()
    }
}

/// `v.toExponential(2)` with the zeros before `e` and the `+` removed: `1.5e6`, `2e-5`.
fn exponential(v: f64) -> String {
    let (d, exponent) = significant(v.abs(), 3);
    let mut mantissa = format!("{}.{}", digit_text(&d[..1]), digit_text(&d[1..]));
    while mantissa.ends_with('0') {
        mantissa.pop();
    }
    if mantissa.ends_with('.') {
        mantissa.pop();
    }
    format!("{}{mantissa}e{exponent}", if v < 0.0 { "-" } else { "" })
}

/// Compact number for tick labels, readouts and inputs (no float noise).
pub fn format_number(v: f64, digits: usize) -> String {
    if !v.is_finite() {
        return String::new();
    }
    if v.abs() < 1e-12 {
        return "0".to_owned();
    }
    let abs = v.abs();
    if !(1e-4..1e6).contains(&abs) {
        return exponential(v);
    }
    to_precision_number(v, digits)
}
