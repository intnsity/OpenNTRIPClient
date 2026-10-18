//! The iGage colorway icons (the docs.igage.com / forum.igage.com
//! appearance switcher's own outline set, 24x24, stroke 2, round caps),
//! painted from their SVG path data. A dependency-free stroke renderer for
//! the handful of path commands these icons use - an SVG rasterizer crate
//! would be a large dependency for six paths.
//!
//! Source SVGs: `_ButterPaper/icons/colorway-*.svg`.

use egui::{Color32, Pos2, Rect, Shape, Stroke, pos2, vec2};

use crate::settings::Colorway;

/// SVG path data per colorway, verbatim from the source files.
fn paths(c: Colorway) -> &'static [&'static str] {
    match c {
        Colorway::ButterPaper => &[
            "M6 2.5h8.5L19 7v14.5H6z",
            "M14.5 2.5V7H19M9 11.5h7M9 14.5h7M9 17.5h4.5",
        ],
        Colorway::SouthwestNight => &[
            "M18.5 9.2A4.6 4.6 0 1 1 13.8 3a3.6 3.6 0 0 0 4.7 6.2z",
            "M2 21h20M4 21v-3.5h4.5l1.5-2.5h5l1.5 2.5H20V21",
        ],
        Colorway::NordicNight => &[
            "M18.5 8.7A4.3 4.3 0 1 1 14.1 3a3.3 3.3 0 0 0 4.4 5.7z",
            "M6 21v-2M6 19l-3.5 0L6 12.5 9.5 19zM15 21v-2M15 19h-4l4-8 4 8z",
        ],
    }
}

/// Paint colorway `c`'s icon into `rect` (a square) in `ink`.
pub fn paint_colorway(painter: &egui::Painter, rect: Rect, c: Colorway, ink: Color32) {
    let scale = rect.width() / 24.0;
    let stroke = Stroke::new((1.8 * scale).max(1.2), ink);
    let map = |p: Pos2| rect.min + vec2(p.x, p.y) * scale;
    for d in paths(c) {
        for sub in polylines(d) {
            let pts: Vec<Pos2> = sub.into_iter().map(map).collect();
            if pts.len() >= 2 {
                let (first, last) = (pts[0], pts[pts.len() - 1]);
                painter.add(Shape::line(pts, stroke));
                // stroke-linecap: round. Only open ends get a cap - dotting
                // every vertex thickened the arcs into a solid moon.
                if (first - last).length() > 0.01 {
                    painter.circle_filled(first, stroke.width / 2.0, ink);
                    painter.circle_filled(last, stroke.width / 2.0, ink);
                }
            }
        }
    }
}

/// Flatten an SVG path (M m L l H h V v A a Z z, implicit repeats) into
/// polylines in the path's own coordinates. Unknown commands end parsing
/// (these icons use none).
pub fn polylines(d: &str) -> Vec<Vec<Pos2>> {
    let toks = tokenize(d);
    let mut out: Vec<Vec<Pos2>> = Vec::new();
    let mut cur = pos2(0.0, 0.0);
    let mut start = cur;
    let mut cmd = ' ';
    let mut i = 0;
    let num = |i: &mut usize| -> Option<f32> {
        match toks.get(*i) {
            Some(Tok::Num(v)) => {
                *i += 1;
                Some(*v)
            }
            _ => None,
        }
    };
    while i < toks.len() {
        if let Tok::Cmd(c) = toks[i] {
            cmd = c;
            i += 1;
            if matches!(c, 'Z' | 'z') {
                if let Some(line) = out.last_mut() {
                    line.push(start);
                }
                cur = start;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { pos2(0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            'M' => {
                let (Some(x), Some(y)) = (num(&mut i), num(&mut i)) else {
                    break;
                };
                cur = pos2(base.x + x, base.y + y);
                start = cur;
                out.push(vec![cur]);
                // Pairs after a moveto are implicit linetos.
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' => {
                let (Some(x), Some(y)) = (num(&mut i), num(&mut i)) else {
                    break;
                };
                cur = pos2(base.x + x, base.y + y);
                push(&mut out, cur);
            }
            'H' => {
                let Some(x) = num(&mut i) else { break };
                cur = pos2(if rel { cur.x + x } else { x }, cur.y);
                push(&mut out, cur);
            }
            'V' => {
                let Some(y) = num(&mut i) else { break };
                cur = pos2(cur.x, if rel { cur.y + y } else { y });
                push(&mut out, cur);
            }
            'A' => {
                let vals: Option<Vec<f32>> = (0..7).map(|_| num(&mut i)).collect();
                let Some(v) = vals else { break };
                let end = pos2(base.x + v[5], base.y + v[6]);
                for p in arc(cur, end, v[0], v[1], v[2], v[3] != 0.0, v[4] != 0.0) {
                    push(&mut out, p);
                }
                cur = end;
            }
            _ => break,
        }
    }
    out
}

fn push(out: &mut Vec<Vec<Pos2>>, p: Pos2) {
    match out.last_mut() {
        Some(line) => line.push(p),
        None => out.push(vec![p]),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tok {
    Cmd(char),
    Num(f32),
}

fn tokenize(d: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphabetic() {
            toks.push(Tok::Cmd(c));
            i += 1;
        } else if c == '-' || c == '.' || c.is_ascii_digit() {
            let s = i;
            i += 1;
            let mut dot = c == '.';
            while i < b.len() {
                let n = b[i] as char;
                if n.is_ascii_digit() || (n == '.' && !dot) {
                    dot |= n == '.';
                    i += 1;
                } else {
                    break;
                }
            }
            if let Ok(v) = d[s..i].parse() {
                toks.push(Tok::Num(v));
            }
        } else {
            i += 1; // separators
        }
    }
    toks
}

/// Points along an SVG elliptical arc from `p0` to `p1` (excluding `p0`),
/// via the spec's endpoint-to-center conversion (SVG 1.1 F.6.5).
fn arc(p0: Pos2, p1: Pos2, rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool) -> Vec<Pos2> {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 || p0 == p1 {
        return vec![p1];
    }
    let phi = rot_deg.to_radians();
    let (sin, cos) = phi.sin_cos();
    let dx = (p0.x - p1.x) / 2.0;
    let dy = (p0.y - p1.y) / 2.0;
    let x1 = cos * dx + sin * dy;
    let y1 = -sin * dx + cos * dy;
    // Scale radii up if the endpoints are too far apart (F.6.6).
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut k = (num / den).max(0.0).sqrt();
    if large == sweep {
        k = -k;
    }
    let cx1 = k * rx * y1 / ry;
    let cy1 = -k * ry * x1 / rx;
    let cx = cos * cx1 - sin * cy1 + (p0.x + p1.x) / 2.0;
    let cy = sin * cx1 + cos * cy1 + (p0.y + p1.y) / 2.0;
    let angle = |ux: f32, uy: f32| uy.atan2(ux);
    let t1 = angle((x1 - cx1) / rx, (y1 - cy1) / ry);
    let t2 = angle((-x1 - cx1) / rx, (-y1 - cy1) / ry);
    let mut dt = t2 - t1;
    let tau = std::f32::consts::TAU;
    if sweep && dt < 0.0 {
        dt += tau;
    } else if !sweep && dt > 0.0 {
        dt -= tau;
    }
    let steps = ((dt.abs() / tau) * 48.0).ceil().max(2.0) as usize;
    (1..=steps)
        .map(|s| {
            let t = t1 + dt * s as f32 / steps as f32;
            let (st, ct) = t.sin_cos();
            pos2(
                cx + cos * rx * ct - sin * ry * st,
                cy + sin * rx * ct + cos * ry * st,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Pos2, b: Pos2) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn sheet_outline_closes_on_its_start() {
        let lines = polylines("M6 2.5h8.5L19 7v14.5H6z");
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        let want = [
            pos2(6.0, 2.5),
            pos2(14.5, 2.5),
            pos2(19.0, 7.0),
            pos2(19.0, 21.5),
            pos2(6.0, 21.5),
            pos2(6.0, 2.5),
        ];
        assert_eq!(l.len(), want.len());
        for (a, b) in l.iter().zip(want) {
            assert!(close(*a, b), "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn moves_start_new_subpaths_and_implicit_pairs_are_lines() {
        let lines = polylines("M6 21v-2M6 19l-3.5 0L6 12.5 9.5 19z");
        assert_eq!(lines.len(), 2);
        assert!(close(lines[0][1], pos2(6.0, 19.0)));
        // "L6 12.5 9.5 19" is two linetos; z returns to (6,19).
        let l = &lines[1];
        assert!(close(l[1], pos2(2.5, 19.0)));
        assert!(close(l[2], pos2(6.0, 12.5)));
        assert!(close(l[3], pos2(9.5, 19.0)));
        assert!(close(*l.last().unwrap(), pos2(6.0, 19.0)));
    }

    #[test]
    fn arcs_stay_on_their_circle_and_end_exactly() {
        // The Southwest moon's outer arc: radius 4.6 around a center the
        // spec conversion recovers; every point must sit on that circle.
        let p0 = pos2(18.5, 9.2);
        let p1 = pos2(13.8, 3.0);
        let pts = arc(p0, p1, 4.6, 4.6, 0.0, true, true);
        assert!(close(*pts.last().unwrap(), p1));
        // Center is equidistant from both endpoints at the radius.
        let mid = pts[pts.len() / 2];
        let r = 4.6;
        let c = {
            // Fit from three points: endpoints and the middle sample.
            let (a, b, m) = (p0, p1, mid);
            let d = 2.0 * (a.x * (b.y - m.y) + b.x * (m.y - a.y) + m.x * (a.y - b.y));
            let sq = |p: Pos2| p.x * p.x + p.y * p.y;
            pos2(
                (sq(a) * (b.y - m.y) + sq(b) * (m.y - a.y) + sq(m) * (a.y - b.y)) / d,
                (sq(a) * (m.x - b.x) + sq(b) * (a.x - m.x) + sq(m) * (b.x - a.x)) / d,
            )
        };
        for p in &pts {
            assert!(((*p - c).length() - r).abs() < 0.05, "{p:?} off the circle");
        }
        // Large-arc: it covers more than half the circle.
        assert!(pts.len() > 24);
    }

    #[test]
    fn every_icon_parses_inside_its_box() {
        for c in [
            Colorway::ButterPaper,
            Colorway::SouthwestNight,
            Colorway::NordicNight,
        ] {
            let mut n = 0;
            for d in paths(c) {
                for line in polylines(d) {
                    assert!(line.len() >= 2, "{c:?}: degenerate subpath in {d}");
                    for p in line {
                        n += 1;
                        assert!(
                            (0.0..=24.0).contains(&p.x) && (0.0..=24.0).contains(&p.y),
                            "{c:?}: {p:?}"
                        );
                    }
                }
            }
            assert!(n > 6, "{c:?} drew almost nothing");
        }
    }
}
