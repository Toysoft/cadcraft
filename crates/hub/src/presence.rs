//! The presence projection (CraftHub spec §6.1, §18.4 row 10): what CADCraft's live UI state
//! looks like on the wire, and what a peer's state renders as here. Presence is ephemeral — it
//! is never stored, never journalled, never saved (§2.7's rule, kept verbatim in v3).
//!
//! CADCraft's row in the per-app projection table: the cursor in drawing units (model space),
//! the tab being worked on (model space or a paper-space layout), the selection (entity
//! handles), and the view rectangle (for follow mode). CADCraft has no timeline, no tracks and
//! no generation jobs — those fields belong to the siblings (FilmCraft, EffectCraft, ArtCraft)
//! and stay `None` here.

use craft_collab::adapter::UiState;

/// Everything the CADCraft UI needs to render one peer: the projection is a two-way street —
/// the same fields go out and come back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeerView {
    /// The peer's cursor in drawing units (model space).
    pub cursor: Option<(f64, f64)>,
    /// The tab the peer is on: 0 is model space, a layout's `tab_order` otherwise (a peer on
    /// another tab renders only in the tab bar).
    pub page: Option<u64>,
    /// The peer's selection (entity handles — outline highlights, never edit locks; §6.7 does
    /// locks).
    pub selection: Vec<craft_collab::ids::NodeId>,
    /// The peer's view rectangle in drawing units, offered to the follow mode.
    pub viewport: Option<(f64, f64, f64, f64)>,
}

impl PeerView {
    /// Project CADCraft's UI state into the fields the hub's `UiState` carries.
    ///
    /// The selection is bounded (§2.12: at most 10,000 ids) by the core's sanitizer, not here —
    /// the app reports what it truly has and the hub enforces the budget.
    #[must_use]
    pub fn from_ui(ui: &UiState) -> Self {
        Self { cursor: ui.cursor, page: ui.page, selection: ui.selection.clone(), viewport: ui.viewport }
    }

    /// Whether this peer shows anything on `tab` at all (the tab bar always shows everyone; the
    /// canvas only the peers on the same space).
    #[must_use]
    pub fn on_tab(&self, tab: u64) -> bool {
        self.page == Some(tab)
    }
}
