//! PLOT of a layout without a `scale` uses the scale saved in its page setup (issue #1163): the
//! issue's own sequence, through the real plotter.

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, base64_decode, set_io};
use serde_json::{Value, json};

const PT_PER_MM: f64 = 72.0 / 25.4;

/// A metric drawing with layout `Tavola` (A3 landscape, no margins, 1:10 in its page setup)
/// holding a rectangle from (100, 100) to (4100, 2870).
fn session() -> Session {
    set_io(IoHooks {
        read: |_, _| Err("no reader in this test".into()),
        write: |_, _| Err("no writer in this test".into()),
        plot: Some(|d, space, opts| cadcraft_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
    });
    let mut s = Session::empty();
    s.new_drawing(true);
    s.execute("layout.new", &json!({"name": "Tavola", "viewport": false})).unwrap();
    s.execute("layout.set", &json!({"name": "Tavola"})).unwrap();
    let page = json!({"layout": "Tavola", "device": "CADCraft PDF", "paper": "A3", "landscape": true, "scale": 0.1, "margins": [0, 0, 0, 0]});
    s.execute("pagesetup", &page).unwrap();
    s.execute("rectang", &json!({"p1": [100, 100], "p2": [4100, 2870]})).unwrap();
    s
}

/// The plotted rectangle in millimetres on the sheet: (min x, min y, max x, max y).
fn plotted_mm(s: &mut Session, cmd: &str, p: Value) -> [f64; 4] {
    let r = s.execute(cmd, &p).unwrap();
    let pdf = base64_decode(r["data"].as_str().unwrap()).unwrap();
    let text = String::from_utf8_lossy(&pdf).into_owned();
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut n = 0;
    for l in text.lines().filter(|l| l.ends_with(" m") || l.ends_with(" l")) {
        let mut it = l.split(' ');
        let (Some(x), Some(y)) = (it.next().and_then(|v| v.parse::<f64>().ok()), it.next().and_then(|v| v.parse::<f64>().ok())) else {
            continue;
        };
        b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        n += 1;
    }
    assert!(n >= 4, "{text}");
    b.map(|v| v / PT_PER_MM)
}

fn close(got: [f64; 4], want: [f64; 4]) -> bool {
    got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01)
}

#[test]
fn a_layout_plots_at_the_scale_saved_in_its_page_setup() {
    let mut s = session();
    // 1:10 from the page setup: the rectangle runs from 10 to 410 mm on the 420 mm sheet.
    let got = plotted_mm(&mut s, "plot", json!({"layout": "Tavola", "compress": false}));
    assert!(close(got, [10.0, 10.0, 410.0, 287.0]), "{got:?}");
    let got = plotted_mm(&mut s, "exportpdf", json!({"layout": "Tavola", "compress": false}));
    assert!(close(got, [10.0, 10.0, 410.0, 287.0]), "{got:?}");
    // From the current layout as well.
    let got = plotted_mm(&mut s, "plot", json!({"compress": false}));
    assert!(close(got, [10.0, 10.0, 410.0, 287.0]), "{got:?}");
    // A scale given for the plot wins, as before.
    let got = plotted_mm(&mut s, "plot", json!({"layout": "Tavola", "compress": false, "scale": 0.05}));
    assert!(close(got, [5.0, 5.0, 205.0, 143.5]), "{got:?}");
    let got = plotted_mm(&mut s, "plot", json!({"layout": "Tavola", "compress": false, "scale": "1:1"}));
    assert!(close(got, [100.0, 100.0, 4100.0, 2870.0]), "{got:?}");
}

#[test]
fn a_page_setup_that_fits_to_paper_leaves_its_scale_aside() {
    let mut s = session();
    s.execute("pagesetup", &json!({"layout": "Tavola", "scaleToFit": true})).unwrap();
    let got = plotted_mm(&mut s, "plot", json!({"layout": "Tavola", "compress": false}));
    assert!(close(got, [100.0, 100.0, 4100.0, 2870.0]), "{got:?}");
    // Back to the scale.
    s.execute("pagesetup", &json!({"layout": "Tavola", "scaleToFit": false})).unwrap();
    let got = plotted_mm(&mut s, "plot", json!({"layout": "Tavola", "compress": false}));
    assert!(close(got, [10.0, 10.0, 410.0, 287.0]), "{got:?}");
}
