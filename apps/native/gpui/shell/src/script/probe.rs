//! View state only the window knows, for a check's `ev:observe` and `ev:state`. Mirrors the
//! core's `script::Probe` field for field. Each screen fills its part (`Screen::probe`); the core
//! adds its own state and writes the line (`nr_script_observe`, `nr_script_state`).

use kit::nr::NrFolderView;
use serde::Serialize;

#[derive(Serialize, Default, Debug)]
pub struct Probe {
    pub settings: bool,
    /// The folder view on screen.
    pub folder: NrFolderView,
    /// The list's rows, or the last column's rows.
    pub rows: Vec<String>,
    /// The list's selection, or each column's selected rows joined with commas.
    pub selected: Vec<String>,
    pub expanded: Vec<String>,
    pub renaming: bool,
    pub columns: usize,
    pub menus: Vec<Vec<String>>,
    pub dialog: String,
    pub keys: String,
    /// The view showing the open file, and whether it sits beside the columns.
    pub file: String,
    pub beside: bool,
    /// The selection as one line, the overlays above the window, and the focused view.
    pub said: String,
    pub over: String,
}

impl Probe {
    pub fn json(&self) -> String {
        serde_json::to_string(self).expect("a probe is plain data")
    }
}
