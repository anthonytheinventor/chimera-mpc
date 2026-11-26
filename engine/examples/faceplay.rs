//! Diagnostic: the face frame cm_display reports in Play, as knobs and the vowel pad move.
use chimera_mpc_engine::*;
use std::ffi::CString;
const FS: f64 = 44100.0;
fn main() {
    let dir = CString::new(format!("{}/../Content", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let h = cm_new(FS, dir.as_ptr(), std::ptr::null());
    let mut q = vec![0.0f32; p::COUNT];
    cm_param_defaults(q.as_mut_ptr(), p::COUNT as i32);
    q[p::MODE] = 1.0;
    let mut buf = [0.0f32; 128];
    let mut run = |q: &Vec<f32>, s: f64| -> (i32, i32) {
        let mut shown = [0i32; 4];
        for _ in 0..(s * FS / 128.0) as usize {
            cm_set_params(h, q.as_ptr(), p::COUNT as i32);
            cm_render(h, buf.as_mut_ptr(), 128);
        }
        cm_display(h, shown.as_mut_ptr());
        (shown[0], shown[1])
    };
    println!("rest, no note: {:?}", run(&q, 0.5));
    let d = q.clone();
    for (name, i, v) in [("jaw 2.0", p::JAW, 2.0f32), ("lips 2.0", p::LIP_OPEN, 2.0), ("tongue up 8", p::TONGUE_UP, 8.0), ("velum 1.0", p::VELUM, 1.0)] {
        q = d.clone();
        q[i] = v;
        let a = run(&q, 0.5);
        cm_note_on(h, 48, 100);
        let b = run(&q, 0.5);
        cm_note_off(h, 48);
        let c = run(&q, 0.5);
        println!("{name:<12} no note {a:?}  note held {b:?}  after {c:?}");
    }
    // the vowel pad, as the plugin applies it
    for (name, f, o) in [("pad i", 1.0f32, 0.0f32), ("pad a", 0.5, 1.0), ("pad u", 0.0, 0.0)] {
        q = d.clone();
        let mut a = [0.0f32; 9];
        cm_vowel_posture(h, f, o, a.as_mut_ptr());
        let keys = [p::TONGUE_FWD, p::TONGUE_UP, p::JAW, p::LIP_OPEN, p::PROTRUDE, p::ROUNDING, p::ROOT, p::LARYNX, p::TIP_CURL];
        for k in 0..9 {
            q[keys[k]] = if k == 5 { 1.0 - a[k] } else { a[k] };
        }
        let x = run(&q, 0.5);
        cm_note_on(h, 48, 100);
        let y = run(&q, 0.5);
        cm_note_off(h, 48);
        println!("{name:<12} no note {x:?}  note held {y:?}");
    }
    cm_free(h);
}
