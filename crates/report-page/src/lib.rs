//! The page contract of the render reports: colour tokens with a dark mode, a layout that holds at
//! phone width, no external asset, a click to zoom a picture. The parity report
//! (`board-draw`'s `compare` example) carries the same style; the whole-video report
//! (`render-compare`) builds on this crate.

use std::fmt::Write as _;

/// The reports' style sheet: tokens on `:root`, dark mode under `prefers-color-scheme` guarded by
/// `[data-theme="light"]` and again under `[data-theme="dark"]`, and the report's own classes.
pub const CSS: &str = r#":root{--bg:#f6f7f9;--card:#fff;--text:#1b1f24;--muted:#5b6571;--line:#d9dee5;--ok:#1a7f37;--ok-bg:#dcf5e3;--bad:#c4281c;--bad-bg:#fde4e1;--warn:#8a5a00;--warn-bg:#fff0cc;--accent:#1971c2}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--bg:#14171b;--card:#1d2127;--text:#e6e9ee;--muted:#9aa4b0;--line:#323941;--ok:#56d364;--ok-bg:#16301f;--bad:#ff7b72;--bad-bg:#3a1b19;--warn:#ffc552;--warn-bg:#3a2c0c;--accent:#4dabf7}}
:root[data-theme="dark"]{--bg:#14171b;--card:#1d2127;--text:#e6e9ee;--muted:#9aa4b0;--line:#323941;--ok:#56d364;--ok-bg:#16301f;--bad:#ff7b72;--bad-bg:#3a1b19;--warn:#ffc552;--warn-bg:#3a2c0c;--accent:#4dabf7}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--text);font:16px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:1200px;margin:0 auto;padding:16px}
h1{font-size:1.5rem;margin:.5rem 0}h2{font-size:1.15rem;margin:0 0 .5rem}h3{font-size:1rem;margin:0 0 .25rem;overflow-wrap:anywhere}
p,li,dd{overflow-wrap:anywhere}
a{color:var(--accent)}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:16px;margin:16px 0}
.card.fail{border-color:var(--bad)}
.card.accepted{border-color:var(--warn)}
.group{margin:28px 0 0;padding-top:4px;border-top:2px solid var(--line)}
.group h2{font-size:1.3rem}
.meta{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,16rem),1fr));gap:8px 24px;margin:0}
.meta dt{color:var(--muted);font-size:.85rem}.meta dd{margin:0 0 .5rem}
.checks{display:flex;flex-wrap:wrap;gap:8px;margin:.5rem 0}
.chip{border-radius:999px;padding:2px 10px;font-size:.85rem;border:1px solid var(--line);text-decoration:none}
.ok{color:var(--ok);background:var(--ok-bg);border-color:var(--ok)}.warn{color:var(--warn);background:var(--warn-bg);border-color:var(--warn)}.bad{color:var(--bad);background:var(--bad-bg);border-color:var(--bad)}
table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}
th,td{text-align:left;padding:4px 10px 4px 0;border-bottom:1px solid var(--line);vertical-align:top}
td.n,th.n{text-align:right;padding-right:16px}
.tablewrap{overflow-x:auto}
.shots{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,17rem),1fr));gap:12px}
figure{margin:0;min-width:0}figcaption{font-size:.85rem;color:var(--muted);margin-bottom:4px}
.pic{overflow:auto;border:1px solid var(--line);border-radius:6px;background:#000;max-height:80vh}
.pic img{display:block;width:100%;height:auto;cursor:zoom-in}
.pic.zoom img{width:auto;max-width:none;cursor:zoom-out}
.nums{display:flex;flex-wrap:wrap;gap:4px 20px;margin:.75rem 0;font-variant-numeric:tabular-nums}
.nums span{white-space:nowrap}
ul.mm{margin:.25rem 0 0;padding-left:1.2rem}
.hint{color:var(--muted);font-size:.85rem}
details summary{cursor:pointer;font-weight:600}
code{font-size:.9em}
.tl{display:flex;flex-wrap:wrap;gap:1px;margin:.25rem 0}
.tl i{display:block;width:12px;height:16px;border-radius:2px;font-style:normal}
.tl .board{background:#e9ecef}.tl .screen{background:#4dabf7}.tl .card{background:#495057}.tl .quiz{background:#f59f00}.tl .none{background:transparent;border:1px dashed var(--line)}
.tl .cam{box-shadow:inset 0 -4px 0 #2f9e44}
.tl .off{outline:2px solid var(--bad)}
.legend{display:flex;flex-wrap:wrap;gap:4px 14px;font-size:.8rem;color:var(--muted);margin:.25rem 0}
.legend i{display:inline-block;width:12px;height:12px;border-radius:2px;margin-right:4px;vertical-align:-1px}
.banner{border:1px solid var(--warn);background:var(--warn-bg);color:var(--warn);border-radius:8px;padding:8px 12px;margin:12px 0}
.bar{display:inline-block;height:.6rem;background:var(--accent);border-radius:3px;vertical-align:middle}"#;

/// Click a picture to see it whole; click again to put it back.
pub const JS: &str = r#"document.addEventListener('click',function(e){var i=e.target;var p=i.closest&&i.closest('.pic');if(!p||i.tagName!=='IMG')return;if(i.dataset.full&&!i.dataset.on){i.dataset.thumb=i.src;i.src=i.dataset.full;i.dataset.on='1';p.classList.add('zoom')}else if(i.dataset.on){i.src=i.dataset.thumb;delete i.dataset.on;p.classList.remove('zoom')}else{p.classList.toggle('zoom')}});"#;

/// How a check ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pass,
    Accepted,
    Fail,
}

impl State {
    /// The worst of some states.
    pub fn worst(states: impl IntoIterator<Item = State>) -> State {
        states.into_iter().fold(State::Pass, |a, b| match (a, b) {
            (State::Fail, _) | (_, State::Fail) => State::Fail,
            (State::Accepted, _) | (_, State::Accepted) => State::Accepted,
            _ => State::Pass,
        })
    }
}

pub fn chip(state: State, label: &str) -> String {
    let (class, word) = match state {
        State::Pass => ("ok", "pass"),
        State::Accepted => ("warn", "accepted"),
        State::Fail => ("bad", "FAIL"),
    };
    format!(
        "<span class=\"chip {class}\">{word} {}</span>",
        escape(label)
    )
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

pub fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(char::from(A[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// A picture as a data URL image in a zoomable frame.
pub fn picture(mime: &str, bytes: &[u8], caption: &str) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "<figure><figcaption>{}</figcaption><div class=\"pic\"><img alt=\"{}\" src=\"data:{mime};base64,{}\"></div></figure>",
        escape(caption),
        escape(caption),
        base64(bytes)
    );
    out
}

/// The whole page around `body`: the title, the style, the script and a phone-width viewport.
pub fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{CSS}</style></head><body><main>{body}</main><script>{JS}</script></body></html>",
        escape(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_has_tokens_dark_mode_and_a_phone_viewport_and_no_external_asset() {
        let html = page("Title <x>", "<p>hi</p>");
        assert!(html.contains("<title>Title &lt;x&gt;</title>"));
        assert!(html.contains("width=device-width"));
        assert!(html.contains("prefers-color-scheme:dark"));
        assert!(html.contains(":root[data-theme=\"dark\"]"));
        assert!(!html.contains("http://") && !html.contains("https://"));
    }

    #[test]
    fn base64_pads_and_a_chip_names_its_state() {
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"Man"), "TWFu");
        assert!(chip(State::Fail, "a<b").contains("bad\">FAIL a&lt;b"));
        assert_eq!(
            State::worst([State::Pass, State::Accepted]),
            State::Accepted
        );
    }
}
