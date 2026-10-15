# The collaboration behavior suite (CADCraft's projections)

> The shared suite every Craft app runs before its CraftHub integration is enabled (the CraftHub
> specification §18.9): each row is one automated test with a written scenario, in the family's
> behaviour-suite tradition. This file is CADCraft's copy of the shared catalog: the scenarios
> are identical in every app, and the *projections* column is where each app writes what the
> scenario means for its own UI.

## The scenarios

| # | Scenario | The guarantee under test | CADCraft's projection |
|---|----------|--------------------------|----------------------|
| 1 | Join by code, join by link | One argument that recognizes itself; the file opens in its own tab, other sessions untouched | A joined drawing opens as its own tab; the model-space view and the status bar stay per-document |
| 2 | Concurrent editing converges | Snapshot-hash equality after arbitrary interleavings, drops and reconnects (per seed) | Entity properties merge last-writer-wins per property; the draw order merges by fractional keys; symbol tables merge per entry |
| 3 | Read-only, with its reason | The reason is shown (view link, role without edit, region lock), never just a badge | The mode bar shows "view link" / "viewer role" / "locked region" with the locked entities tinted |
| 4 | Batch refused with fixes | The server's refusal carries the fixes; the client applies them and lands on the server state | A refused entity creation snaps back with a toast explaining why (the schema names the unit or range it broke) |
| 5 | Per-user undo | My steps are mine alone; later writes by others survive my undo | Undo after a peer's edit undoes only my edit; the peer's entity stays where they put it |
| 6 | Offline, then resume as the same client | The recovery copy reattaches; unsent changes replay or land as a "(recovered copy)" beside the file | The unsent grip edit replays on reconnect; a conflicting copy appears in the recent-files list |
| 7 | Cursor chat and its bounds | ≤100 characters, ephemeral, never enters the document or the journal | The bubble renders at the peer's cursor in model space, then is gone |
| 8 | Spotlight and follow | Follow tracks view and page; any view movement of mine stops the follow | Follow tracks the peer's view rectangle (pan/zoom in drawing units); a peer on another layout tab does not carry the model-space view along |
| 9 | Presence and per-app projections | Ephemeral, throttled to change-only, bounded sizes | **The active tab** is the projection (`page`): 0 is model space, a layout's `tab_order` otherwise — a peer on another tab renders only in the tab bar's collaborator badges; the selection carries entity handles (outline highlights) |
| 10 | Region locks | Locks sync as document properties; locked regions tint; the reason is shown | Locked entities tint; editing them is refused with "locked by <name>" |
| 11 | Contribution, review, merge | The branch lives inside the live session; merge is one undo step; each side's changes show | The review pane shows both sides' property diffs per entity, Properties-palette style |
| 12 | Comments and their anchors | Anchor kinds per app; comments are entries with ids and order | Drawing comments anchor to a point in model space (CADCraft's native form); they ride the document-level entry lists with their order |
| 13 | Background file notifications | Delivered when the tab is not front; bounded, dismissible | The tab badge shows "3 new" for a backgrounded shared drawing |
| 14 | Closing a file ends its session alone | Other sessions keep syncing behind the front tab | Closing one shared drawing never drops another drawing's peers |
| 15 | The revert guard | `file.revert` is refused while its document is in a live session (including reconnect and pre-welcome) | The toast says "leave the session before reverting"; the drawing and its backup are intact |

## CADCraft's adapter surface (what the suite drives)

* **Property tables** — the per-kind payload (`kind`, one value: the DXF entity record) plus
  every common entity field (`layer`, `color`, `linetype`, `lineweight`, `ltscale`,
  `transparency`, `visible`, `thickness`, `extrusion`, `plotStyle`, `annoScales`, `xdata`), with
  the compile-time guard on `Common`.
* **Units and precision (§18.4 row 10)** — every unit-bearing number syncs at a canonical ten
  significant digits: tighter than display precision (`LUPREC` maxes at eight decimals) and than
  AUTOCONSTRAIN's default tolerance, looser than `f64` noise, so equal-within-epsilon writes
  canonicalize to the same bytes and never fight (the tolerance test pins this, against the real
  `cadcraft-constraints` solver). Non-finite numbers never ride a batch: the property writer and
  the schema both refuse them.
* **The tree (honestly flat)** — model space *is* the node tree: entities are the root's
  children, ordered by their draw order (fractional keys). A CAD entity owns no sub-entities; a
  block reference displays a definition's entities without containing them, so paper space and
  block definitions sync as document-level entries carrying their whole content (the documented
  first-cut decision; per-entity sync there is the follow-up).
* **The document level** — the unit system every collaborator must agree on (`INSUNITS`,
  `MEASUREMENT`, `LUNITS`/`LUPREC`, `AUNITS`/`AUPREC`, `LTSCALE`), validated against their real
  enums and ranges, plus the symbol tables (layers, linetypes, styles), blocks, layouts,
  constraints and parameters as per-entry keyed lists with their orders. The handle seed
  deliberately does **not** sync: handle allocation is per client (§5.1).
* **The schema's unit rules** — radii and text heights positive, oblique angles inside ±85°,
  linetype scale in (0, 1e6], every number finite and inside ±1e18, and the constraint and
  parameter expressions balanced and bounded before they can confuse anyone's solver.
* **Commands** — the unified `hub.*` catalog with the v2 `collab.*` twins
  (`cadcraft-hub::commands`): same names in every Craft app.
* **Deep links** — `cadcraft://join?<code|token>&server=<url>` (`cadcraft-hub::deeplink`).

## The enable rule

No app is enabled before its protocol core passes its simulation tests in CI for a full week
(nightly, fixed seeds) — and enabling happens through the app's own `crafthub` feature flag,
never through the core, so a regression is one flag flip away from off (§18.4).
