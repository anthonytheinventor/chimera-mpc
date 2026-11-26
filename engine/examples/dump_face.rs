//! The outlines the skin's face is drawn from, as JSON on stdout (tools/make_skin.py reads it):
//! the male body's tract at rest ("rest", for the parts that stay put), at each of the face
//! strip's postures ("face", the same list the plugin picks frames from) and at each soft palate
//! frame ("velum"). Points are body centimeters (x forward, y up).
//!
//!     cargo run --release --example dump_face > ../build/face.json
use chimera::anatomy::geom::Vec2;
use chimera::anatomy::human::{BODY_MALE, HumanArtic, HumanFrame, HumanVocalTract, NEUTRAL};
use chimera_mpc_engine::{VELUM_FRAMES, face_postures, velum_frame_value};

fn pts(v: &[Vec2]) -> String {
    let inner: Vec<String> = v.iter().map(|p| format!("[{:.4},{:.4}]", p.x, p.y)).collect();
    format!("[{}]", inner.join(","))
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
    format!("{{{},\"glottis\":[{:.4},{:.4}]}}", body.join(","), f.glottis.x, f.glottis.y)
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
