# Reopening drawings from Finder

Finder double-click, Open With, and dropping files on the app icon deliver open-document
Apple events rather than command-line arguments or egui window-drop events. The event adapter
is supplied by [PR #683](https://github.com/storytold/cadcraft/pull/683); this follow-up adds no
second adapter, macOS dependency, or document-type registration. Until that adapter lands,
the Finder-opening gap remains open.

The shared `CadApp::open_path` selects an already open drawing through `document.switch`,
retaining its edits, undo history and view. Native symlinks resolve to the same drawing.
Repeated requests while a background read is pending share that read, and its completion
checks again before adding a tab. Recently opened files still move to the top. This applies
to window drops and Open Recent now, and to Finder when its adapter calls `open_path`.
The engine's explicit `OPEN` command is unchanged.

## Regression validation

```sh
cargo test -p cadcraft-ui-egui --test reopen_drawing --test recent_files
```

These tests cover DWG/DXF paths with Chinese names/spaces, dirty tabs, deleted files, native
symlinks, repeated requests while loading, and a drawing opened before a pending read finishes.
The reader in the shared UI tests is synthetic; real format loading is checked below.

The transport regression needs a live macOS GUI session and a packaged app containing both
this follow-up and the event adapter from #683 (or an equivalent):

```sh
CADCRAFT_MACOS_TEST_APP=/absolute/path/to/CADCraft.app \
  cargo test -p cadcraft --test finder_launchservices -- --ignored
```

The test is explicitly ignored by default because it needs a packaged app and GUI session.
It makes a temporary copy with a private bundle identifier and config directory, then sends
a synthetic DWG through LaunchServices on cold launch, a DXF to the running app, and the DWG
again after editing it. File paths are never passed after `--args`. It checks loaded contents,
preserved dirty state and unchanged tab count. The installed app, user drawings and default
file associations are untouched.
