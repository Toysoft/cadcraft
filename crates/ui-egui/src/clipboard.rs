//! The system clipboard: Cut/Copy/Paste key presses, and the drawing clipboard mirrored as DXF
//! text so it can be pasted into another CADCraft window.
//!
//! egui turns Cmd/Ctrl+C, X and V into `Copy`, `Cut` and `Paste(text)` events instead of key
//! presses, so the clipboard shortcuts are read from those. A `Paste` arrives only when the system
//! clipboard holds text; every copy therefore puts its DXF text there.

use std::hash::{DefaultHasher, Hash, Hasher};

use cadcraft_engine::cmd::clipboard;

use crate::CadApp;

/// egui memory: the clipboard copy last mirrored, and the hash of the text it became.
fn serial_id() -> egui::Id {
    egui::Id::new("cc_clipboard_serial")
}
fn text_id() -> egui::Id {
    egui::Id::new("cc_clipboard_text")
}
/// egui memory: the hash of the table cells' text last put on the system clipboard.
fn cells_id() -> egui::Id {
    egui::Id::new("cc_clipboard_cells")
}

/// Line endings normalised (pasted text arrives with `\n`), so our own text is recognised.
fn hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.replace("\r\n", "\n").hash(&mut h);
    h.finish()
}

/// Put a new copy (COPYCLIP, CUTCLIP, COPYBASE from anywhere, or table cells) on the system
/// clipboard, once.
pub fn mirror(app: &mut CadApp, ctx: &egui::Context) {
    if let Some(text) = app.canvas.cell_text.take() {
        ctx.data_mut(|d| d.insert_temp(cells_id(), hash(&text)));
        ctx.copy_text(text);
    }
    let Some(src) = &app.session.clipboard_source else { return };
    if src.uid.is_none() || ctx.data(|d| d.get_temp::<u64>(serial_id())) == Some(src.serial) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(serial_id(), src.serial));
    if let Some(text) = clipboard::system_text(&app.session) {
        ctx.data_mut(|d| d.insert_temp(text_id(), hash(&text)));
        ctx.copy_text(text);
    }
}

/// The commands the Cut/Copy/Paste events of this frame ask for. Pasted DXF text that isn't our
/// own last copy (another CADCraft's) becomes the clipboard first.
pub fn shortcut_commands(app: &mut CadApp, ctx: &egui::Context) -> Vec<&'static str> {
    let (events, shift) = ctx.input(|i| {
        let ev: Vec<egui::Event> =
            i.events.iter().filter(|e| matches!(e, egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_))).cloned().collect();
        (ev, i.modifiers.shift)
    });
    let mut out = Vec::new();
    // On picked table cells: the cells are copied, cut and pasted (objects copied in a CADCraft
    // still paste as objects).
    let cells = app.canvas.cells.is_some() && !app.session.prompting();
    for e in events {
        match e {
            egui::Event::Copy | egui::Event::Cut if cells => crate::table_cells::copy(app, matches!(e, egui::Event::Cut)),
            egui::Event::Paste(text) if cells && !text.trim_start().starts_with("0\n") && !text.trim_start().starts_with("0\r\n") => {
                let own = ctx.data(|d| d.get_temp::<u64>(cells_id())) == Some(hash(&text));
                crate::table_cells::paste(app, (!own).then_some(text.as_str()));
            }
            egui::Event::Copy => out.push(if shift { "copybase" } else { "copyclip" }),
            egui::Event::Cut => out.push("cutclip"),
            egui::Event::Paste(text) => {
                let h = hash(&text);
                if ctx.data(|d| d.get_temp::<u64>(text_id())) != Some(h) && clipboard::load_system_text(&mut app.session, &text) {
                    ctx.data_mut(|d| d.insert_temp(text_id(), h));
                }
                out.push(if shift { "pasteblock" } else { "pasteclip" });
            }
            _ => {}
        }
    }
    out
}
