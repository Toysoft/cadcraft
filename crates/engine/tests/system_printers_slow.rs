//! A slow system printer listing (CUPS can take seconds, or hang) never blocks what draws the
//! device list: it runs in the background, and commands that name a printer or report the
//! devices wait for it.

use std::time::{Duration, Instant};

use cadcraft_doc::{DevicePaper, Drawing, PlotterConfig, Space};
use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io};
use cadcraft_engine::cmd::plotters::{
    PlotterStore, PrintJob, SystemPrinters, devices, set_plotter_store, set_system_printers, system_printers_pending,
};
use serde_json::{Value, json};

const LISTING: Duration = Duration::from_millis(1500);

fn slow_printers() -> Vec<PlotterConfig> {
    std::thread::sleep(LISTING);
    vec![PlotterConfig {
        name: "Slow_Queue".into(),
        default_paper: "Letter".into(),
        papers: vec![DevicePaper { name: "Letter".into(), width: 215.9, height: 279.4, margins: [5.0; 4] }],
        system: true,
        ..PlotterConfig::default()
    }]
}

fn print(j: &PrintJob) -> Result<String, String> {
    Ok(format!("{}-1", j.printer))
}

fn echo_plot(_: &Drawing, _: &Space, opts: &Value) -> Result<Vec<u8>, String> {
    Ok(opts.to_string().into_bytes())
}

#[test]
fn a_slow_listing_never_blocks_the_device_list() {
    set_plotter_store(PlotterStore::memory());
    set_system_printers(SystemPrinters { list: slow_printers, print });
    set_io(IoHooks { read: |_, _| Err("no reader in this test".into()), write: |_, _| Err("no writer in this test".into()), plot: Some(echo_plot) });

    // What a dialog asks every frame comes back at once, without the printer while it is listed.
    let start = Instant::now();
    for _ in 0..100 {
        assert!(!devices().iter().any(|d| d.name == "Slow_Queue"));
    }
    assert!(start.elapsed() < LISTING / 3, "devices() waited {:?}", start.elapsed());
    assert!(system_printers_pending());

    // A plot naming the printer waits for the listing and prints.
    let mut s = Session::new();
    s.cmdline("line 0,0 100,0").unwrap();
    s.cmdline("").unwrap();
    let r = s.execute("plot", &json!({ "layout": "Model", "device": "Slow_Queue" })).unwrap();
    assert_eq!(r["job"], "Slow_Queue-1");
    assert!(!system_printers_pending());
    // Then the device list has it, still without waiting.
    let start = Instant::now();
    assert!(devices().iter().any(|d| d.name == "Slow_Queue"));
    assert!(start.elapsed() < LISTING / 3);
}
