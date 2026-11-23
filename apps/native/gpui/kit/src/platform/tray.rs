//! The contract between the tray icon (`mac/tray.rs`, `windows/tray.rs`) and its owner: the
//! menu rows, rebuilt each time it opens, and the chosen row by index.

pub struct TrayRow {
    pub title: String,
    /// A line between rows.
    pub separator: bool,
    pub enabled: bool,
    pub ticked: bool,
    /// The shortcut key shown beside the row, used with ⌘; empty for none.
    pub key: String,
}

/// Supplies the menu rows and handles the chosen one.
pub struct TrayMenu {
    pub rows: Box<dyn Fn() -> Vec<TrayRow>>,
    pub chosen: Box<dyn Fn(usize)>,
}
