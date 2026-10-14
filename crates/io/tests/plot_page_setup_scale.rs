//! A layout plotted without a `scale` plots at the scale saved in its page setup (issue #1163);
//! a `scale` given for the plot still wins, and a page setup that fits to paper keeps 1:1.

use cadcraft_doc::{Common, Drawing, EntityKind, Line, Space};
use cadcraft_geom::Vec3;
use serde_json::{Value, json};

const PT_PER_MM: f64 = 72.0 / 25.4;

/// A metric drawing whose `Layout1` is A3 landscape without margins, with its page setup at
/// `scale` (fitting to paper when `fit`), holding the bottom edge of the issue's rectangle: a
/// line from (100, 100) to (4100, 100). `Layout2` holds the same line at the default page setup.
fn drawing(scale: f64, fit: bool) -> Drawing {
    let mut d = Drawing::new_metric();
    for name in ["Layout1", "Layout2"] {
        let line = EntityKind::Line(Line { a: Vec3::new(100.0, 100.0, 0.0), b: Vec3::new(4100.0, 100.0, 0.0) });
        d.add(&Space::Paper(name.into()), Common::default(), line).unwrap();
    }
    let page = &mut d.layouts[0].page;
    (page.width_mm, page.height_mm, page.landscape, page.margins_mm) = (297.0, 420.0, true, [0.0; 4]);
    (page.scale, page.scale_to_fit) = (scale, fit);
    d
}

/// The x and y (millimetres on the sheet) of every path point in the plot, in order.
fn plotted_mm(d: &Drawing, opts: Value) -> Vec<(f64, f64)> {
    let mut o = json!({"compress": false});
    if let (Some(o), Some(e)) = (o.as_object_mut(), opts.as_object()) {
        o.extend(e.clone());
    }
    let bytes = cadcraft_io::plot(d, &Space::Paper("Layout1".into()), &o).unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let pts: Vec<(f64, f64)> = text
        .lines()
        .filter(|l| l.ends_with(" m") || l.ends_with(" l"))
        .filter_map(|l| {
            let mut it = l.split(' ');
            Some((it.next()?.parse::<f64>().ok()? / PT_PER_MM, it.next()?.parse::<f64>().ok()? / PT_PER_MM))
        })
        .collect();
    assert!(!pts.is_empty(), "{text}");
    pts
}

fn close(got: &[(f64, f64)], want: &[(f64, f64)]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(g, w)| (g.0 - w.0).abs() < 0.01 && (g.1 - w.1).abs() < 0.01)
}

#[test]
fn a_layout_plots_at_the_scale_saved_in_its_page_setup() {
    // Page setup at 1:10: the line runs from 10 to 410 mm, 10 mm up the A3 sheet.
    let got = plotted_mm(&drawing(0.1, false), json!({}));
    assert!(close(&got, &[(10.0, 10.0), (410.0, 10.0)]), "{got:?}");
    // With `fit: false` as the Plot dialog sends it, the same.
    let got = plotted_mm(&drawing(0.1, false), json!({"fit": false}));
    assert!(close(&got, &[(10.0, 10.0), (410.0, 10.0)]), "{got:?}");
    // The page setup's 1:1 plots as before.
    let got = plotted_mm(&drawing(1.0, false), json!({}));
    assert!(close(&got, &[(100.0, 100.0), (4100.0, 100.0)]), "{got:?}");
}

#[test]
fn a_scale_given_for_the_plot_wins_over_the_page_setup() {
    let got = plotted_mm(&drawing(0.1, false), json!({"scale": 0.05}));
    assert!(close(&got, &[(5.0, 5.0), (205.0, 5.0)]), "{got:?}");
    let got = plotted_mm(&drawing(0.1, false), json!({"fit": false, "scale": 1}));
    assert!(close(&got, &[(100.0, 100.0), (4100.0, 100.0)]), "{got:?}");
}

#[test]
fn a_page_setup_that_fits_to_paper_leaves_its_scale_aside() {
    // "Fit to paper" greys the scale out: the one saved with it is not applied.
    let got = plotted_mm(&drawing(0.1, true), json!({}));
    assert!(close(&got, &[(100.0, 100.0), (4100.0, 100.0)]), "{got:?}");
    // Asked to fit, the plot fits the line to the printable area (the A3 sheet, 420 mm wide).
    let got = plotted_mm(&drawing(0.1, true), json!({"fit": true}));
    assert!(got.len() == 2 && (got[1].0 - got[0].0 - 420.0).abs() < 0.01, "{got:?}");
}

#[test]
fn a_hostile_page_setup_scale_plots_1_to_1() {
    for scale in [0.0, -0.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let got = plotted_mm(&drawing(scale, false), json!({}));
        assert!(close(&got, &[(100.0, 100.0), (4100.0, 100.0)]), "scale {scale}: {got:?}");
    }
}

#[test]
fn each_sheet_of_a_multi_sheet_plot_uses_its_own_page_setup_scale() {
    // Layout1 at 1:10, Layout2 at its default 1:1, in one PDF.
    let got = plotted_mm(&drawing(0.1, false), json!({"layouts": ["Layout1", "Layout2"]}));
    assert!(close(&got, &[(10.0, 10.0), (410.0, 10.0), (100.0, 100.0), (4100.0, 100.0)]), "{got:?}");
    // A scale given for the plot applies to every sheet.
    let got = plotted_mm(&drawing(0.1, false), json!({"layouts": ["Layout1", "Layout2"], "scale": 0.5}));
    assert!(close(&got, &[(50.0, 50.0), (2050.0, 50.0), (50.0, 50.0), (2050.0, 50.0)]), "{got:?}");
}
