//! A PUBLISH run as a job: it plots a snapshot of the drawings on another thread, reports its
//! progress and can be cancelled; and the sheet stamp, which is text in a corner of each sheet.

use std::sync::atomic::{AtomicBool, Ordering};

use cadcraft_engine::Session;
use cadcraft_engine::cmd::PublishJob;
use cadcraft_engine::cmd::file::{IoHooks, base64_decode, set_io};
use cadcraft_engine::doc::{Drawing, EntityKind, HAlign, Space};
use serde_json::{Value, json};

/// A plotter that lists the texts in the space it plots and how many objects it has.
fn texts_plot(d: &Drawing, space: &Space, _: &Value) -> Result<Vec<u8>, String> {
    let store = d.space(space).ok_or("no such space")?;
    let texts: Vec<Value> = store
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => {
                Some(json!({"text": t.value, "x": t.insert.x, "y": t.insert.y, "h": t.height, "right": t.halign == HAlign::Right}))
            }
            _ => None,
        })
        .collect();
    Ok(json!({"count": store.len(), "texts": texts}).to_string().into_bytes())
}

fn session() -> Session {
    set_io(IoHooks { read: |_, _| Err("no reader in this test".into()), write: |_, _| Err("no writer in this test".into()), plot: Some(texts_plot) });
    let mut s = Session::new();
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [10, 10]]})).unwrap();
    s.execute("layout.set", &json!({"name": "Model"})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [100, 50]]})).unwrap();
    s
}

fn plotted(sheet: &Value) -> Value {
    serde_json::from_slice(&base64_decode(sheet["data"].as_str().unwrap()).unwrap()).unwrap()
}

#[test]
fn a_job_plots_its_snapshot_on_another_thread() {
    let mut s = session();
    let job = PublishJob::new(&s, &json!({"sheets": [{"layout": "Model"}, {"layout": "Layout1"}]})).unwrap();
    assert_eq!(job.len(), 2);
    // Edits made while the job runs aren't in it.
    s.execute("line", &json!({"points": [[0, 0], [5, 5]]})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [6, 6]]})).unwrap();
    let r = std::thread::spawn(move || job.run(&mut |_, _| true)).join().unwrap().unwrap();
    assert_eq!(plotted(&r["sheets"][0])["count"], 1, "the model space it was given: one line");
    assert_eq!(s.doc().unwrap().model.len(), 3, "the drawing kept the edits");
}

#[test]
fn progress_is_reported_and_a_run_can_be_cancelled() {
    let s = session();
    let p = json!({"sheets": [{"layout": "Model"}, {"layout": "Layout1"}, {"layout": "Model"}]});
    let mut seen = Vec::new();
    PublishJob::new(&s, &p)
        .unwrap()
        .run(&mut |done, total| {
            seen.push((done, total));
            true
        })
        .unwrap();
    assert_eq!(seen, [(0, 3), (1, 3), (2, 3), (3, 3)]);
    // Stop after the first sheet: the rest are reported as cancelled.
    let stop = AtomicBool::new(false);
    let r = PublishJob::new(&s, &p)
        .unwrap()
        .run(&mut |done, _| {
            let go = !stop.load(Ordering::SeqCst);
            if done == 1 {
                stop.store(true, Ordering::SeqCst);
            }
            go && done < 1
        })
        .unwrap();
    let ok: Vec<bool> = r["sheets"].as_array().unwrap().iter().map(|x| x["ok"].as_bool().unwrap()).collect();
    assert_eq!(ok, [true, false, false]);
    assert_eq!(r["sheets"][1]["error"], "cancelled");
    assert_eq!(r["cancelled"], true);
    // A status request is a job too, and plots nothing.
    let status = PublishJob::new(&s, &json!({"action": "status", "sheets": [{"layout": "Model"}]})).unwrap();
    assert!(status.is_status());
    assert_eq!(status.status()["sheets"][0]["status"], "default");
}

#[test]
fn the_stamp_goes_in_the_corner_of_each_sheet() {
    let mut s = session();
    let model_before = s.doc().unwrap().model.len();
    let r = s
        .execute(
            "publish",
            &json!({"sheets": [{"layout": "Layout1"}, {"layout": "Model"}], "stamp": {"text": "{n}: {drawing} / {sheet} {date} {{draft}}", "height": 3.5}}),
        )
        .unwrap();
    let layout = plotted(&r["sheets"][0]);
    let t = &layout["texts"][0];
    let text = t["text"].as_str().unwrap();
    assert!(text.starts_with("1: Drawing1 / Layout1 20") && text.ends_with("{draft}"), "{text}");
    let date = text.split(' ').nth(4).unwrap();
    assert!(date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-', "a YYYY-MM-DD date: {date}");
    // Bottom right of the printable area (inches on this imperial drawing): right-aligned, 3.5 mm high.
    assert_eq!(t["right"], true);
    assert!((t["h"].as_f64().unwrap() - 3.5 / 25.4).abs() < 1e-9, "{t}");
    assert!(t["x"].as_f64().unwrap() > 8.0 && t["y"].as_f64().unwrap() < 1.0, "near the bottom right of a Letter sheet: {t}");
    // Model space: just under the drawing's extents, at the left for bottom-left.
    let model = plotted(&r["sheets"][1]);
    assert!(model["texts"][0]["y"].as_f64().unwrap() < 0.0, "{model}");
    let r = s.execute("publish", &json!({"sheets": [{"layout": "Model"}], "stamp": {"corner": "top-left"}})).unwrap();
    let t = &plotted(&r["sheets"][0])["texts"][0];
    assert!(t["y"].as_f64().unwrap() > 50.0 && t["x"].as_f64().unwrap() == 0.0 && t["right"] == false, "{t}");
    assert!(t["text"].as_str().unwrap().starts_with("Drawing1 · Model · "), "the default text: {t}");
    // The drawing itself has no stamp.
    assert_eq!(s.doc().unwrap().model.len(), model_before);
    // A page setup changed for the run moves the stamp with the paper: A3 landscape is wider.
    let r =
        s.execute("publish", &json!({"sheets": [{"layout": "Layout1", "pageSetup": {"paper": "A3", "landscape": true}}], "stamp": true})).unwrap();
    assert!(plotted(&r["sheets"][0])["texts"][0]["x"].as_f64().unwrap() > 15.0, "inside A3's right edge (16.5 in)");
    // Bad stamps are refused.
    for stamp in [
        json!({"text": ""}),
        json!({"text": "x".repeat(201)}),
        json!({"text": "a\nb"}),
        json!({"text": "{who}"}),
        json!({"text": "}"}),
        json!({"corner": "middle"}),
        json!({"height": 0}),
        json!({"height": 51}),
        json!({"colour": "red"}),
        json!(3),
    ] {
        assert!(s.execute("publish", &json!({"sheets": [{"layout": "Layout1"}], "stamp": stamp})).is_err(), "{stamp}");
    }
}

#[test]
fn the_stamp_follows_a_margins_change_and_n_is_the_place_in_the_run() {
    let mut s = session();
    let at = |r: &Value, i: usize| plotted(&r["sheets"][i])["texts"][0].clone();
    let plain = s.execute("publish", &json!({"sheets": [{"layout": "Layout1"}], "stamp": {"corner": "bottom-left"}})).unwrap();
    let wide = s
        .execute(
            "publish",
            &json!({"sheets": [{"layout": "Layout1", "pageSetup": {"margins": [30, 30, 30, 30]}}], "stamp": {"corner": "bottom-left"}}),
        )
        .unwrap();
    let (x0, x1) = (at(&plain, 0)["x"].as_f64().unwrap(), at(&wide, 0)["x"].as_f64().unwrap());
    assert!(x1 > x0 + 0.9, "30 mm margins move the stamp in by about an inch: {x0} → {x1}");
    // {n} is the sheet's place in the run, whether or not an earlier sheet failed.
    let r = s.execute("publish", &json!({"sheets": [{"layout": "Nope"}, {"layout": "Model"}], "stamp": {"text": "{n}"}})).unwrap();
    assert_eq!(at(&r, 1)["text"], "2");
}
