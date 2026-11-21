//! The maths pack's scenes: graphs (every kind of series and option, poles, steep slopes, formulas
//! that cannot be read, views from tiny to huge, boxes at their limits, the size they are too small
//! to draw at), solids (every shape, view option, added points and segments), both at every zoom
//! and under a remote user's previews.

use serde_json::{Value, json};

use super::{Board, Scene, centred, presence_rec, scene, shape};

const G: &str = "Maths";
/// 2 pi as the presets write it: rounded to four places.
fn tau4() -> f64 {
    (std::f64::consts::TAU * 1e4).round() / 1e4
}

/// A graph as a teacher's preset makes it: a box, a view and series, the rest at its defaults.
fn graph(color: &str, size: f64, b: [f64; 4], view: [f64; 4], series: Vec<Value>) -> Value {
    json!({"type": "graph", "color": color, "size": size,
        "x1": b[0], "y1": b[1], "x2": b[2], "y2": b[3], "view": view, "series": series,
        "grid": true, "axes": true, "degrees": false, "piTicks": false, "params": {}})
}

/// The same with some fields replaced.
fn with(mut el: Value, changes: Value) -> Value {
    el.as_object_mut()
        .unwrap()
        .extend(changes.as_object().unwrap().clone());
    el
}

fn func(expr: &str, color: &str, extra: Value) -> Value {
    with(
        json!({"kind": "fn", "expr": expr, "color": color, "dash": false, "shade": null, "marks": []}),
        extra,
    )
}

fn param(x: &str, y: &str, t: [f64; 2], color: &str, extra: Value) -> Value {
    with(
        json!({"kind": "param", "x": x, "y": y, "t": t, "color": color, "dash": false}),
        extra,
    )
}

fn point(x: f64, y: f64, label: &str, color: &str) -> Value {
    json!({"kind": "point", "x": x, "y": y, "label": label, "color": color})
}

/// The view with y's range made as long per unit as x's, round about its middle (`withEqualScale`).
fn equalised(view: [f64; 4], w: f64, h: f64) -> [f64; 4] {
    let span = (view[1] - view[0]) / w * h;
    let mid = (view[2] + view[3]) / 2.0;
    [view[0], view[1], mid - span / 2.0, mid + span / 2.0]
}

fn solid(shape: &str, color: &str, size: f64, b: [f64; 4], extra: Value) -> Value {
    with(
        json!({"type": "solid", "shape": shape, "color": color, "size": size,
            "x1": b[0], "y1": b[1], "x2": b[2], "y2": b[3],
            "a": 4, "b": 4, "h": 4, "n": 4, "labels": true, "hidden": true, "tint": false,
            "points": [], "segments": []}),
        extra,
    )
}

/// One graph on its own.
fn one(name: &str, about: &str, el: Value) -> Scene {
    let mut b = Board::new();
    b.add(el);
    b.still(name, G, about)
}

const BOX: [f64; 4] = [200.0, 150.0, 680.0, 470.0];

fn standard(view: [f64; 4], series: Vec<Value>) -> Value {
    graph("ink", 3.0, BOX, view, series)
}

fn graphs() -> Vec<Scene> {
    let mut out = Vec::new();
    out.push(one(
        "maths-graph-line-with-marks",
        "A line with letters for its slope and intercept; the intercept and root are marked and labelled.",
        with(
            standard(
                [-6.0, 6.0, -4.0, 6.0],
                vec![func("a*x+b", "red", json!({"marks": ["intercept", "roots"]}))],
            ),
            json!({"params": {"a": 2, "b": -1}}),
        ),
    ));
    out.push(one(
        "maths-graph-parabola-all-marks",
        "A parabola in vertex form: roots, the vertex and the y-intercept, with their coordinates.",
        with(
            standard(
                [-5.0, 6.0, -6.0, 8.0],
                vec![func(
                    "a*(x-h)^2+k",
                    "red",
                    json!({"marks": ["roots", "extrema", "intercept"]}),
                )],
            ),
            json!({"params": {"a": 1, "h": 1, "k": -4}}),
        ),
    ));
    out.push(one(
        "maths-graph-cubic-roots-and-extrema",
        "x^3-3x: three roots (one at the origin, where the axes' labels give way) and two turning points.",
        standard(
            [-3.0, 3.0, -5.0, 5.0],
            vec![func("x^3-3x", "red", json!({"marks": ["roots", "extrema"]}))],
        ),
    ));
    out.push(one(
        "maths-graph-sine-pi-ticks",
        "A sine wave with its four letters, ticks at multiples of pi.",
        with(
            standard(
                [-tau4(), tau4(), -2.5, 2.5],
                vec![func("A*sin(B*(x-C))+D", "red", json!({}))],
            ),
            json!({"piTicks": true, "params": {"A": 1, "B": 1, "C": 0, "D": 0}}),
        ),
    ));
    out.push(one(
        "maths-graph-sine-and-cosine-shifted",
        "Two waves in two colours with changed letters; pi ticks.",
        with(
            standard(
                [-tau4(), tau4(), -2.5, 2.5],
                vec![
                    func("A*sin(B*(x-C))+D", "red", json!({})),
                    func("cos(x)", "blue", json!({"dash": true})),
                ],
            ),
            json!({"piTicks": true, "params": {"A": 2, "B": 0.5, "C": 1.5, "D": -0.5}}),
        ),
    ));
    out.push(one(
        "maths-graph-tangent-poles",
        "tan(x): the curve is cut at every pole instead of jumping from one branch to the next.",
        with(
            standard(
                [-tau4(), tau4(), -4.0, 4.0],
                vec![func("tan(x)", "violet", json!({}))],
            ),
            json!({"piTicks": true}),
        ),
    ));
    out.push(one(
        "maths-graph-hyperbola-pole",
        "a/(x-h)+k: one pole, the branches on either side.",
        with(
            standard(
                [-6.0, 6.0, -5.0, 5.0],
                vec![func("a/(x-h)+k", "violet", json!({}))],
            ),
            json!({"params": {"a": 2, "h": 0, "k": 0}}),
        ),
    ));
    out.push(one(
        "maths-graph-reciprocal-poles",
        "1/x and 1/(x^2-1) together: poles at -1, 0 and 1, and the curve's far ends leaving the view.",
        standard(
            [-4.0, 4.0, -4.0, 4.0],
            vec![
                func("1/x", "orange", json!({})),
                func("1/(x^2-1)", "green", json!({"dash": true})),
            ],
        ),
    ));
    out.push(one(
        "maths-graph-exponential-logarithm-sqrt-abs",
        "Four curves whose domain or shape is cut by the view: a power of a letter, a logarithm (undefined at and below 0), a root and an absolute value.",
        with(
            standard(
                [-4.0, 9.0, -4.0, 10.0],
                vec![
                    func("a*b^x", "green", json!({"marks": ["intercept"]})),
                    func("ln(x)", "orange", json!({"marks": ["roots"]})),
                    func("sqrt(x-h)", "blue", json!({"marks": ["roots"]})),
                    func("abs(x-1)-2", "violet", json!({})),
                ],
            ),
            json!({"params": {"a": 1, "b": 2, "h": -2}}),
        ),
    ));
    out.push(one(
        "maths-graph-steep-slopes",
        "x^10 and 1e3*x: curves that leave the view almost at once; their heights are held to fifty views so the line stays a line.",
        standard(
            [-2.0, 2.0, -5.0, 5.0],
            vec![
                func("x^10", "red", json!({})),
                func("1e3*x", "blue", json!({})),
                func("x^2", "green", json!({})),
            ],
        ),
    ));
    out.push(one(
        "maths-graph-circle-ellipse-projectile",
        "Parametric curves: a circle and an ellipse with round scales, and a projectile with its angle in degrees.",
        with(
            graph(
                "ink",
                3.0,
                BOX,
                equalised([-6.0, 6.0, -4.0, 4.0], 480.0, 320.0),
                vec![
                    param("h+r*cos(t)", "k+r*sin(t)", [0.0, tau4()], "blue", json!({})),
                    param("h+a*cos(t)", "k+b*sin(t)", [0.0, tau4()], "green", json!({"dash": true})),
                    param("v*cos(θ)*t-6", "v*sin(θ)*t-5*t^2", [0.0, 3.5], "orange", json!({})),
                ],
            ),
            json!({"degrees": true, "params": {"r": 3, "h": 0, "k": 0, "a": 4, "b": 2, "v": 14, "θ": 50}}),
        ),
    ));
    out.push(one(
        "maths-graph-projectile-equal-scale",
        "A projectile's flight on a plane with one unit as long on y as on x.",
        with(
            graph(
                "ink",
                4.0,
                BOX,
                equalised([-1.0, 20.0, -1.0, 8.0], 480.0, 320.0),
                vec![param(
                    "v*cos(θ)*t",
                    "v*sin(θ)*t-5*t^2",
                    [0.0, 3.5],
                    "orange",
                    json!({}),
                )],
            ),
            json!({"degrees": true, "params": {"v": 14, "θ": 50}}),
        ),
    ));
    out.push(one(
        "maths-graph-vertical-line-parametric",
        "A vertical line, which no y = f(x) can be, and a constant curve.",
        standard(
            [-4.0, 4.0, -4.0, 4.0],
            vec![
                param("2", "t", [-3.0, 3.0], "red", json!({})),
                param("t", "-1", [-3.0, 3.0], "blue", json!({"dash": true})),
            ],
        ),
    ));
    out.push(one(
        "maths-graph-shaded-area",
        "The area under x^2 between 0 and 2, shaded, with its value (S = 2.667) written on it.",
        standard(
            [-1.5, 3.5, -1.0, 6.0],
            vec![func("x^2", "green", json!({"shade": [0, 2]}))],
        ),
    ));
    out.push(one(
        "maths-graph-shade-partly-outside-and-over-a-pole",
        "Shading that starts outside the view, one over a pole (no area is given) and a shaded sine that goes below the axis.",
        standard(
            [-3.0, 3.0, -3.0, 3.0],
            vec![
                func("x^2/3", "orange", json!({"shade": [-8, 1]})),
                func("1/x", "red", json!({"shade": [-1, 1]})),
                func("sin(2x)", "blue", json!({"shade": [-2, 2.5]})),
            ],
        ),
    ));
    out.push(one(
        "maths-graph-points-and-labels",
        "Labelled points: a label in every colour, one cut at 24 characters, one without a label and thick lines making the dots larger.",
        graph(
            "ink",
            8.0,
            BOX,
            [-5.0, 5.0, -4.0, 4.0],
            vec![
                point(-4.0, 3.0, "A", "red"),
                point(0.0, 0.0, "O'", "ink"),
                point(2.5, -1.5, "B(2,5; -1,5)", "blue"),
                point(4.5, 3.5, "abcdefghijklmnopqrstuvwxyz0123456789", "green"),
                point(-2.0, -3.0, "", "violet"),
                point(3.0, 2.0, "C", "orange"),
            ],
        ),
    ));
    out.push(one(
        "maths-graph-six-colours-dashed-and-solid",
        "One curve in each colour, alternately dashed and solid, at a stroke size of 2 (a hairline stroke is held to 1.5).",
        graph(
            "ink",
            2.0,
            BOX,
            [-4.0, 4.0, -4.0, 4.0],
            ["ink", "red", "orange", "green", "blue", "violet"]
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    func(
                        &format!("x+{}", i as f64 - 2.5),
                        c,
                        json!({"dash": i % 2 == 1}),
                    )
                })
                .collect(),
        ),
    ));
    out.push(one(
        "maths-graph-eight-series-and-the-ninth-dropped",
        "Eight curves, the most a graph keeps: the ninth and tenth in the log are dropped on reading.",
        standard(
            [-5.0, 5.0, -5.0, 5.0],
            (0..10)
                .map(|i| {
                    func(
                        &format!("sin(x)*{}+{}", 0.4 + f64::from(i) * 0.1, i - 5),
                        ["red", "orange", "green", "blue", "violet", "ink"][i as usize % 6],
                        json!({}),
                    )
                })
                .collect(),
        ),
    ));
    out.push(one(
        "maths-graph-degrees",
        "Trigonometric functions in degrees: one period of sine takes 360 on x.",
        with(
            standard(
                [-360.0, 360.0, -1.5, 1.5],
                vec![
                    func("sin(x)", "red", json!({"marks": ["extrema"]})),
                    func("cos(x)", "blue", json!({})),
                    func("tan(x)", "green", json!({"dash": true})),
                ],
            ),
            json!({"degrees": true}),
        ),
    ));
    let sin = || vec![func("sin(x)", "red", json!({"marks": ["roots"]}))];
    out.push(one(
        "maths-graph-no-grid",
        "Axes and ticks without the grid.",
        with(
            standard([-7.0, 7.0, -2.0, 2.0], sin()),
            json!({"grid": false}),
        ),
    ));
    out.push(one(
        "maths-graph-no-axes",
        "The grid without axes: no arrows, ticks or numbers.",
        with(
            standard([-7.0, 7.0, -2.0, 2.0], sin()),
            json!({"axes": false}),
        ),
    ));
    out.push(one(
        "maths-graph-bare-frame",
        "Neither grid nor axes: only the frame and the curve.",
        with(
            standard([-7.0, 7.0, -2.0, 2.0], sin()),
            json!({"axes": false, "grid": false}),
        ),
    ));
    out.push(one(
        "maths-graph-origin-outside-the-view-above-right",
        "A view that does not contain the origin: the axes sit on the nearest edges and the labels move with them.",
        standard(
            [2.0, 8.0, 3.0, 9.0],
            vec![func("x+1", "red", json!({"marks": ["intercept"]}))],
        ),
    ));
    out.push(one(
        "maths-graph-origin-outside-the-view-below-left",
        "The origin outside the view the other way: axes on the top and right edges.",
        standard([-8.0, -2.0, -9.0, -3.0], vec![func("x", "blue", json!({}))]),
    ));
    out.push(one(
        "maths-graph-axis-through-a-corner",
        "The y axis on the left edge (x from 0) and the x axis through the middle.",
        standard(
            [0.0, 10.0, -3.0, 3.0],
            vec![func(
                "sin(x)",
                "green",
                json!({"marks": ["roots", "extrema"]}),
            )],
        ),
    ));
    out.push(one(
        "maths-graph-tiny-view",
        "A view a thousandth wide: tick labels in exponent form and a curve that is a straight line.",
        standard(
            [-0.001, 0.001, -0.001, 0.001],
            vec![func("x^2+0.0000005", "red", json!({"marks": ["extrema", "intercept"]}))],
        ),
    ));
    out.push(one(
        "maths-graph-huge-view",
        "A view of a hundred million: tick labels as 1e8, a curve spanning it.",
        standard(
            [-1e8, 1e8, -1e8, 1e8],
            vec![func("x^2/1e8", "blue", json!({}))],
        ),
    ));
    out.push(one(
        "maths-graph-view-clamped-to-a-billion",
        "A view written far beyond what is kept (a trillion): it is clamped to a billion either way.",
        standard([-1e12, 1e12, -1e12, 1e12], vec![func("x", "red", json!({}))]),
    ));
    out.push(one(
        "maths-graph-view-narrow-span",
        "A view two millionths wide, almost the narrowest that is kept: ticks of a few ten-millionths.",
        standard(
            [10.0, 10.000_002, 0.0, 1.0],
            vec![func("x-10", "violet", json!({}))],
        ),
    ));
    out.push(one(
        "maths-graph-view-too-narrow-is-dropped",
        "A view narrower than a millionth: the element is not valid and draws nothing.",
        standard([0.0, 1e-7, 0.0, 1.0], vec![func("x", "red", json!({}))]),
    ));
    out.push(one(
        "maths-graph-minimum-box",
        "A box dragged to 20 by 20 is drawn at the smallest size, 80 by 80.",
        graph(
            "ink",
            2.0,
            [300.0, 200.0, 320.0, 220.0],
            [-3.0, 3.0, -3.0, 3.0],
            vec![func("x^2-1", "red", json!({"marks": ["roots"]}))],
        ),
    ));
    out.push(one(
        "maths-graph-box-dragged-from-the-far-corner",
        "A box whose corners are given the other way round is normalised.",
        graph(
            "ink",
            2.0,
            [650.0, 420.0, 250.0, 180.0],
            [-4.0, 4.0, -3.0, 3.0],
            vec![func("x^3/4", "orange", json!({}))],
        ),
    ));
    out.push(one(
        "maths-graph-maximum-box",
        "A box wider than 4000 units is drawn 4000 wide; the camera fits the board.",
        graph(
            "ink",
            4.0,
            [0.0, 0.0, 6000.0, 300.0],
            [-60.0, 60.0, -3.0, 3.0],
            vec![func("2*sin(x)", "green", json!({"marks": ["extrema"]}))],
        ),
    ));
    out.push(one(
        "maths-graph-unreadable-formulas",
        "Formulas that cannot be read draw no curve and the rest of the graph is unaffected: a syntax error, a stray character, an empty formula, brackets left open, a formula too long and a parametric curve with one bad half; a readable curve stays.",
        standard(
            [-4.0, 4.0, -4.0, 4.0],
            vec![
                func("2x+", "red", json!({})),
                func("x $ 2", "orange", json!({})),
                func("", "green", json!({})),
                func("sin(x", "blue", json!({})),
                func(&"x+".repeat(101), "violet", json!({})),
                param("t", "t +* 2", [-3.0, 3.0], "ink", json!({})),
                func("x^2/2-1", "red", json!({"marks": ["roots"]})),
            ],
        ),
    ));
    out.push(one(
        "maths-graph-values-that-are-not-numbers",
        "Formulas that give NaN or infinity: 1/0, ln(0), sqrt(-1), 0/0, x^1000 and tan at the poles; only where they are numbers do they draw.",
        standard(
            [-4.0, 4.0, -4.0, 4.0],
            vec![
                func("1/0", "red", json!({})),
                func("ln(0)", "orange", json!({})),
                func("sqrt(-1)", "green", json!({})),
                func("0/0", "blue", json!({})),
                func("x^1000", "violet", json!({"marks": ["intercept"]})),
                func("sqrt(x)/sqrt(x)", "ink", json!({})),
            ],
        ),
    ));
    out.push(one(
        "maths-graph-formula-grammar",
        "Written the way a teacher writes: implicit products (2x, a(x-h), sin 2x, xy), a power of a power, a signed exponent, pi and the Greek letter, the multiplication and minus signs.",
        with(
            standard(
                [-4.0, 4.0, -4.0, 4.0],
                vec![
                    func("2x−1", "red", json!({})),
                    func("a(x-h)^2/2", "orange", json!({})),
                    func("sin 2x+1", "green", json!({})),
                    func("2^-x", "blue", json!({})),
                    func("x^2^0.5", "violet", json!({})),
                    func("π/4*x×0.5", "ink", json!({})),
                ],
            ),
            json!({"params": {"a": 2, "h": 1}}),
        ),
    ));
    out.push(one(
        "maths-graph-crowded-marks",
        "More marks than room for: a fast sine whose roots and turning points are capped at twelve, each with its label.",
        standard(
            [-10.0, 10.0, -1.5, 1.5],
            vec![func("sin(3x)", "red", json!({"marks": ["roots", "extrema", "intercept"]}))],
        ),
    ));
    out.push(one(
        "maths-graph-pi-ticks-narrow-view",
        "A narrow view with ticks at multiples of pi: steps of pi/4.",
        with(
            standard(
                [-1.6, 1.6, -1.0, 1.0],
                vec![func("sin(x)", "red", json!({}))],
            ),
            json!({"piTicks": true}),
        ),
    ));
    out.push(one(
        "maths-graph-pi-ticks-wide-view",
        "A view of many periods with ticks at multiples of pi.",
        with(
            standard(
                [-95.0, 95.0, -1.5, 1.5],
                vec![func("sin(x)", "red", json!({}))],
            ),
            json!({"piTicks": true}),
        ),
    ));
    out.push(one(
        "maths-graph-fractional-parameters",
        "Parameters with fractions and negatives, and a letter the log gives no value to (it is 1).",
        with(
            standard(
                [-5.0, 5.0, -5.0, 5.0],
                vec![func("a*x^2+b*x+c+m", "blue", json!({"marks": ["roots", "extrema"]}))],
            ),
            json!({"params": {"a": -0.5, "b": 1.25, "c": 3.125}}),
        ),
    ));
    out
}

fn solids() -> Vec<Scene> {
    let mut out = Vec::new();
    let cell = |i: usize| {
        let (col, row) = (i % 4, i / 4);
        let (x, y) = (60.0 + col as f64 * 300.0, 40.0 + row as f64 * 280.0);
        [x, y, x + 280.0, y + 260.0]
    };
    let every = |hidden: bool, tint: bool, labels: bool| {
        let mut b = Board::new();
        let dims = [
            ("cube", json!({"a": 4})),
            ("cuboid", json!({"a": 5, "b": 3, "h": 3})),
            ("prism", json!({"n": 3, "a": 3, "h": 5})),
            ("pyramid", json!({"n": 4, "a": 4, "h": 5})),
            ("cylinder", json!({"a": 2, "h": 5})),
            ("cone", json!({"a": 2, "h": 5})),
            ("sphere", json!({"a": 3})),
        ];
        for (i, (shape, extra)) in dims.into_iter().enumerate() {
            let colour = ["ink", "red", "orange", "green", "blue", "violet", "ink"][i];
            b.add(solid(
                shape,
                colour,
                3.0,
                cell(i),
                with(
                    extra,
                    json!({"hidden": hidden, "tint": tint, "labels": labels}),
                ),
            ));
        }
        b
    };
    out.push(every(true, false, true).still(
        "maths-solid-every-shape",
        G,
        "All seven shapes: hidden edges dashed, vertices lettered (S.ABCD, ABC.A'B'C', O and S on the round ones).",
    ));
    out.push(every(false, false, true).still(
        "maths-solid-every-shape-hidden-edges-left-out",
        G,
        "The same with the hidden edges not drawn at all.",
    ));
    out.push(every(true, true, false).still(
        "maths-solid-every-shape-tinted-without-letters",
        G,
        "The visible faces lightly filled, no letters; the dots at the centres stay.",
    ));
    out.push(every(false, true, true).still(
        "maths-solid-every-shape-tinted-without-hidden-edges",
        G,
        "Tinted, hidden edges left out, lettered.",
    ));

    let mut b = Board::new();
    for (i, n) in [3, 5, 6, 12].into_iter().enumerate() {
        b.add(solid(
            "prism",
            "blue",
            4.0,
            cell(i),
            json!({"n": n, "a": 3, "h": 5}),
        ));
        b.add(solid(
            "pyramid",
            "green",
            4.0,
            cell(i + 4),
            json!({"n": n, "a": 4, "h": 5}),
        ));
    }
    out.push(b.still(
        "maths-solid-prisms-and-pyramids-by-sides",
        G,
        "Prisms (above) and pyramids (below) with 3, 5, 6 and 12 sides: the bases are regular, the first edge faces the viewer, and letters run to L.",
    ));

    let mut b = Board::new();
    b.add(solid(
        "pyramid",
        "ink",
        3.0,
        [60.0, 40.0, 440.0, 400.0],
        json!({
            "n": 4, "a": 4, "h": 6,
            "points": [
                {"name": "H", "centre": "base"},
                {"name": "M", "from": "A", "to": "B", "t": 0.5},
                {"name": "N", "from": "S", "to": "C", "t": 0.3},
                {"name": "K", "centre": "top"},
                {"name": "P", "from": "M", "to": "N", "t": 0.5},
            ],
            "segments": [
                {"from": "S", "to": "H"}, {"from": "M", "to": "N"},
                {"from": "S", "to": "M", "dashed": false}, {"from": "A", "to": "C", "dashed": true},
                {"from": "P", "to": "H"},
            ],
        }),
    ));
    b.add(solid("prism", "red", 3.0, [480.0, 40.0, 800.0, 400.0], json!({
        "n": 3, "a": 3, "h": 5,
        "points": [
            {"name": "O", "centre": "base"}, {"name": "O'", "centre": "top"},
            {"name": "M", "from": "A'", "to": "B'", "t": 0.5},
        ],
        "segments": [{"from": "O", "to": "O'"}, {"from": "A", "to": "M"}, {"from": "M", "to": "C"}],
    })));
    b.add(solid("cuboid", "blue", 3.0, [840.0, 40.0, 1200.0, 400.0], json!({
        "a": 4, "b": 3, "h": 2,
        "points": [{"name": "I", "from": "B", "to": "D'", "t": 0.5}],
        "segments": [{"from": "B", "to": "D'"}, {"from": "A", "to": "C'"}, {"from": "I", "to": "A"}],
    })));
    out.push(b.still(
        "maths-solid-added-points-and-segments",
        G,
        "What a lesson adds: the height of a pyramid, midpoints and a point a third of the way, a section line; a prism's axis; a cuboid's diagonals. A segment is seen only along a face the viewer sees unless it says otherwise.",
    ));

    let mut b = Board::new();
    b.add(solid("cube", "ink", 3.0, cell(0), json!({
        "a": 3,
        "points": [
            {"name": "A", "from": "B", "to": "C", "t": 0.5},
            {"name": "M", "from": "Q", "to": "A", "t": 0.5},
            {"name": "N", "from": "B", "to": "C", "t": 1.5},
            {"name": "X", "from": "B", "to": "C", "t": -1},
            {"name": "ABCDE", "centre": "top"},
            {"name": "", "centre": "top"},
        ],
        "segments": [{"from": "N", "to": "X"}, {"from": "Z", "to": "A"}, {"from": "A", "to": "A"}],
    })));
    b.add(solid(
        "pyramid",
        "ink",
        3.0,
        cell(1),
        json!({
            "points": [{"name": "K", "centre": "top"}, {"name": "H", "centre": "base"}],
            "segments": [{"from": "S", "to": "H"}, {"from": "S", "to": "K"}],
        }),
    ));
    b.add(solid(
        "sphere",
        "ink",
        3.0,
        cell(2),
        json!({
            "points": [{"name": "M", "centre": "base"}],
            "segments": [{"from": "O", "to": "M"}],
        }),
    ));
    out.push(b.still(
        "maths-solid-points-and-segments-that-do-not-apply",
        G,
        "Points and segments that cannot be placed are left out: a name that is a vertex, an unknown end, a name cut to three letters, an empty name, a fraction outside 0 to 1 (clamped), a pyramid's top, a round solid's added points.",
    ));

    let mut b = Board::new();
    b.add(solid(
        "cuboid",
        "orange",
        2.0,
        [300.0, 200.0, 320.0, 220.0],
        json!({"a": 5, "b": 3, "h": 3}),
    ));
    b.add(solid(
        "cone",
        "violet",
        12.0,
        [500.0, 80.0, 800.0, 380.0],
        json!({"a": 2, "h": 5}),
    ));
    b.add(solid(
        "sphere",
        "blue",
        1.0,
        [900.0, 60.0, 1220.0, 420.0],
        json!({"a": 3, "tint": true}),
    ));
    out.push(b.still(
        "maths-solid-minimum-box-and-line-widths",
        G,
        "A solid dragged to 20 by 20 is drawn in 80 by 80 (its letters crowd), a size of 12 is held to a width of 6 and a size of 1 to 1.5.",
    ));

    let mut b = Board::new();
    b.add(solid(
        "cuboid",
        "ink",
        3.0,
        [60.0, 40.0, 560.0, 140.0],
        json!({"a": 8, "b": 2, "h": 1, "tint": true}),
    ));
    b.add(solid(
        "cylinder",
        "green",
        3.0,
        [60.0, 180.0, 160.0, 580.0],
        json!({"a": 1, "h": 9, "tint": true}),
    ));
    b.add(solid(
        "cone",
        "red",
        3.0,
        [220.0, 180.0, 620.0, 580.0],
        json!({"a": 4, "h": 1}),
    ));
    b.add(solid(
        "cube",
        "orange",
        3.0,
        [660.0, 40.0, 1240.0, 600.0],
        json!({"a": 1e6, "labels": true}),
    ));
    out.push(b.still(
        "maths-solid-extreme-proportions",
        G,
        "A flat slab, a tall thin cylinder, a squat cone and the largest edge that is kept, each fitted to its box.",
    ));

    let mut b = Board::new();
    b.add(solid(
        "cube",
        "ink",
        3.0,
        cell(0),
        json!({"a": 0, "b": -3, "h": 1e9}),
    ));
    b.add(solid(
        "prism",
        "ink",
        3.0,
        cell(1),
        json!({"n": 2.5, "a": -1, "h": 0}),
    ));
    b.add(solid("pyramid", "ink", 3.0, cell(2), json!({"n": 13})));
    b.add(solid(
        "cylinder",
        "ink",
        3.0,
        cell(3),
        json!({"n": "x", "a": "3", "h": 1e9}),
    ));
    out.push(b.still(
        "maths-solid-dimensions-out-of-range",
        G,
        "Dimensions that are not positive fall back to 4, very large ones are held to a million, the number of sides is rounded and kept between 3 and 12.",
    ));
    out
}

/// A graph and a solid, a stroke and a sheet's worth of other things around them.
fn content() -> Vec<Value> {
    let mut b = Board::new();
    b.add(with(
        graph(
            "ink",
            3.0,
            [60.0, 60.0, 540.0, 380.0],
            [-5.0, 6.0, -6.0, 8.0],
            vec![
                func(
                    "a*(x-h)^2+k",
                    "red",
                    json!({"marks": ["roots", "extrema", "intercept"]}),
                ),
                func("x/2", "blue", json!({"dash": true, "shade": [0, 3]})),
                point(3.0, 1.5, "P", "violet"),
            ],
        ),
        json!({"params": {"a": 1, "h": 1, "k": -4}}),
    ));
    b.add(solid(
        "pyramid",
        "green",
        3.0,
        [600.0, 40.0, 900.0, 340.0],
        json!({
            "n": 4, "a": 4, "h": 5, "tint": true,
            "points": [{"name": "H", "centre": "base"}], "segments": [{"from": "S", "to": "H"}],
        }),
    ));
    b.add(solid(
        "cylinder",
        "orange",
        3.0,
        [940.0, 60.0, 1220.0, 340.0],
        json!({"a": 2, "h": 5}),
    ));
    b.add(with(
        graph(
            "ink",
            3.0,
            [60.0, 420.0, 540.0, 740.0],
            [-tau4(), tau4(), -2.5, 2.5],
            vec![
                func("sin(x)", "red", json!({})),
                func("cos(x)", "blue", json!({})),
            ],
        ),
        json!({"piTicks": true}),
    ));
    b.add(shape(
        "ellipse",
        "violet",
        4.0,
        [620.0, 420.0, 920.0, 560.0],
    ));
    b.snapshot()
}

fn zooms() -> Vec<Scene> {
    let mut out = Vec::new();
    for (name, zoom, centre, about) in [
        (
            "maths-zoom-0-2",
            0.2,
            (650.0, 400.0),
            "The smallest zoom: a graph 480 wide is 96 pixels, still drawn; its samples are at their minimum.",
        ),
        (
            "maths-zoom-0-4",
            0.4,
            (650.0, 400.0),
            "A graph is 192 pixels wide: sampled at the minimum and hairlines kept at one pixel.",
        ),
        ("maths-zoom-0-5", 0.5, (650.0, 400.0), "Half size."),
        ("maths-zoom-1", 1.0, (650.0, 400.0), "Scale 1."),
        (
            "maths-zoom-1-37-fractional-pan",
            1.37,
            (333.3, 211.7),
            "A fractional zoom on a fractional pan, on the first graph.",
        ),
        (
            "maths-zoom-2",
            2.0,
            (300.5, 220.25),
            "Twice the size: the curves are sampled more finely and the strokes are the board's, scaled.",
        ),
        (
            "maths-zoom-5",
            5.0,
            (180.0, 215.0),
            "The largest zoom, on the parabola's vertex and marks: the samples reach their maximum.",
        ),
        (
            "maths-zoom-5-solid",
            5.0,
            (750.0, 200.0),
            "The largest zoom on the pyramid: dashes, dots and letters at five times.",
        ),
    ] {
        let mut s = scene(name, G, about, Vec::new(), &[1500]);
        s.log = Some("maths-content".into());
        s.view = Some(centred(zoom, centre.0, centre.1, 1280, 624));
        out.push(s);
    }
    let mut s = scene(
        "maths-zoom-too-small-for-a-graph",
        G,
        "A graph under 24 pixels wide is only its frame: an 80-unit graph at 0.2 is 16 pixels.",
        Vec::new(),
        &[1500],
    );
    s.log = Some("maths-small".into());
    s.view = Some(centred(0.2, 150.0, 150.0, 1280, 624));
    out.push(s);
    let mut s = scene(
        "maths-zoom-just-big-enough-for-a-graph",
        G,
        "The same graph at 0.3 (24 pixels): drawn in full.",
        Vec::new(),
        &[1500],
    );
    s.log = Some("maths-small".into());
    s.view = Some(centred(0.3, 150.0, 150.0, 1280, 624));
    out.push(s);
    out
}

/// A graph at the smallest box, and a bigger one beside it.
fn small() -> Vec<Value> {
    let mut b = Board::new();
    b.add(graph(
        "ink",
        2.0,
        [100.0, 100.0, 180.0, 180.0],
        [-3.0, 3.0, -3.0, 3.0],
        vec![func("x^2-1", "red", json!({}))],
    ));
    b.add(solid(
        "cube",
        "blue",
        2.0,
        [200.0, 100.0, 280.0, 180.0],
        json!({"a": 3}),
    ));
    b.snapshot()
}

/// Boards a remote user changes by preview.
fn previews() -> Vec<Scene> {
    let mut out = Vec::new();
    let base = || {
        let mut b = Board::new();
        b.add_as(
            "g1",
            with(
                graph(
                    "ink",
                    3.0,
                    [60.0, 60.0, 540.0, 380.0],
                    [-6.0, 6.0, -4.0, 4.0],
                    vec![
                        func("a*sin(x)", "red", json!({"marks": ["extrema"]})),
                        func("x/2", "blue", json!({})),
                    ],
                ),
                json!({"params": {"a": 1.5}}),
            ),
        );
        b.add_as(
            "g2",
            with(
                graph(
                    "ink",
                    3.0,
                    [600.0, 60.0, 1080.0, 380.0],
                    [-5.0, 5.0, -3.0, 5.0],
                    vec![func(
                        "a*x^2+b",
                        "green",
                        json!({"marks": ["roots", "intercept"]}),
                    )],
                ),
                json!({"params": {"a": 1, "b": -2}}),
            ),
        );
        b.add_as(
            "s1",
            solid(
                "pyramid",
                "violet",
                3.0,
                [60.0, 420.0, 360.0, 720.0],
                json!({"n": 4, "a": 4, "h": 5}),
            ),
        );
        b
    };
    let at = |recs: &mut Vec<Value>, t: u32, user: &str, el: &str, set: Value| {
        recs.push(presence_rec(
            t,
            user,
            json!({"cursor": [500, 400], "preview": {"ref": el, "set": set}}),
        ));
    };

    let mut recs = base().snapshot();
    at(
        &mut recs,
        2000,
        "u1",
        "g1",
        json!({"x2": 700.5, "y2": 460.2, "view": [-6, 9.9, -4, 5.4]}),
    );
    out.push(scene(
        "maths-preview-graph-resized-by-a-corner",
        G,
        "Someone drags the corner of a graph: the box and the view are previewed together, as the editor sends them (the board shows only the preview, not the old graph).",
        recs,
        &[1500, 2500],
    ));

    let mut recs = base().snapshot();
    at(&mut recs, 2000, "u1", "g1", json!({"params": {"a": 3.25}}));
    at(&mut recs, 2100, "u2", "g2", json!({"params": {"b": 0.5}}));
    at(&mut recs, 2200, "u2", "g2", json!({"params": {"a": -0.5}}));
    out.push(scene(
        "maths-preview-parameter-sliders",
        G,
        "Sliders dragged by two people: only the letter each moves is sent, merged into the graph's own letters.",
        recs,
        &[1500, 2050, 2150, 2500],
    ));

    let mut recs = base().snapshot();
    at(
        &mut recs,
        2000,
        "u1",
        "s1",
        json!({"x1": 100.0, "x2": 500.0, "y2": 700.0, "a": 6, "h": 3}),
    );
    at(
        &mut recs,
        2100,
        "u2",
        "g2",
        json!({"degrees": true, "piTicks": true, "view": [-360, 360, -2, 2]}),
    );
    out.push(scene(
        "maths-preview-solid-resized-and-graph-switched",
        G,
        "A solid dragged to a new size and dimensions; a graph switched to degrees with pi ticks and a new view.",
        recs,
        &[1500, 2500],
    ));

    let mut recs = base().snapshot();
    at(&mut recs, 2000, "u1", "g1", json!({"view": [0, 0, 0, 0]}));
    at(&mut recs, 2100, "u2", "s1", json!({"shape": "torus"}));
    at(
        &mut recs,
        2200,
        "u3",
        "g2",
        json!({"series": [{"kind": "fn", "expr": "x", "color": "red"}], "x1": "left"}),
    );
    out.push(scene(
        "maths-preview-that-does-not-decode-is-ignored",
        G,
        "A preview that would make the element invalid (an empty view, an unknown shape, a corner that is not a number): nothing is drawn for it and the element stays as it is.",
        recs,
        &[1500, 2500],
    ));

    let mut recs = base().snapshot();
    at(
        &mut recs,
        2000,
        "u1",
        "g1",
        json!({"series": [{"kind": "fn", "expr": "x^3-3x", "color": "orange", "marks": ["roots"]}, {"kind": "point", "x": 1, "y": -2, "color": "ink", "label": "M"}], "grid": false}),
    );
    out.push(scene(
        "maths-preview-series-replaced",
        G,
        "A graph whose formulas are rewritten while someone types: the new curve and point replace the old ones, the grid goes.",
        recs,
        &[1500, 2500],
    ));

    let mut recs = base().snapshot();
    at(&mut recs, 2000, "u1", "g1", json!({"x2": 900.0}));
    recs.push(presence_rec(2500, "u1", json!({"cursor": [500, 400]})));
    out.push(scene(
        "maths-preview-ends-and-the-element-returns",
        G,
        "A preview that is replaced by a cursor alone: the graph is drawn as it was.",
        recs,
        &[2200, 2700],
    ));
    out
}

pub fn scenes() -> Vec<Scene> {
    let mut out = graphs();
    out.extend(solids());
    out.extend(zooms());
    out.extend(previews());
    out
}

/// The logs several scenes draw.
pub fn shared_logs() -> Vec<(&'static str, Vec<Value>)> {
    vec![("maths-content", content()), ("maths-small", small())]
}
