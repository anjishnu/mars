use crate::{layout::PaneLayout, pane::PaneId};

pub type TabId = usize;

pub struct Tab {
    pub id: TabId,
    pub name: String,
    pub layout: PaneLayout,
    pub focused_pane: PaneId,
    /// Some(pane) when zoomed to fill the tab (tmux prefix-z).
    pub zoomed: Option<PaneId>,
}

impl Tab {
    pub fn new(id: TabId, name: String, root_pane: PaneId) -> Self {
        Tab {
            id,
            name,
            layout: PaneLayout::Single(root_pane),
            focused_pane: root_pane,
            zoomed: None,
        }
    }
}
