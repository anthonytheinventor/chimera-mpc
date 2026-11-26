//! The outlines the skin's face is drawn from, as JSON on stdout (tools/make_skin.py reads it):
//! the male body's tract at rest ("rest", for the parts that stay put), at each of the face
//! strip's postures ("face", the same list the plugin picks frames from) and at each soft palate
//! frame ("velum"). Points are body centimeters (x forward, y up).
//!
//! Each frame also carries "head": the head around the tract (skin, skull, brain, eye, nasal
//! cavity, palate, vertebrae, larynx, tongue fibres...), ported from CHIMERA's head template
//! (src/ui/render/profile.ts buildProfile, as docs/mockups/gen/face.ts draws it), and "handles":
//! the ring positions on the tongue, jaw, lips and soft palate. Built on the same frame and P()
//! mapping as the airway, so the head lines up with the moving mouth exactly.
//!
//!     cargo run --release --example dump_face > ../build/face.json
use chimera::anatomy::geom::{Vec2, catmull_rom, dist, rotate_about, v};
use chimera::anatomy::human::{BODY_MALE, HumanArtic, HumanFrame, HumanVocalTract, NEUTRAL};
use chimera_mpc_engine::{VELUM_FRAMES, face_postures, velum_frame_value};
use std::f64::consts::PI;

fn pts(v: &[Vec2]) -> String {
    let inner: Vec<String> = v.iter().map(|p| format!("[{:.4},{:.4}]", p.x, p.y)).collect();
    format!("[{}]", inner.join(","))
}

fn closed(p: &[Vec2]) -> Vec<Vec2> {
    let mut o = p.to_vec();
    if let Some(&f) = p.first() {
        o.push(f);
    }
    o
}

/// an ellipse; rotation in screen space (y down), as the TypeScript renderer has it
fn ellipse(c: Vec2, rx: f64, ry: f64, rot: f64, n: usize) -> Vec<Vec2> {
    let (cr, sr) = ((-rot).cos(), (-rot).sin());
    (0..=n)
        .map(|i| {
            let an = (i as f64 / n as f64) * PI * 2.0;
            let (x, y) = (an.cos() * rx, an.sin() * ry);
            v(c.x + x * cr - y * sr, c.y + x * sr + y * cr)
        })
        .collect()
}

fn round_rect(p0: Vec2, p1: Vec2, r: f64) -> Vec<Vec2> {
    let (x0, x1) = (p0.x.min(p1.x), p0.x.max(p1.x));
    let (y0, y1) = (p0.y.min(p1.y), p0.y.max(p1.y));
    let rr = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    let mut o = Vec::new();
    let mut corner = |cx: f64, cy: f64, a0: f64| {
        for i in 0..=6 {
            let an = a0 + (i as f64 / 6.0) * (PI / 2.0);
            o.push(v(cx + an.cos() * rr, cy + an.sin() * rr));
        }
    };
    corner(x1 - rr, y1 - rr, 0.0);
    corner(x0 + rr, y1 - rr, PI / 2.0);
    corner(x0 + rr, y0 + rr, PI);
    corner(x1 - rr, y0 + rr, 1.5 * PI);
    let first = o[0];
    o.push(first);
    o
}

/// The head template (profile.ts / face.ts) for one frame, as JSON object members.
fn head_json(t: &HumanVocalTract, f: &HumanFrame, a: &HumanArtic) -> String {
    let k = t.scale();
    let ph = t.body.pharynx;
    let p = |x: f64, y: f64| t.p(x, y);
    let b = |x: f64, y: f64| v(x * k, (if y < 7.0 { 7.0 - (7.0 - y) * ph } else { y }) * k);
    let condyle = t.p(-2.5, 10.5);
    let phi = (-a.jaw.clamp(-0.2, 3.0) * k) / dist(condyle, t.p(6.95, 7.75));
    let j = |x: f64, y: f64| rotate_about(t.p(x, y), condyle, phi);
    let lar = a.larynx * k;
    let ul_out: Vec<Vec2> = f.upper_lip[7..].to_vec();
    let mut ll_out: Vec<Vec2> = f.lower_lip[7..].to_vec();
    ll_out.reverse();
    let neck_front = catmull_rom(&[b(2.3, -4.6), b(2.38, -1.5), b(2.52, 0.4), b(3.0, 1.7), j(5.3, 3.0), j(7.25, 3.35), j(7.95, 4.6), j(7.55, 5.7)], 4);
    let face = catmull_rom(&[*ul_out.last().unwrap(), b(7.05, 10.6), b(7.75, 10.78), b(8.75, 11.05), b(9.25, 11.65), b(8.95, 12.6), b(8.35, 13.9), b(7.7, 15.0), b(7.95, 16.1), b(7.9, 18.1), b(6.9, 20.5)], 4);
    let cranium = catmull_rom(&[b(6.9, 20.5), b(3.0, 22.9), b(-2.2, 22.7), b(-6.9, 19.6), b(-8.7, 15.4), b(-8.3, 11.9), b(-7.4, 8.9), b(-6.8, 4.8), b(-6.6, 0.0), b(-6.6, -4.6)], 4);
    // the face (nose, brow, forehead) on its own, from the nose's base: it does not move with the lips
    let face_fixed = catmull_rom(&[b(7.05, 10.6), b(7.75, 10.78), b(8.75, 11.05), b(9.25, 11.65), b(8.95, 12.6), b(8.35, 13.9), b(7.7, 15.0), b(7.95, 16.1), b(7.9, 18.1), b(6.9, 20.5)], 4);
    let inset = catmull_rom(&[b(1.85, -4.6), b(1.9, -0.8), b(2.3, 1.6), j(5.2, 3.6), j(6.9, 3.9), j(7.4, 4.9), j(7.2, 5.9), b(6.6, 9.9), b(7.6, 10.9), b(8.55, 11.5), b(8.1, 13.4), b(7.25, 15.2), b(7.45, 18.0), b(3.0, 22.2), b(-2.0, 22.0), b(-6.3, 19.2), b(-7.9, 15.3), b(-7.6, 12.0), b(-6.7, 8.7), b(-6.1, 4.6), b(-6.0, -4.6)], 3);
    let skull = catmull_rom(&[b(7.0, 16.2), b(7.3, 18.5), b(6.0, 20.7), b(3.0, 22.1), b(-2.0, 22.0), b(-6.3, 19.2), b(-7.9, 15.3), b(-7.4, 12.6), b(-4.6, 12.2), b(-2.2, 13.3), b(1.0, 13.7), b(4.1, 14.1), b(6.2, 14.9)], 4);
    let brain = catmull_rom(&[b(6.55, 16.3), b(6.75, 18.4), b(5.5, 20.1), b(2.8, 21.4), b(-1.9, 21.3), b(-5.8, 18.8), b(-7.1, 15.4), b(-6.7, 13.1), b(-4.4, 12.8), b(-2.1, 13.9), b(1.0, 14.3), b(4.0, 14.7), b(5.9, 15.4)], 4);
    let sulcus = catmull_rom(&[b(5.4, 19.4), b(2.4, 20.5), b(-1.4, 20.4), b(-4.8, 18.4)], 4);
    let wall_x = |y: f64| -1.2 - 0.025 * y;
    let mut vertebrae = Vec::new();
    for i in 0..9 {
        let y0 = -4.4 + i as f64 * 1.75;
        if y0 > 10.5 {
            break;
        }
        let x0 = wall_x(y0) - 2.25;
        vertebrae.push(round_rect(p(x0, y0 + 1.4), p(x0 + 1.6, y0), 0.32 * k));
    }
    let nasal = catmull_rom(&[b(0.3, 11.2), b(1.1, 10.25), b(2.7, 10.75), b(4.6, 10.85), b(6.4, 10.65), b(7.6, 10.85), b(8.45, 11.3), b(8.0, 12.2), b(6.7, 13.4), b(4.4, 13.9), b(1.8, 13.6), b(0.2, 12.6)], 4);
    let turbinates: Vec<Vec<Vec2>> = [(3.0, 11.45, 2.1, 0.32), (3.5, 12.45, 1.9, 0.3), (4.0, 13.3, 1.45, 0.22)]
        .iter()
        .map(|&(cx, cy, rx, ry)| ellipse(b(cx, cy), rx * k, ry * k, -0.04, 48))
        .collect();
    let pal_raw = [(1.0, 9.6), (1.8, 9.92), (2.7, 10.18), (3.6, 10.3), (4.4, 10.25), (5.1, 10.05), (5.7, 9.72), (6.15, 9.36), (6.55, 9.15), (7.0, 9.6)];
    let pal_oral: Vec<Vec2> = pal_raw.iter().map(|&(x, y)| p(x, y)).collect();
    let pal_nasal: Vec<Vec2> = pal_oral.iter().map(|q| v(q.x, q.y + 0.55 * k)).collect();
    let jb: Vec<Vec2> = f.jaw_bone.iter().copied().filter(|q| q.x > 0.8 * k).collect();
    let jb_upper: Vec<Vec2> = jb.iter().map(|q| v(q.x, q.y + 0.75 * k)).collect();
    let g = f.glottis;
    let hy = v(p(1.6, 0.0).x, p(0.0, 2.9).y + 0.6 * lar);
    let mut rings = Vec::new();
    for i in 0..4 {
        let y = g.y - (0.9 + i as f64 * 1.0) * k;
        if y < -4.4 {
            break;
        }
        rings.push(vec![v(g.x + 0.8 * k, y), v(g.x + 1.05 * k, y - 0.05), v(g.x + 1.05 * k, y - 0.5 * k), v(g.x + 0.82 * k, y - 0.45 * k), v(g.x + 0.8 * k, y)]);
    }
    let wall_at_glottis = v(wall_x(g.y / k) * k, g.y);
    let origin = j(6.0, 5.45);
    let tc = f.tongue_center;
    let mut fibres = Vec::new();
    for i in 0..=16 {
        let ang = PI * (0.18 + (0.9 * i as f64) / 16.0);
        let end = v(tc.x + ang.cos() * f.tongue_radius * 1.25, tc.y + ang.sin() * f.tongue_radius * 1.25);
        let ctrl = v(tc.x, (tc.y + origin.y) / 2.0);
        let c: Vec<Vec2> = (0..=16)
            .map(|jj| {
                let u = jj as f64 / 16.0;
                v((1.0 - u) * (1.0 - u) * origin.x + 2.0 * (1.0 - u) * u * ctrl.x + u * u * end.x, (1.0 - u) * (1.0 - u) * origin.y + 2.0 * (1.0 - u) * u * ctrl.y + u * u * end.y)
            })
            .collect();
        fibres.push(c);
    }
    let mut skin_lower = neck_front.clone();
    skin_lower.extend_from_slice(&ll_out);
    let mut skin_upper = ul_out.clone();
    skin_upper.extend_from_slice(&face[1..]);
    skin_upper.extend_from_slice(&cranium[1..]);
    let mut lower_teeth = f.lower_teeth.clone();
    lower_teeth.push(j(6.8, 6.0));
    lower_teeth.push(j(6.4, 6.2));
    let outer_wall: Vec<Vec2> = f.outer.iter().copied().filter(|q| q.y > g.y || q.x > 0.0).collect();
    let glottis_top = f.outer.iter().copied().find(|q| q.y > g.y).unwrap_or(wall_at_glottis);
    let thyroid = vec![v(g.x + 0.75 * k, g.y - 1.6 * k), v(g.x + 1.75 * k, g.y - 0.6 * k), v(g.x + 1.95 * k, g.y + 1.2 * k), v(g.x + 1.45 * k, g.y + 1.9 * k), v(g.x + 1.15 * k, g.y + 1.1 * k), v(g.x + 1.3 * k, g.y - 0.4 * k), v(g.x + 0.75 * k, g.y - 1.6 * k)];

    let single: Vec<(&str, Vec<Vec2>)> = vec![
        ("skin_lower", skin_lower),
        ("skin_upper", skin_upper),
        ("face", face_fixed),
        ("cranium", cranium),
        ("inset", inset),
        ("skull", closed(&skull)),
        ("brain", closed(&brain)),
        ("sulcus", sulcus),
        ("eye", ellipse(b(6.5, 15.4), 1.1 * k, 0.95 * k, 0.0, 48)),
        ("lens", ellipse(b(7.35, 15.4), 0.18 * k, 0.42 * k, 0.0, 24)),
        ("cord_a", vec![p(-5.3, -4.6), p(-4.5, -4.6), p(-4.5, 11.8), p(-5.1, 12.4), p(-5.6, 11.6)]),
        ("cord_b", vec![p(-5.1, -4.6), p(-4.7, -4.6), p(-4.72, 11.4), p(-5.15, 11.6)]),
        ("nasal", closed(&nasal)),
        ("pal_oral", pal_oral),
        ("pal_nasal", pal_nasal),
        ("jaw_lower", jb),
        ("jaw_upper", jb_upper),
        ("hyoid", ellipse(hy, 0.5 * k, 0.28 * k, 0.3, 32)),
        ("thyroid", thyroid),
        ("trachea_l", vec![v(g.x - 0.65 * k, g.y), v(g.x - 0.75 * k, -4.8)]),
        ("trachea_r", vec![v(g.x + 0.75 * k, g.y), v(g.x + 0.95 * k, -4.8)]),
        ("outer_wall", outer_wall),
        ("glottis_wall", vec![wall_at_glottis, glottis_top]),
        ("inner_wall", f.inner.clone()),
        ("tongue", closed(&f.tongue)),
        ("upper_lip", closed(&f.upper_lip)),
        ("lower_lip", closed(&f.lower_lip)),
        ("upper_teeth", closed(&f.upper_teeth)),
        ("lower_teeth", closed(&lower_teeth)),
        ("epiglottis", f.epiglottis.clone()),
        ("velum", closed(&f.velum)),
        ("fold_l", vec![v(g.x - 0.62 * k, g.y + 0.05), v(g.x - 0.1, g.y + 0.12 * k), v(g.x - 0.62 * k, g.y + 0.32 * k)]),
        ("fold_r", vec![v(g.x + 0.72 * k, g.y + 0.05), v(g.x + 0.12, g.y + 0.12 * k), v(g.x + 0.72 * k, g.y + 0.32 * k)]),
    ];
    let multi: Vec<(&str, Vec<Vec<Vec2>>)> = vec![("vertebrae", vertebrae), ("turbinates", turbinates), ("rings", rings), ("fibres", fibres)];
    let mut members: Vec<String> = single.iter().map(|(n, q)| format!("\"{n}\":{}", pts(q))).collect();
    for (n, list) in &multi {
        let inner: Vec<String> = list.iter().map(|q| pts(q)).collect();
        members.push(format!("\"{n}\":[{}]", inner.join(",")));
    }
    // the rings (no dragging on the MPC: drawn, as in CHIMERA, but the knobs move the mouth)
    let lips_h = v((f.upper_lip[6].x + f.lower_lip[6].x) / 2.0 + 0.15, (f.upper_lip[6].y + f.lower_lip[6].y) / 2.0);
    let jp = j(7.5, 4.9);
    let vp = f.velum[f.velum.len() / 2];
    let handles = [("tongue", tc), ("tip", f.tip), ("jaw", jp), ("lips", lips_h), ("velum", vp), ("larynx", v(g.x + 1.2 * k, g.y))];
    let hs: Vec<String> = handles.iter().map(|(n, q)| format!("\"{n}\":[{:.4},{:.4}]", q.x, q.y)).collect();
    format!("\"head\":{{{}}},\"handles\":{{{}}}", members.join(","), hs.join(","))
}

fn frame_json(t: &mut HumanVocalTract, a: &HumanArtic) -> String {
    let mut f = HumanFrame::with_capacity();
    t.compute(a, &mut f);
    let parts: [(&str, &[Vec2]); 11] = [
        ("outer", &f.outer),
        ("inner", &f.inner),
        ("tongue", &f.tongue),
        ("velum", &f.velum),
        ("upper_lip", &f.upper_lip),
        ("lower_lip", &f.lower_lip),
        ("upper_teeth", &f.upper_teeth),
        ("lower_teeth", &f.lower_teeth),
        ("jaw_bone", &f.jaw_bone),
        ("epiglottis", &f.epiglottis),
        ("nasal_cavity", &f.nasal_cavity),
    ];
    let body: Vec<String> = parts.iter().map(|(k, v)| format!("\"{k}\":{}", pts(v))).collect();
    let head = head_json(t, &f, a);
    format!("{{{},\"glottis\":[{:.4},{:.4}],{head}}}", body.join(","), f.glottis.x, f.glottis.y)
}

fn main() {
    let mut t = HumanVocalTract::new(BODY_MALE);
    let rest = frame_json(&mut t, &NEUTRAL);
    let face: Vec<String> = face_postures().iter().map(|a| frame_json(&mut t, a)).collect();
    let velum: Vec<String> = (0..VELUM_FRAMES)
        .map(|i| {
            let mut a = NEUTRAL;
            a.velum = velum_frame_value(i);
            frame_json(&mut t, &a)
        })
        .collect();
    println!("{{\"rest\":{rest},\"face\":[{}],\"velum\":[{}]}}", face.join(","), velum.join(","));
}
