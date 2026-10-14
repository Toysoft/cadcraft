# Drawing history (the OMNI toolkit)

CADCraft keeps a *drawing history* next to the drawing it belongs to: snapshots of the drawing as
it was, a listing of them, and a way to lay an old revision under the current one to see what
changed. It comes from a small AutoLISP toolkit (`OMNI.lsp`) its author used inside AutoCAD, and
is here rebuilt as native commands — no AutoLISP, no `-XREF`/`-LAYER` command-line scripting, and
working the same for DXF and DWG.

Everything is a command, so the command line, the menus (**Tools ▸ Drawing History**), scripts,
the CLI, the control channel and MCP all reach the same code. See
[control-protocol.md](control-protocol.md) for the JSON forms.

![CADCraft with the previous revision of the drawing laid under it in red: the wall outline and the two door openings of the older revision show in red while the current drawing is white, and the command line reads "[OMNI] OMNI_plan_20261014_073841: 4 object(s) on 1 layer(s) laid under the drawing in ACI 1" and "Compared with …walls-and-doors_plan.dxf: 3 only in this drawing, 1 only in the snapshot, 0 changed, 3 the same."](images/ui-drawing-history.png)

## Commands

| Command | Aliases | What it does |
|---|---|---|
| `OMNI-SNAP` | `OMNISNAP` | Writes a snapshot of the drawing as it is right now, into `_OMNI_HISTORY` next to it |
| `OMNI-LOG` | `OMNILOG`, `OMNI` | Lists the snapshots newest first (number, time, size, description, file) and opens one |
| `OMNI-DIFF` | `OMNIDIFF` | Lays a snapshot under the drawing as a recoloured overlay reference, and/or reports what changed |
| `OMNI-TOGGLE` | `OMNITOGGLE` | Shows or hides the snapshot overlay |
| `OMNI-CLEAR` | `OMNICLEAR` | Detaches the overlays `OMNI-DIFF` attached — and nothing else |
| `OMNI-NOTE` | `OMNINOTE` | A marker circle and a comment on the `OMNI_NOTES` layer |

Typed at the command line, `OMNI-SNAP` asks for the description, `OMNI-LOG` and `OMNI-DIFF` list
the history and ask for a number (`0` cancels), and `OMNI-NOTE` asks for the point and then the
comment, rubber-banding the marker as you move.

```text
Command: OMNI-SNAP
Enter a description for this snapshot <snapshot>: walls done
[OMNI] Snapshot saved: 20261014.083512_482_lab_walls-done_plan.dxf (1 in _OMNI_HISTORY).

Command: OMNI-DIFF
[OMNI] Snapshot history (3 in C:\jobs\site\_OMNI_HISTORY):
[  1] 2026-10-14T08:35:12.482Z  21.4 kB  walls-done  20261014.083512_482_lab_walls-done_plan.dxf
...
Enter the number of the snapshot to compare with <1>: 1
[OMNI] OMNI_plan_20261014_083512: 214 object(s) on 6 layer(s) laid under the drawing in ACI 1. Hide it with OMNI-TOGGLE, detach it with OMNI-CLEAR.
```

## Snapshots

A snapshot is a **complete copy of the drawing**, written through the same writer a save uses, so
it is a DXF when the drawing is a DXF and a DWG when it is a DWG. It is taken from the drawing as
it stands, *including changes that are not saved yet* — nothing is written back to the drawing's
own file, which is why `OMNI-SNAP` is not a save and cannot lose unsaved work.

Snapshots live in `_OMNI_HISTORY` in the drawing's folder, named:

```text
<date>.<time>_<milliseconds>_<user>_<description>_<file>

20261014.083512_482_lab_walls-done_plan.dxf
```

- **Date and time** are UTC, so one history folder stays sortable and comparable however many
  machines and time zones it is written from; a plain name sort is a chronological sort.
- **Milliseconds** keep two snapshots taken in the same second apart.
- **User** comes from `CADCRAFT_USER`, `USER`, `USERNAME` or `LOGNAME` — whichever the platform
  sets — so a snapshot says who took it on Windows, Linux, macOS and FreeBSD alike.
- **Description** is folded into a file name (letters and digits of any script survive, so a
  description in Russian is fine; separators and path characters become `-`). It is at most 48
  characters and never contains `_`, which is what makes a name parseable back exactly.
- **File** is the drawing's own name, which is why the drawing is recovered exactly however many
  separators the parts before it had.

`OMNI-LOG` reads what a name says without opening the snapshot; a file in the folder that does not
follow the pattern is still listed, with what its name didn't say left empty.

`OMNI-LOG {open}` loads a revision as a drawing of its own, and that drawing is what the commands
work on from then on: a snapshot lives in `_OMNI_HISTORY`, so *its* history is
`_OMNI_HISTORY/_OMNI_HISTORY`. Name the drawing with `path` (or switch back to it) to keep working
on its history. A revision opens under the snapshot's own file name, so a plain save would write
over the snapshot: use **Save As…** to keep a copy of an old revision.

## Seeing what changed

`OMNI-DIFF` (the default, `mode: "overlay"`) attaches the snapshot as an **overlay reference** at
0,0 — aligned with the drawing, because both are the same drawing — and recolours everything it
brings to a single colour (ACI 1, red, or `color:` any ACI 1–255). Both its layers *and* its
objects: the AutoLISP original recoloured only the layers, and an object carrying a colour of its
own would stay visible in it.

- `OMNI-TOGGLE` shows and hides the overlay by unloading and reloading its reference — the
  reference stays attached either way. The AutoLISP original turned the overlay's *layers* off,
  which leaves behind every object the snapshot kept on layer 0, and an external reference's layer
  0 is the *host's* layer 0 (in AutoCAD as here), so those objects can't be hidden that way. The
  colour the overlay was given stays on its layers, so showing it again brings it back red.
- `OMNI-CLEAR` detaches the overlays and nothing else: an external reference of the user's own
  stays attached, wherever the history folder is and whatever it is called. Overlays are found by
  their `OMNI_` prefix.

`OMNI-DIFF` with `mode: "report"` attaches nothing and instead compares the drawing with the
snapshot through the same machinery `COMPARE` uses (objects matched by geometry and properties,
handle first) and answers with the counts and the areas holding changes:

```json
{"id":1,"method":"engine.execute","params":{"command":"omni-diff","params":{"mode":"report"}}}
→ {"result":{"counts":{"added":12,"removed":3,"changed":5,"common":4961},"regions":[[[0,0],[120,80]]]}}
```

`mode: "both"` does both in one call. A snapshot that is already attached is reused rather than
attached twice, so calling `OMNI-DIFF` again is safe.

## Notes

`OMNI-NOTE` places a circle (radius 50 by default) and the text you type (height 25 by default) on
the `OMNI_NOTES` layer, created red if it doesn't exist yet and left alone if it does — so a
drawing's own note layer keeps its own settings. Both sizes and the layer are parameters, and the
note is one undoable change like any other drawing edit.

## What the port adds over the AutoLISP original

- **DXF as well as DWG.** A snapshot is written and read back through the io layer, so a DXF
  drawing gets a DXF history and an AutoCAD DWG gets a DWG one, and either can be diffed.
- **No file copy on save.** The original ran `QSAVE` and copied the file; a snapshot here is the
  drawing in memory, so it is exact and an unsaved drawing can be snapshotted too.
- **A report, not only an overlay.** `mode: "report"` answers what changed as data, which is what
  an agent or a script wants; the overlay is for the eye.
- **A toggle that always hides.** Unload and reload rather than layer on/off, so an underlay whose
  objects sit on layer 0 cannot be left behind.
- **Nothing platform-specific.** Paths are built with `std::path`, an overlay reference is stored
  relative to the drawing with `/`, the user name comes from the environment and the stamp is UTC.
- **The same on every surface.** The commands are ordinary commands: command line, menus, scripts,
  `cadcraft-cli`, the control channel and MCP. File-system commands report "not available on the
  web" in the browser build; `OMNI-TOGGLE`, `OMNI-CLEAR` and `OMNI-NOTE` work there too.
