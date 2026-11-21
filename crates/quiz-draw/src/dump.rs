//! The layout dump of a painted tree, in the fields the page's dump writes (`parity/harness.ts`): for every
//! text node the lines as laid out (text, box, whether an ellipsis cut it), and the painted boxes.

use serde_json::json;

use crate::text::{Laid, TextEngine, TextSpec};
use crate::ui::Dump;

fn round2(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

/// Adds the lines of one text node. Chromium's boxes are those of the characters: the box is the font's
/// ascent and descent (rounded) around the baseline, not the line.
pub fn text_lines(engine: &TextEngine, spec: &TextSpec, laid: &Laid, x: f32, y: f32, width: f32, out: &mut Dump) {
    let metrics = laid.paragraph.get_line_metrics();
    let full: Vec<u16> = spec.spans.iter().flat_map(|s| s.text.encode_utf16()).collect();
    let clipped = spec.max_lines == Some(1) && laid.paragraph.max_intrinsic_width() > width + 0.01;
    for (n, m) in metrics.iter().enumerate() {
        let slice = &full[m.start_index..m.end_excluding_whitespaces.min(full.len())];
        let text = String::from_utf16_lossy(slice);
        if text.trim().is_empty() {
            continue;
        }
        out.missing += engine.missing(&text, spec.size);
        out.texts.push(json!({
            "text": text,
            "x": round2(x + m.left as f32),
            "y": round2(y + laid.half_leading + n as f32 * spec.line_height),
            "w": round2(((m.width as f32) * 64.0).ceil() / 64.0),
            "h": round2(laid.content_h),
            "clipped": clipped,
        }));
    }
}
