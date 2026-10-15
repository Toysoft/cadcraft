# UI parity

> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-14 · **Change:** major (every row re-checked against the source; areas re-scored) · **Target:** Autodesk AutoCAD 2027 for Mac (26.0)

How CADCraft's interaction compares with AutoCAD's: point entry, object snaps, tracking, dynamic
input, grips, selection, the command line, chrome and navigation. A drafter's speed in AutoCAD
comes from these, so "the command exists" isn't enough: it has to accept the same input, snap to
the same points and respond to the same keys. Work items: [gaps.md](gaps.md#uiux).

**Headline (estimated): UI/UX fidelity ≈ 69%.** Point entry (FROM, M2P, TK, point filters, typed
snap overrides), the running snaps with tracking, PolarSnap, editable dynamic input with grip
value fields and the options menu, hot-grip options, selection keywords, context menus, named
views, tiled viewports and the Options dialog are real. Missing: lasso and selection cycling,
hover grip menus and multifunctional grips beyond polylines, lines and arcs, the AutoSnap magnet,
workspaces, the navigation bar, CUI customisation, ViewCube dragging, most system variables and
several dialogs (DIMSTYLE manager, Geometric Tolerance, Object Grouping). Remaining:
**≈ 60–100 Opus 5.5 hours** (estimated from the rows below and [gaps.md](gaps.md#uiux)).

## How this was measured

- CADCraft: read from the source on 2026-10-14 (main at e707ab4): `crates/engine/src/{prompt,
  pointmod, snap, select, grips, grip_dims, undoctl, sysvars, lib, units}.rs`, `snap/*`, `cmd/*`,
  `crates/ui-egui/src/*`. Not run in this pass. The area percentages are judgements from the
  rows, re-scored on 2026-10-14.
- AutoCAD: behaviour observed black-box in the running AutoCAD 2027 for Mac
  (`plan/autocad/01-observed-ui.md`, local only, with reference screenshots that are never
  committed), its menu tree (`xtask/data/menu-catalog.txt`) and Autodesk's documentation.
- Weights (how often a drafter hits each area): point entry 15%, object snaps 15%, tracking and
  polar 10%, dynamic input 10%, grips 10%, selection 10%, command line 10%, chrome and palettes
  10%, navigation 5%, keyboard and context menus 5%.

| Area | Weight | CADCraft | Weighted |
|---|---:|---:|---:|
| Point entry | 15% | 80% | 12.0 |
| Object snaps | 15% | 65% | 9.8 |
| Tracking, polar, ortho, grid/snap | 10% | 85% | 8.5 |
| Dynamic input | 10% | 70% | 7.0 |
| Grips | 10% | 65% | 6.5 |
| Selection | 10% | 60% | 6.0 |
| Command line | 10% | 70% | 7.0 |
| Chrome and palettes | 10% | 55% | 5.5 |
| Navigation | 5% | 75% | 3.8 |
| Keyboard shortcuts, context menus | 5% | 60% | 3.0 |
| **Total** | 100% | | **≈ 69%** |

## Point entry (≈ 80%)

| Input | AutoCAD | CADCraft | Evidence |
|---|---|---|---|
| `x,y[,z]`, `@dx,dy`, `@d<a`, `d<a`, `#x,y`, bare `@` | yes | yes; `#` equals absolute because there is no UCS; Z parsed then discarded | `prompt.rs:147-180` |
| `*x,y` (WCS override) | yes | yes (`*x,y`, `*@dx,dy`, `@*dx,dy`); the same as the plain forms because there is no UCS | `prompt.rs` |
| Direct distance entry | yes, honours ortho, polar and tracking paths | yes, ortho; polar only through the UI's snapped cursor | `lib.rs:782-793` |
| Angle forms: `45d30'`, radians, grads, surveyor `N45dE` | yes | yes | `units.rs:141-197` |
| Architectural/engineering distances `5'6-1/2"` | yes | yes | `units.rs:93-140` |
| `FROM` (base point + offset) | yes | yes: "Base point:", "<Offset>:" with `@` measured from the base; nests with M2P and filters | `pointmod.rs` |
| `M2P` / `MTP` (midpoint between two points) | yes | yes | `pointmod.rs` |
| `TK` (tracking) and `TT` (temporary track point) | yes | yes; `TT` also from the snap menu | `pointmod.rs`, `snap/tracking.rs` |
| Point filters `.x`, `.y`, `.xy`, `.z` | yes | yes (`.x` … `.yz`, "(need YZ):"); Z is asked for and dropped (no 3D) | `pointmod.rs` |
| Angle override `<45` | yes | yes: `<a`, `<<a`, `<<<a`; a typed distance or a pick follows the angle, typed coordinates win | `pointmod.rs` |
| Object snap overrides (`END`, `MID`, `INT` … `NON`) | yes | yes, every mode and `NON`, with long names and `_`/`'` prefixes, for the next pick only; a typed point is snapped within the aperture; a command's own keyword wins (`_cen` always snaps); also from the snap menu | `pointmod.rs` |
| Typed calculator `'CAL` | yes | CAL evaluates an expression and the QuickCalc palette exists; **not usable transparently inside a point prompt** | `cmd/utility.rs`, `palettes/quickcalc.rs` |

## Object snaps (≈ 65%)

| Mode | AutoCAD | CADCraft (`snap.rs`, `snap/curves.rs`) |
|---|---|---|
| Endpoint | every curve, solid/3D vertices, dimension ends | lines, arcs, polyline segments, open ellipses and splines, ray base, solid/trace corners |
| Midpoint | every curve incl. ellipse/spline arcs, polyline segments | segments, elliptical arcs and splines |
| Center | arcs, circles, ellipses, polyarcs | arcs, circles, ellipses |
| Geometric center | any closed polyline, spline | closed LWPOLYLINE and closed splines |
| Node | points, dimension definition points, text alignment points | POINT and dimension definition points; no text alignment points |
| Quadrant | circles, arcs, ellipses | yes |
| Intersection | every pair, extended intersection | among up to 200 nearby primitives; ellipse/spline pairs approximate ([geometry-parity.md](geometry-parity.md)) |
| Extension | yes, with dashed extension path | yes: rest on a line or arc end, then the line runs on (a ray) and the arc round its circle; dotted path and "Extension: d < a°" (`snap/tracking.rs`); not from ellipse, spline or polyline-arc ends |
| Insertion | text, mtext, blocks, attributes, attdefs | TEXT, MTEXT, INSERT, ATTDEF, MLEADER and tolerance frames |
| Perpendicular | every curve, deferred | lines, arcs, circles, xlines, ellipses and splines; deferred for the first point (#168) |
| Tangent | circles, arcs, ellipses, splines, deferred | arcs, circles, ellipses and splines; deferred for the first point on arcs and circles only |
| Nearest | every curve | yes |
| Apparent intersection | projected intersection | yes in 2D: where objects near the cursor would meet if extended; an object rested on sends its whole extension, so its crossing with another object snaps too (`snap.rs` `osnap_with`, `snap/tracking.rs`) |
| Parallel | yes, with parallel path | yes: rest on a line, then a path parallel to it runs through the base point (`snap/tracking.rs`) |
| Temporary overrides typed at a prompt (`END`, `MID`, `INT` …) | yes | yes, every mode and `NON`, for the next pick only (#393, `pointmod.rs`) |
| Shift/Ctrl+right-click snap menu | yes | yes: Temporary track point (#441), From, Mid Between 2 Points, Point Filters, the snap modes, None, Osnap Settings (`context_menu.rs`) |
| Tab to cycle candidate snaps | yes | through the closest snap of each running mode under the cursor, **not every candidate** (`pointmod::snap_candidates`) |
| AutoSnap marker, tooltip, magnet, aperture box | yes | marker per mode and name label; aperture from APERTURE; no magnet, no aperture box |
| OSNAPZ, 3D object snaps | yes | no |

## Tracking, polar, ortho, grid and snap (≈ 85%)

| Feature | AutoCAD | CADCraft |
|---|---|---|
| Ortho (F8) | yes | yes (`snap.rs:503`) |
| Polar tracking increment | yes | yes, measured from ANGBASE; dotted path and "Polar: d < a°" tooltip (`snap/tracking.rs`, `canvas.rs` `draw_tracking`) |
| Additional polar angles, relative to last segment | yes | yes: additional angles (POLARADDANG with POLARMODE 4) and angles measured from the last segment when the base point ends it (`snap/tracking.rs`) |
| Polar distance snap (PolarSnap) | yes | yes: SNAPTYPE 1 snaps along tracking paths at POLARDIST steps; Drafting Settings sets both |
| **Object snap tracking** (acquire points, alignment paths, intersections of paths) | yes, F11 | yes (#379): resting on a snap point acquires it (`+`), resting again releases it, seven at most; horizontal/vertical paths, or every polar angle with POLARMODE 2; Shift to acquire with POLARMODE 8; crossings of two paths, a path and an object, and a path and a polar path snap; tooltip names each path; a typed distance (direct distance entry) runs along the path the cursor is on, measured from the point the path comes from. Engine: `Session::snap_cursor`, `snap/tracking.rs`; AUTOSNAP bits 8 and 16 |
| Grid (rectangular, major lines, adaptive, limits) | yes | rectangular and isometric (30°, 90° and 150° lines), major lines, adaptive display |
| Snap (rectangular, isometric, polar snap) | yes | rectangular and isometric (SNAPSTYL 1: the snap Y spacing along the isometric axes), from SNAPBASE turned by SNAPANG |
| Isometric drafting (ISODRAFT, isoplanes F5) | yes | yes (#556): ISODRAFT and the status-bar toggle set SNAPSTYL and the plane; ISOPLANE / F5 / Ctrl+E cycle Left, Top, Right (SNAPISOPAIR); ortho, direct distance entry and the crosshair follow the plane's axes; ELLIPSE Isocircle (radius or Diameter, also for elliptical arcs); Drafting Settings snap style radio. Engine: `snap/iso.rs`, `cmd/settings/iso.rs` |
| Drafting Settings dialog | yes | yes (`dialogs.rs:36-80`) |
| Hatch and Gradient / Hatch Edit dialogs | yes | yes (`hatchdlg.rs`): pattern swatches, gradient colours and shapes, origin, pick points or objects, associative, separate, island detection; opened by HATCH, GRADIENT (HPDLGMODE), HATCHEDIT, right-click or double-clicking a hatch; no Preview or Inherit Properties |

## Dynamic input (≈ 70%)

AutoCAD's heads-up input (F12) has editable pointer-input fields (x, y), dimensional input
fields (length and angle) with Tab between them and lock icons, relative-by-default second
points, dimensional input on grips, a prompt with clickable options and a down-arrow menu.
CADCraft has pointer input boxes since #60 (`crates/ui-egui/src/dyninput.rs`): value boxes
beside the cursor while a command asks for a point; second and later points default to polar
distance and angle (DYNPIFORMAT = 1 for Cartesian); `,` and `<` lock a value and move to the next
box; Tab types the separator; Enter sends a relative entry (DYNPICOORDS = 1 for absolute) and
fills an empty box from the cursor; a plain number stays direct distance entry. Dimensional
input on grips is there (`grip_dyn.rs`, `engine/src/grip_dims.rs`: the dimensions a stretched grip
changes show as value fields, Tab locks one and moves on; DYNDIGRIP and DYNDIVIS), and so is the
prompt beside the cursor with its Down Arrow options menu (DYNPROMPT). Missing: lock icons on
locked fields, and dimensional input fields for the second point of a command (the pointer boxes
show distance and angle instead).

## Grips (≈ 65%)

| Feature | AutoCAD | CADCraft |
|---|---|---|
| Stretch grips per object type | yes | lines (midpoint moves), arcs (start/mid/end), ellipses (axes), polylines (vertices; segment midpoints move or reshape the bulge), dimensions (text and definition points), MLEADER, viewport, wipeout, spline (`grips.rs:79-270`) |
| Text/MText grips | position, width, column grips | move the whole object |
| Hot-grip modes with Space/Enter cycling | Stretch, Move, Rotate, Scale, Mirror | yes (`canvas.rs:58-67`) |
| Hot-grip options: Base point, Copy, Undo, Reference, eXit | yes | yes (#425, `cmd/griphot.rs`) |
| Several hot grips at once (Shift-click) | yes | yes; STRETCH moves them together (`cmd/griphot.rs`) |
| Multifunctional grip menus (Add Vertex, Remove Vertex, Convert to Arc, Lengthen …) | on hover, Ctrl cycles the options | on right-click only, for polylines, lines and arcs (#425); **no hover menu, no Ctrl cycling, none on dimensions, MLEADER, hatches or text**; Add Vertex on an arc segment straightens it |
| Grip dimensional input (length/angle fields) | yes | yes (`grip_dyn.rs`) |
| Dynamic block grips | yes | yes for references read from other files: distance arrows, position squares, visibility and lookup lists (`dyngrips.rs`); no authoring |
| Grip appearance (GRIPSIZE, colours, hover) | yes | GRIPSIZE, GRIPS and GRIPOBJLIMIT honoured (Options ▸ Selection); grip colours come from the theme, **not GRIPCOLOR/GRIPHOT/GRIPHOVER** |

## Selection (≈ 60%)

| Feature | AutoCAD | CADCraft |
|---|---|---|
| Pick, implied window (left→right) and crossing (right→left) | yes | yes, coloured and dashed rectangles (`lib.rs:576-589`, `canvas.rs:823-836`), filled as SELECTIONAREA, WINDOWAREACOLOR, CROSSINGAREACOLOR and SELECTIONAREAOPACITY say (Visual Effect Settings) |
| Keywords W, C, WP, CP, F, BOX, AU, SI, M, G (group), CL (class) | yes | yes except CLass: W, C, F, WP, CP (#226); BOX, AUto, SIngle, Multiple, Group, Add, Remove, Undo (#428); ALL, Last, Previous |
| Press-drag selection windows (PICKDRAG) | yes | yes (#63, PICKDRAG = 2) |
| Lasso | yes | **no** |
| Selection cycling (SELECTIONCYCLING) | yes | **no**: the toggle is stored and nothing reads it |
| Selection preview, hover highlight | yes | selected and hovered objects redrawn in the selection colour, fills half see-through (SELECTIONEFFECT 1; 0 dashed), selection effect color; HIGHLIGHT (not saved), SELECTIONPREVIEW, PREVIEWEFFECT, PREVIEWFILTER (Visual Effect Settings) |
| PICKADD, PICKFIRST, PICKAUTO | yes | PICKADD (Shift), PICKFIRST |
| QSELECT, SELECTSIMILAR, FILTER | yes | QSELECT dialog; SELECTSIMILAR with SELECTSIMILARMODE (#428, SEttings asks for the bit code; no dialog); no FILTER |
| Object isolate/hide (ISOLATEOBJECTS, HIDEOBJECTS) | yes | yes, temporary (OBJECTISOLATIONMODE 0; #428): Tools > Isolate, UNISOLATEOBJECTS; no status-bar button |
| Groups selectable as one (PICKSTYLE) | yes | yes (PICKSTYLE 1, #417), Object grouping on Options ▸ Selection; associative hatch selection (bit 2) is stored, not used |

## Command line (≈ 70%)

| Feature | AutoCAD | CADCraft |
|---|---|---|
| AutoComplete (commands, sysvars, mid-string, synonyms) | yes | commands and aliases by prefix, Tab to accept (`cmdline.rs` `suggestions`); **no system variables, no mid-string search** |
| Aliases | about 250 in `acad.pgp` | 144 aliases across the commands (counted from source) |
| History (F2), Up/Down recall | yes | yes |
| Transparent commands (`'zoom`, `'pan`, `'osnap`) | yes | yes (`lib.rs:496-503`) |
| Enter/Space repeats last command | yes | yes |
| Clickable keywords in the prompt | yes | yes (`cmdline.rs:253-267`) |
| UNDO with Mark, Back, BEgin, End, Auto, Control | yes | yes, with MREDO and UNDOCTL (#461, `undoctl.rs`) |
| SETVAR for every system variable | ~1,000 sysvars | about 150 session and header variables (`sysvars.rs`, `sysvars/table.rs`); a bare variable name typed as a command runs SETVAR on it |
| Scripts (SCRIPT, `.scr`) | yes | yes: SCRIPT runs a `.scr` file through the same script engine as CLI/MCP/control (#693); nested scripts refused |
| Floating/docked command window, transparency, lines of history | yes | floating overlay, history lines setting |

## Chrome and palettes (≈ 55%)

| Feature | AutoCAD for Mac | CADCraft |
|---|---|---|
| Menu bar | 11 menus, 491 leaf items | same 11 menus; 280/491 items covered by `cargo xtask parity` (see [parity-checklist.md](parity-checklist.md)) |
| Tool Bar, File tabs, Start tab, layout tabs | yes | yes |
| Tool Sets palette (Drafting, Modeling, … with flyouts) | yes | yes for 2D groups |
| Properties Inspector, Quick Properties | yes | yes (every control acts or is visibly unavailable, #182); block references show read-only Position X/Y/Z from their stored insertion point; coordinate editing is still missing |
| Layer Properties Manager (filters, states, VP overrides) | yes | yes |
| Blocks palette, Content | yes | Blocks window (click to insert, #28) |
| QuickCalc palette | yes | yes (#563); history and variables last for the session |
| Status bar toggles | ~25 | 14 buttons; Workspace Switching does nothing; no isolate-objects button |
| ViewCube | interactive 3D | clickable for 2D (Top/Home → extents); dragging does nothing (#7) |
| Navigation bar, SteeringWheels | yes | no |
| Workspaces | Drafting & Annotation, 3D Modeling | no |
| Options / Preferences dialog | many tabs | yes (#23): Display, Open and Save, Drafting, Selection, User Preferences, Profiles; left: right-click customisation, autosave interval and default save version, AutoSnap magnet and tooltip toggles, other drawing-area colours, named profiles with import and export |
| Customisation (CUI, aliases editor, tool palettes) | yes | no (user `.pgp` alias files are read, #552) |
| Dialogs still missing | DIMSTYLE manager, Geometric Tolerance, Object Grouping, MATCHPROP Settings, SELECTSIMILAR Settings, data extraction wizard, APPLOAD with Startup Suite | none of these; the commands run on the command line. Hatch dialogs have no Preview or Inherit Properties |
| In-place MTEXT editor | yes | new MTEXT only (#532); **existing MTEXT can't be reopened in it**; no justification or columns controls |
| Themes | Dark, Light | Dark, Light, System (#179) |
| Accessibility | VoiceOver basics | AccessKit through eframe; screen-reader coverage untested; magnifier issue #39; animation effects follow the OS setting (View ▸ Accessibility ▸ Animation Effects: System, On, Off) |

## Navigation (≈ 75%)

| Feature | AutoCAD | CADCraft |
|---|---|---|
| ZOOM Extents, All, Window, Previous, In/Out, Center, Scale, Object | yes | yes, 50-entry history (`cmd/view.rs:107-149`) |
| ZOOM Realtime (drag), Dynamic | yes | yes (#463, `cmd/view.rs`) |
| Wheel zoom about cursor, ZOOMFACTOR, ZOOMWHEEL | yes | yes with ZOOMFACTOR; no ZOOMWHEEL |
| Middle-drag pan, double-middle-click extents | yes | yes (`canvas.rs:538-583`) |
| Trackpad two-finger pan, pinch zoom (macOS) | yes | yes (#58) |
| Named views (VIEW) | yes | yes (#463, `cmd/view/named_views.rs`), read and written; no Orthographic, UCS or background settings; view twist kept but not shown |
| Tiled model-space viewports (VPORTS) | yes | yes (#463, `cmd/view/vports.rs`): -VPORTS SIngle, 2/3/4, Join, Save, Restore; MVIEW Restore into a layout is missing |
| 3D orbit, ViewCube drag, SteeringWheels, navigation bar | yes | no (M11) |

## Keyboard and context menus (≈ 60%)

- Shortcut table: 29 entries (Cmd shortcuts, F1, F2, F3, F5, F7–F12) plus 24 command `.key()`
  shortcuts. AutoCAD for Mac's defaults are similar in number; F4 (3D osnap) and F6 (dynamic UCS)
  are missing.
- Delete erases the selection on macOS (#64); Escape cancels.
- Right-click on the canvas is Enter while a command runs; holding it (250 ms, AutoCAD's
  time-sensitive right-click) opens the command menu: Enter, Cancel, Recent Input, the prompt's
  options, Snap Overrides, Pan/Zoom. Otherwise it opens a shortcut menu (Repeat, Recent Commands,
  Clipboard ▸, Isolate ▸, the selection's editing commands, Draw Order ▸, Group, the selected
  object type's edit commands, Select Similar, Deselect All, Undo/Redo, Pan/Zoom; #325, #512).
  The command line, drawing tabs, layout tabs, Layers palette rows and the Properties palette
  have menus too (#512). Still missing: in-place layout tab rename (Rename Layout asks on the
  command line), the Move or Copy dialog, Select All Layouts, Paste to command line, and menus on
  the other palettes.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-14 | major | Every row re-checked against the source (main at e707ab4). Rows that still said "no" for landed work corrected: TK/TT, typed snap overrides, midpoint/tangent/perpendicular on ellipses and splines, PolarSnap, polar angles relative to the last segment, grip value fields, DYNPROMPT menu, hot-grip options and several hot grips (#425), dynamic block grips, UNDO options (#461), SCRIPT (#693), Options dialog (#23), ZOOM Realtime/Dynamic, VIEW and VPORTS (#463). Areas re-scored: total 48% → 69%, remaining 110–170 h → 60–100 h. ROADMAP.md and target-app-parity.md not yet updated |
| 2026-10-12 | minor | Selected and hovered objects redrawn in the selection colour (text and arrowheads included); Options ▸ Selection laid out as AutoCAD's: Object grouping, Show grips, grip object limit, selection preview, Visual Effect Settings (selection area, preview filter) |
| 2026-10-12 | minor | Animation effects (dialog and menu fades, smoothed wheel scrolling and ⌘/Ctrl+wheel zoom) follow the operating system's setting; View ▸ Accessibility ▸ Animation Effects chooses System, On or Off |
| 2026-10-12 | minor | Hatch and Gradient / Hatch Edit dialogs (HATCH, GRADIENT, HATCHEDIT, double-click a hatch; HPDLGMODE) |
| 2026-10-12 | minor | Block references now show their stored insertion X/Y/Z in the Properties Inspector, including nonzero Z; fields are read-only |
| 2026-10-11 | minor | Isometric drafting (#556): isometric snap and grid, ISODRAFT, ISOPLANE (F5), ortho along isoplane axes, ELLIPSE Isocircle |
| 2026-10-11 | minor | Object snap tracking, Extension, Parallel, Apparent Intersection, additional polar angles (#379): tracking 30% → 70%, object snaps 40% → 45%; total 43% → 48% |
| 2026-10-10 | minor | Dynamic input 15% → 45% (pointer boxes, #60); press-drag windows (#63); total 40% → 43% |
| 2026-10-10 | major | First UI-parity audit: point entry, snaps, tracking, dynamic input, grips, selection, command line, chrome, navigation, keys |
