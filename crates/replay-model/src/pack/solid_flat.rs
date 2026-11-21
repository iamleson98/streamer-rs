//! A solid as flat drawing primitives: the geometry half of `board/math/solids.ts`. One view, the
//! textbook one: cavalier (oblique) projection, the front face true, depth drawn at 45 degrees and
//! halved, hidden edges flagged, vertices lettered S.ABCD or ABC.A'B'C'. Hidden edges are exact for
//! convex solids: an edge is hidden when every face that owns it faces away from the viewer. Round
//! solids draw their outlines and the back half of their base circle hidden.

use std::collections::HashMap;
use std::f64::consts::PI;

use super::solid::{Centre, Shape, Solid, SolidPoint};

type Vec3 = [f64; 3];

/// A convex polyhedron: lettered vertices and faces (vertex index cycles).
struct Mesh {
    vertices: Vec<Vec3>,
    names: Vec<String>,
    faces: Vec<Vec<usize>>,
}

const LETTERS: &str = "ABCDEFGHIJKL";

fn letters(n: usize) -> Vec<String> {
    LETTERS.chars().take(n).map(String::from).collect()
}

fn prism_mesh(n: usize, edge: f64, h: f64) -> Mesh {
    let nf = n as f64;
    let radius = edge / (2.0 * (PI / nf).sin());
    let base: Vec<Vec3> = (0..n)
        .map(|i| {
            // The first edge faces the viewer: A front left, B front right, then round.
            let t = -PI / 2.0 - PI / nf + (2.0 * PI * i as f64) / nf;
            [radius * t.cos(), radius * t.sin(), 0.0]
        })
        .collect();
    let vertices = base
        .iter()
        .copied()
        .chain(base.iter().map(|[x, y, _]| [*x, *y, h]))
        .collect();
    let mut faces = vec![(0..n).collect::<Vec<_>>(), (0..n).map(|i| n + i).collect()];
    for i in 0..n {
        faces.push(vec![i, (i + 1) % n, n + ((i + 1) % n), n + i]);
    }
    let names = letters(n);
    let primed = names.iter().map(|c| format!("{c}'"));
    Mesh {
        vertices,
        names: names.iter().cloned().chain(primed).collect(),
        faces,
    }
}

fn pyramid_mesh(n: usize, edge: f64, h: f64) -> Mesh {
    let mut vertices = prism_mesh(n, edge, 0.0).vertices;
    vertices.truncate(n);
    vertices.push([0.0, 0.0, h]);
    let mut faces = vec![(0..n).collect::<Vec<_>>()];
    for i in 0..n {
        faces.push(vec![i, (i + 1) % n, n]);
    }
    let mut names = letters(n);
    names.push("S".to_owned());
    Mesh {
        vertices,
        names,
        faces,
    }
}

fn cuboid_mesh(a: f64, b: f64, h: f64) -> Mesh {
    Mesh {
        vertices: vec![
            [0.0, 0.0, 0.0],
            [a, 0.0, 0.0],
            [a, b, 0.0],
            [0.0, b, 0.0],
            [0.0, 0.0, h],
            [a, 0.0, h],
            [a, b, h],
            [0.0, b, h],
        ],
        names: ["A", "B", "C", "D", "A'", "B'", "C'", "D'"]
            .map(String::from)
            .to_vec(),
        faces: vec![
            vec![0, 1, 2, 3],
            vec![4, 5, 6, 7],
            vec![0, 1, 5, 4],
            vec![1, 2, 6, 5],
            vec![2, 3, 7, 6],
            vec![3, 0, 4, 7],
        ],
    }
}

fn mesh_of(el: &Solid) -> Option<Mesh> {
    let n = el.n as usize;
    Some(match el.shape {
        Shape::Cube => cuboid_mesh(el.a, el.a, el.a),
        Shape::Cuboid => cuboid_mesh(el.a, el.b, el.h),
        Shape::Prism => prism_mesh(n, el.a, el.h),
        Shape::Pyramid => pyramid_mesh(n, el.a, el.h),
        _ => return None,
    })
}

/// Depth drawn at 45 degrees, halved.
const K: f64 = 0.5;

fn dx() -> f64 {
    K * (PI / 4.0).cos()
}

fn dz() -> f64 {
    K * (PI / 4.0).sin()
}

/// From the viewer into the scene: a face is seen when its outward normal opposes this.
fn look() -> Vec3 {
    [-dx(), 1.0, -dz()]
}

/// x to the right, z up, depth up and to the right (the plane's y is up).
fn project([x, y, z]: Vec3) -> [f64; 2] {
    [x + dx() * y, z + dz() * y]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A line of the drawing: flat `[x, y, x, y...]` in the plane (y up).
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub points: Vec<f64>,
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub text: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Flat {
    pub lines: Vec<Line>,
    /// Visible faces and outlines to tint, as flat polygons.
    pub fills: Vec<Vec<f64>>,
    pub labels: Vec<Label>,
    /// Points to mark (a centre).
    pub dots: Vec<[f64; 2]>,
}

fn mean(v: &[Vec3], ids: &[usize]) -> Vec3 {
    let sum = ids.iter().fold([0.0; 3], |s, &i| {
        [s[0] + v[i][0], s[1] + v[i][1], s[2] + v[i][2]]
    });
    let n = ids.len() as f64;
    [sum[0] / n, sum[1] / n, sum[2] / n]
}

/// Which faces of a convex polyhedron the viewer sees.
fn facing_faces(mesh: &Mesh) -> Vec<bool> {
    let v = &mesh.vertices;
    let all: Vec<usize> = (0..v.len()).collect();
    let centre = mean(v, &all);
    mesh.faces
        .iter()
        .map(|face| {
            let mut n = cross(sub(v[face[1]], v[face[0]]), sub(v[face[2]], v[face[0]]));
            // Outward: away from the middle of the solid (it is convex).
            if dot(n, sub(mean(v, face), centre)) < 0.0 {
                n = [-n[0], -n[1], -n[2]];
            }
            dot(n, look()) < 0.0
        })
        .collect()
}

struct Added {
    lines: Vec<Line>,
    labels: Vec<Label>,
    dots: Vec<[f64; 2]>,
}

fn shared(a: &[usize], b: &[usize]) -> Vec<usize> {
    a.iter().copied().filter(|f| b.contains(f)).collect()
}

/// The points and segments the teacher added. A segment is seen when it lies in a face the viewer
/// sees (both ends belong to it); one that crosses the inside of the solid, or lies in a face
/// turned away, is dashed.
fn construct(el: &Solid, mesh: &Mesh, facing: &[bool]) -> Added {
    let mut place: HashMap<String, Vec3> = HashMap::new();
    let mut on_faces: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, name) in mesh.names.iter().enumerate() {
        place.insert(name.clone(), mesh.vertices[i]);
        let owned = (0..mesh.faces.len())
            .filter(|fi| mesh.faces[*fi].contains(&i))
            .collect();
        on_faces.insert(name.clone(), owned);
    }
    let mut labels = Vec::new();
    let mut dots = Vec::new();
    for p in &el.points {
        if place.contains_key(p.name()) {
            continue;
        }
        let (at, on) = match p {
            SolidPoint::Centre { centre, .. } => {
                // A prism's or cuboid's top is face 1; a pyramid has only its base.
                let face = usize::from(*centre == Centre::Top);
                if *centre == Centre::Top && el.shape == Shape::Pyramid {
                    continue;
                }
                (mean(&mesh.vertices, &mesh.faces[face]), vec![face])
            }
            SolidPoint::Between { from, to, t, .. } => {
                let (Some(a), Some(b)) = (place.get(from), place.get(to)) else {
                    continue;
                };
                let at = [
                    a[0] + (b[0] - a[0]) * t,
                    a[1] + (b[1] - a[1]) * t,
                    a[2] + (b[2] - a[2]) * t,
                ];
                (at, shared(&on_faces[from], &on_faces[to]))
            }
        };
        place.insert(p.name().to_owned(), at);
        on_faces.insert(p.name().to_owned(), on);
        let [x, y] = project(at);
        labels.push(Label {
            text: p.name().to_owned(),
            x,
            y,
        });
        dots.push([x, y]);
    }
    let mut lines = Vec::new();
    for s in &el.segments {
        let (Some(a), Some(b)) = (place.get(&s.from), place.get(&s.to)) else {
            continue;
        };
        let both = shared(&on_faces[&s.from], &on_faces[&s.to]);
        let hidden = s.dashed.unwrap_or_else(|| !both.iter().any(|f| facing[*f]));
        let (pa, pb) = (project(*a), project(*b));
        lines.push(Line {
            points: vec![pa[0], pa[1], pb[0], pb[1]],
            hidden,
        });
    }
    Added {
        lines,
        labels,
        dots,
    }
}

/// A polyhedron: its edges (hidden ones flagged) and the faces the viewer sees.
fn flatten_mesh(el: &Solid, mesh: &Mesh, with_hidden: bool) -> Flat {
    let v = &mesh.vertices;
    let facing = facing_faces(mesh);
    let mut owners: Vec<((usize, usize), Vec<usize>)> = Vec::new();
    for (fi, f) in mesh.faces.iter().enumerate() {
        for (k, &a) in f.iter().enumerate() {
            let b = f[(k + 1) % f.len()];
            let key = (a.min(b), a.max(b));
            match owners.iter_mut().find(|(k, _)| *k == key) {
                Some((_, faces)) => faces.push(fi),
                None => owners.push((key, vec![fi])),
            }
        }
    }
    let mut lines = Vec::new();
    for ((a, b), faces) in &owners {
        let hidden = !faces.iter().any(|fi| facing[*fi]);
        if hidden && !with_hidden {
            continue;
        }
        let (pa, pb) = (project(v[*a]), project(v[*b]));
        lines.push(Line {
            points: vec![pa[0], pa[1], pb[0], pb[1]],
            hidden,
        });
    }
    let added = construct(el, mesh, &facing);
    lines.extend(added.lines);
    let fills = mesh
        .faces
        .iter()
        .zip(&facing)
        .filter(|(_, seen)| **seen)
        .map(|(f, _)| f.iter().flat_map(|i| project(v[*i])).collect())
        .collect();
    let mut labels: Vec<Label> = mesh
        .names
        .iter()
        .zip(v)
        .map(|(text, p)| {
            let [x, y] = project(*p);
            Label {
                text: text.clone(),
                x,
                y,
            }
        })
        .collect();
    labels.extend(added.labels);
    Flat {
        lines,
        fills,
        labels,
        dots: added.dots,
    }
}

const CIRCLE_STEPS: usize = 96;

struct CirclePoint {
    xy: [f64; 2],
    front: bool,
}

/// A horizontal circle at height z: its points, and which of them face the viewer.
fn circle(r: f64, z: f64) -> Vec<CirclePoint> {
    (0..CIRCLE_STEPS)
        .map(|i| {
            let t = (2.0 * PI * i as f64) / CIRCLE_STEPS as f64;
            CirclePoint {
                xy: project([r * t.cos(), r * t.sin(), z]),
                // The side of a round solid is seen where its outward normal opposes the look.
                front: dot([t.cos(), t.sin(), 0.0], look()) < 0.0,
            }
        })
        .collect()
}

/// The circle split into its visible and hidden arcs (each a run of points).
fn arcs(points: &[CirclePoint], whole: bool) -> Vec<Line> {
    let n = points.len();
    let mut out = Vec::new();
    // Start where the visibility changes so no arc wraps around the start.
    let start = (0..n)
        .find(|&i| points[i].front != points[(i + n - 1) % n].front)
        .unwrap_or(0);
    let mut run: Vec<f64> = Vec::new();
    let mut hidden = !(whole || points[start].front);
    for k in 0..=n {
        let p = &points[(start + k) % n];
        let h = !(whole || p.front);
        if k > 0 && h != hidden {
            let keep = run[run.len() - 2..].to_vec();
            out.push(Line {
                points: std::mem::replace(&mut run, keep),
                hidden,
            });
            hidden = h;
        }
        run.extend(p.xy);
    }
    out.push(Line {
        points: run,
        hidden,
    });
    out.retain(|a| a.points.len() >= 4);
    out
}

const SPHERE_SQUASH: f64 = 0.35;

fn flatten_sphere(r: f64, with_hidden: bool) -> Flat {
    let mut outline = Vec::new();
    let mut near: Vec<f64> = Vec::new();
    let mut far: Vec<f64> = Vec::new();
    for i in 0..=CIRCLE_STEPS {
        let t = (2.0 * PI * i as f64) / CIRCLE_STEPS as f64;
        outline.extend([r * t.cos(), r * t.sin()]);
        // The equator seen from above: the near half is the lower one.
        let at = [r * t.cos(), r * SPHERE_SQUASH * t.sin()];
        if t <= PI {
            far.extend(at);
        } else {
            near.extend(at);
        }
    }
    far.extend([-r, 0.0]);
    near.splice(0..0, [r, 0.0]);
    let lines = vec![
        Line {
            points: outline.clone(),
            hidden: false,
        },
        Line {
            points: near,
            hidden: false,
        },
        Line {
            points: far,
            hidden: true,
        },
        Line {
            points: vec![0.0, 0.0, r, 0.0],
            hidden: true,
        },
    ];
    Flat {
        lines: keep_visible(lines, with_hidden),
        fills: vec![outline],
        labels: vec![Label {
            text: "O".to_owned(),
            x: 0.0,
            y: 0.0,
        }],
        dots: vec![[0.0, 0.0]],
    }
}

fn keep_visible(mut lines: Vec<Line>, with_hidden: bool) -> Vec<Line> {
    if !with_hidden {
        lines.retain(|l| !l.hidden);
    }
    lines
}

fn xy_line(from: [f64; 2], to: [f64; 2], hidden: bool) -> Line {
    Line {
        points: vec![from[0], from[1], to[0], to[1]],
        hidden,
    }
}

fn flatten_round(el: &Solid, with_hidden: bool) -> Flat {
    let r = el.a;
    let bottom = circle(r, 0.0);
    let apex = project([0.0, 0.0, el.h]);
    let centre_low = project([0.0, 0.0, 0.0]);
    let centre_high = project([0.0, 0.0, el.h]);
    let mut lines = arcs(&bottom, false);
    let (mut min_i, mut max_i) = (0, 0);
    for (i, p) in bottom.iter().enumerate() {
        if p.xy[0] < bottom[min_i].xy[0] {
            min_i = i;
        }
        if p.xy[0] > bottom[max_i].xy[0] {
            max_i = i;
        }
    }
    let mut fills = Vec::new();
    let mut labels = vec![Label {
        text: "O".to_owned(),
        x: centre_low[0],
        y: centre_low[1],
    }];
    let mut dots = vec![centre_low];
    if el.shape == Shape::Cylinder {
        let top = circle(r, el.h);
        // The top is seen from above: all of its circle is drawn.
        lines.extend(arcs(&top, true));
        lines.push(xy_line(top[min_i].xy, bottom[min_i].xy, false));
        lines.push(xy_line(top[max_i].xy, bottom[max_i].xy, false));
        lines.push(xy_line(centre_low, centre_high, true));
        fills.push(top.iter().flat_map(|p| p.xy).collect());
        labels.push(Label {
            text: "O'".to_owned(),
            x: centre_high[0],
            y: centre_high[1],
        });
        dots.push(centre_high);
    } else {
        // Tangents from the apex to the base: the extreme directions as seen from it.
        let rel = |i: usize| {
            let a = (bottom[i].xy[1] - apex[1]).atan2(bottom[i].xy[0] - apex[0]) + PI / 2.0;
            if a > PI {
                a - 2.0 * PI
            } else if a < -PI {
                a + 2.0 * PI
            } else {
                a
            }
        };
        let (mut lo, mut hi) = (0, 0);
        for i in 0..bottom.len() {
            if rel(i) < rel(lo) {
                lo = i;
            }
            if rel(i) > rel(hi) {
                hi = i;
            }
        }
        lines.push(xy_line(apex, bottom[lo].xy, false));
        lines.push(xy_line(apex, bottom[hi].xy, false));
        lines.push(xy_line(apex, centre_low, true));
        let mut fill = apex.to_vec();
        fill.extend(bottom[lo].xy);
        fill.extend(bottom.iter().flat_map(|p| p.xy));
        fill.extend(bottom[hi].xy);
        fills.push(fill);
        labels.push(Label {
            text: "S".to_owned(),
            x: apex[0],
            y: apex[1],
        });
    }
    Flat {
        lines: keep_visible(lines, with_hidden),
        fills,
        labels,
        dots,
    }
}

/// The solid as flat drawing primitives in the plane (y up, units of its dimensions).
pub fn flatten_solid(el: &Solid) -> Flat {
    if let Some(mesh) = mesh_of(el) {
        return flatten_mesh(el, &mesh, el.hidden);
    }
    if el.shape == Shape::Sphere {
        flatten_sphere(el.a, el.hidden)
    } else {
        flatten_round(el, el.hidden)
    }
}

/// The box around everything the solid draws (plane units): `[min_x, min_y, max_x, max_y]`.
pub fn flat_bounds(flat: &Flat) -> [f64; 4] {
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for l in &flat.lines {
        for p in l.points.as_chunks::<2>().0 {
            min_x = min_x.min(p[0]);
            max_x = max_x.max(p[0]);
            min_y = min_y.min(p[1]);
            max_y = max_y.max(p[1]);
        }
    }
    if min_x.is_finite() {
        [min_x, min_y, max_x, max_y]
    } else {
        [0.0, 0.0, 1.0, 1.0]
    }
}
