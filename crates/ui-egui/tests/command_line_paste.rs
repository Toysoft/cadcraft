//! System-clipboard events must reach the right input owner, not stale copied objects.

use cadcraft_engine::Session;
use cadcraft_ui_egui::{CadApp, Services};
use egui::{Event, Key, Modifiers};
use serde_json::json;

fn frame(app: &mut CadApp, ctx: &egui::Context, events: Vec<Event>) {
    let input =
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))), events, ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
}

#[test]
fn drawing_paste_consumes_dxf_for_own_and_foreign_windows_including_paste_as_block() {
    use cadcraft_engine::cmd::{clipboard, file};
    file::set_io(file::IoHooks {
        read: |bytes, name| cadcraft_io::read(bytes, name).map_err(|e| e.to_string()),
        write: |drawing, name| cadcraft_io::write(drawing, name).map_err(|e| e.to_string()),
        plot: None,
    });
    for foreign in [false, true] {
        for as_block in [false, true] {
            let mut app = CadApp::new(Session::new(), Services::default());
            app.run("line", json!({"points": [[0, 0], [10, 0]]})).unwrap();
            app.session.set_selection(app.session.doc().unwrap().model.handles());
            let mut ctx = egui::Context::default();
            frame(&mut app, &ctx, Vec::new());
            frame(&mut app, &ctx, vec![Event::Copy]);
            assert_eq!(app.session.clipboard.len(), 1);
            frame(&mut app, &ctx, Vec::new()); // mirror the copy to this window's system clipboard
            let dxf = clipboard::system_text(&app.session).unwrap().replace("\r\n", "\n");
            if foreign {
                app = CadApp::new(Session::new(), Services::default());
                ctx = egui::Context::default();
                frame(&mut app, &ctx, Vec::new());
            }
            let before = app.session.doc().unwrap().model.len();
            let modifiers = if as_block { Modifiers::SHIFT } else { Modifiers::NONE };
            frame(&mut app, &ctx, vec![Event::ModifiersChanged(modifiers), Event::Paste(dxf)]);
            assert_eq!(app.session.running.as_ref().map(|r| r.id.as_str()), Some(if as_block { "pasteblock" } else { "pasteclip" }));
            assert!(app.cmd.buffer.is_empty(), "DXF must not leak into the command line");
            app.cmdline("20,30");
            assert_eq!(app.session.doc().unwrap().model.len(), before + 1);
            let handle = app.session.selection().into_iter().next().unwrap();
            let entity = app.session.doc().unwrap().model.get(handle).unwrap();
            assert_eq!(matches!(entity.kind, cadcraft_doc::EntityKind::Insert(_)), as_block);
        }
    }
}

fn enter() -> Event {
    Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
}

#[test]
fn pasted_command_waits_for_enter_even_with_hidden_command_line_and_stale_objects() {
    for visible in [true, false] {
        let mut app = CadApp::new(Session::new(), Services::default());
        app.ui.show_command_line = visible;
        app.run("line", json!({"points": [[0, 0], [10, 0]]})).unwrap();
        let handles = app.session.doc().unwrap().model.handles();
        app.run("copyclip", json!({"handles": handles.iter().map(|h| h.hex()).collect::<Vec<_>>()})).unwrap();
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, Vec::new());
        let text = "zoom w 620800,3825200 626300,3829500";
        // Some frontends deliver both the key and clipboard payload in the same frame.
        let modifiers = Modifiers::COMMAND;
        frame(
            &mut app,
            &ctx,
            vec![Event::Key { key: Key::V, physical_key: None, pressed: true, repeat: false, modifiers }, Event::Paste(text.into())],
        );
        assert_eq!(app.cmd.buffer, text, "visible={visible}");
        assert!(app.session.running.is_none(), "paste must not start PASTECLIP or execute the command");
        assert_eq!(app.session.doc().unwrap().model.len(), 1);
        assert_eq!(app.session.clipboard.len(), 1, "text must not replace the drawing clipboard");

        frame(&mut app, &ctx, vec![enter()]);
        assert!(app.cmd.buffer.is_empty());
        assert_eq!(app.cmd.history.last().map(String::as_str), Some(text));
        let view = app.session.state().unwrap().view();
        assert!((view.center.x - 623550.0).abs() < 1e-6);
        assert!((view.center.y - 3827350.0).abs() < 1e-6);
    }
}

#[test]
fn paste_appends_unicode_and_whitespace_without_submitting_or_picking_options() {
    let mut app = CadApp::new(Session::new(), Services::default());
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, Vec::new());
    app.cmdline("line 0,0");
    frame(&mut app, &ctx, vec![Event::Key { key: Key::ArrowDown, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    assert!(app.cmd.options.is_some());
    app.cmd.buffer = "前缀".into();
    frame(&mut app, &ctx, vec![Event::Paste(" 缺陷\t说明\r\n第二行\r第三行\n".into())]);
    assert_eq!(app.cmd.buffer, "前缀 缺陷 说明 第二行 第三行 ");
    assert!(app.cmd.options.is_none(), "pasting is typing, not accepting the highlighted keyword");
    assert_eq!(app.session.running.as_ref().map(|r| r.id.as_str()), Some("line"));
    assert_eq!(app.session.doc().unwrap().model.len(), 0);
    assert!(app.cmd.history.is_empty());
}

#[test]
fn oversized_paste_leaves_the_command_and_buffer_unchanged() {
    let mut app = CadApp::new(Session::new(), Services::default());
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, Vec::new());
    app.cmdline("line 0,0");
    app.cmd.buffer = "10,".into();
    frame(&mut app, &ctx, vec![Event::Paste("x".repeat(1024 * 1024))]);
    assert_eq!(app.cmd.buffer, "10,");
    assert_eq!(app.session.running.as_ref().map(|r| r.id.as_str()), Some("line"));
    assert!(app.session.log.iter().any(|line| line.contains("Clipboard text is too long")));
    assert_eq!(app.session.doc().unwrap().model.len(), 0);
}

#[test]
fn focused_text_edit_keeps_its_paste_event() {
    let mut app = CadApp::new(Session::new(), Services::default());
    let ctx = egui::Context::default();
    let mut text = String::new();
    let mut edit_frame = |events| {
        let mut out = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
            app.logic(ui.ctx());
            let response = ui.text_edit_singleline(&mut text);
            response.request_focus();
            cadcraft_ui_egui::cmdline::keyboard(&mut app, ui.ctx());
        });
        out.textures_delta.clear();
    };
    edit_frame(Vec::new());
    edit_frame(vec![Event::Paste("缺陷说明".into())]);
    assert_eq!(text, "缺陷说明");
    assert!(app.cmd.buffer.is_empty());
    assert!(app.session.running.is_none());
}
