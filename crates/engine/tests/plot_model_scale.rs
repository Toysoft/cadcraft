//! A model-space plot given a `scale` and no `fit` plots at that scale, as choosing a scale in
//! the Plot dialog turns off Fit to paper (issue #1226).

use cadcraft_doc::{Drawing, Space};
use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, base64_decode, set_io};
use serde_json::{Value, json};

fn real_plot(d: &Drawing, space: &Space, opts: &Value) -> Result<Vec<u8>, String> {
    cadcraft_io::plot(d, space, opts).map_err(|e| e.to_string())
}

/// Width in millimetres of what `exportpdf` draws for a 1000 × 1000 square on A3.
fn square_mm(s: &mut Session, extra: Value) -> f64 {
    let mut p = json!({"paper": "A3", "compress": false});
    if let (Some(p), Some(e)) = (p.as_object_mut(), extra.as_object()) {
        p.extend(e.clone());
    }
    let r = s.execute("exportpdf", &p).unwrap();
    let text = String::from_utf8_lossy(&base64_decode(r["data"].as_str().unwrap()).unwrap()).into_owned();
    let xs: Vec<f64> = text.lines().filter(|l| l.ends_with(" m") || l.ends_with(" l")).filter_map(|l| l.split(' ').next()?.parse().ok()).collect();
    assert!(!xs.is_empty(), "{text}");
    let (lo, hi) = xs.iter().fold((f64::MAX, f64::MIN), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
    (hi - lo) * 25.4 / 72.0
}

#[test]
fn model_space_export_plots_at_the_given_scale() {
    set_io(IoHooks { read: |_, _| Err("no reader in this test".into()), write: |_, _| Err("no writer in this test".into()), plot: Some(real_plot) });
    let mut s = Session::new();
    s.open_drawing(Drawing::new_metric(), "square", None);
    s.execute("rectang", &json!({"p1": [0, 0], "p2": [1000, 1000]})).unwrap();
    for (extra, mm) in [
        (json!({"scale": "1:100"}), 10.0),
        (json!({"scale": "1:10"}), 100.0),
        (json!({"scale": "1:10", "fit": false}), 100.0),
        // An explicit `fit: true` still fits, scale or not.
        (json!({"scale": "1:10", "fit": true}), 284.3),
        (json!({}), 284.3),
    ] {
        let got = square_mm(&mut s, extra.clone());
        assert!((got - mm).abs() < 0.5, "{extra}: {got} mm, want {mm}");
    }
}
