//! The drive as columns, one per folder from the drive's own down to the open one, as Finder
//! shows them. The state is here, not in a shell: which folders are open, what each column
//! lists, what is selected and which file fills the rest of the window.

use crate::archive::Compress;
use crate::drive::history::{History, Step};
use crate::list::row_pointer::{Chosen, Mode, Selected};
use crate::strings::Message;
use crate::workspace::{DriveError, DriveResult, Entry, EntryKind, Location, Trashed, Workspace};
use std::path::PathBuf;

pub struct Column {
    pub location: Location,
    pub rows: Vec<Entry>,
    /// The row whose folder is open in the next column, or whose file shows beside this one.
    pub selected: Option<usize>,
    /// The column's selection by file id (`row_pointer::Chosen`): a group's rows (a group opens
    /// nothing), the range anchor and the row keys act from.
    chosen: Chosen,
    /// The revision the group was chosen at. A group chosen while a row is open (another row
    /// picked up to drag) shows only until the next change; then the open row shows again.
    group_at: u64,
}

impl Column {
    fn new(location: Location, rows: Vec<Entry>, selected: Option<usize>) -> Self {
        Self {
            location,
            rows,
            selected,
            chosen: Chosen::default(),
            group_at: 0,
        }
    }

    fn keys(&self) -> Vec<&str> {
        self.rows.iter().map(|entry| entry.id.as_str()).collect()
    }
}

/// A table of rows a person selects in: the list, or one of the columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    List,
    Column(usize),
}

/// What the list shows of a row, for choosing among its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListShown {
    /// The file's id; a row standing in for an open folder's files has a key of its own.
    pub key: String,
    /// The file's name as the row shows it.
    pub name: String,
    /// The words standing in for an open folder's files: such a row is no file.
    pub placeholder: bool,
    /// A file the system lets move.
    pub movable: bool,
    pub depth: u32,
    pub parent: Location,
}

impl ListShown {
    pub fn of(row: &ListRow) -> Self {
        let placeholder = row.placeholder.is_some();
        Self {
            // A placeholder's entry is the folder it stands in, so it is keyed apart from it.
            key: if placeholder {
                format!("{}\0inside", row.entry.id)
            } else {
                row.entry.id.clone()
            },
            name: row.entry.title.clone(),
            placeholder,
            movable: !placeholder && row.entry.protection.is_none(),
            depth: row.depth,
            parent: row.parent.clone(),
        }
    }
}

/// The folder the list's commands act in for `selected`: their folder when all of them sit in
/// one folder opened in place, otherwise `None` (the folder shown).
pub fn list_scope(rows: &[ListShown], selected: &[usize]) -> Option<Location> {
    let chosen: Vec<&ListShown> = selected.iter().filter_map(|&row| rows.get(row)).collect();
    if chosen.iter().any(|row| row.depth == 0) {
        return None;
    }
    let mut parents = chosen.iter().map(|row| &row.parent);
    let first = parents.next()?;
    parents.all(|parent| parent == first).then(|| first.clone())
}

/// What a right click on list row `clicked` acts in and on: the clicked file's folder when it
/// is opened in place (`None`: the folder shown), and the selected files in that folder.
pub fn list_menu(
    rows: &[ListShown],
    selected: &[usize],
    clicked: usize,
) -> (Option<Location>, Vec<usize>) {
    let row = &rows[clicked];
    let scope = (row.depth > 0).then(|| row.parent.clone());
    let group = selected
        .iter()
        .copied()
        .filter(|&at| !rows[at].placeholder && rows[at].parent == row.parent)
        .collect();
    (scope, group)
}

/// The file that fills the window beside the columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub id: String,
    pub kind: EntryKind,
    pub title: String,
    /// The terminal files opened with it, side by side, it among them, by id and name in
    /// their folder's order; empty for a file open alone.
    pub together: Vec<(String, String)>,
    /// Opened by its card's play button: the film or video starts playing as its view opens
    /// (`take_starts_playing`).
    pub starts_playing: bool,
}

/// A tab closed lately, as Reopen Closed Tab brings it back: where it stood among the tabs,
/// the folder it showed and the file open in it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ClosedTab {
    at: usize,
    location: Location,
    /// The open file's id, and whether it filled the tab.
    opened: Option<(String, bool)>,
    /// The root a Changes tab showed.
    changes: Option<std::path::PathBuf>,
}

/// How many closed tabs Reopen Closed Tab can go back through.
const CLOSED_TABS_KEPT: usize = 20;

/// A tab's id, unique among the tabs a drive has opened. It stays the same whatever the tab
/// shows and wherever it moves in the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabId(pub(crate) u64);

/// One open place, with its own columns, open file, and back and forward history.
struct Tab {
    id: TabId,
    columns: Vec<Column>,
    opened: Option<Opened>,
    /// The folders visited before the one shown, oldest first.
    past: Vec<Location>,
    /// The folders gone back from, nearest first.
    future: Vec<Location>,
    /// The open file was opened to fill the tab (a double click, ⌘↓ or ⌘O). It then fills it
    /// as columns too, where a file that is only selected shows beside its column.
    filling: bool,
    /// A Changes tab: the root whose changes it shows (`changes`). Its columns are not shown
    /// and have no history; going to a folder in it makes it that folder's tab.
    changes: Option<std::path::PathBuf>,
}

impl Tab {
    fn location(&self) -> Location {
        self.columns
            .last()
            .map_or(Location::ROOT, |column| column.location.clone())
    }

    /// The open file closes and its folder shows again.
    fn close_opened(&mut self) {
        self.opened = None;
        self.filling = false;
        if let Some(column) = self.columns.last_mut() {
            column.selected = None;
        }
    }

    /// Up to the enclosing folder, which Forward comes back from.
    fn go_up(&mut self) {
        let current = self.location();
        self.columns.pop();
        if let Some(column) = self.columns.last_mut() {
            column.selected = None;
        }
        self.future.insert(0, current);
    }
}

/// The order a folder's files are listed in: the list's sort and whether folders come first.
type Order = (Option<crate::list::Sort>, bool);

/// How a folder is shown. The shell shares this type (`window::ffi::NrFolderView`), and its
/// lowercase name ("board", "list", "columns") is used by `view.json`, menu ids and checks.
/// Keep the order; the shell reads it by number.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    Board,
    List,
    /// What a drive never opened before shows.
    #[default]
    Columns,
}

impl View {
    /// Every view, in the order the menus list them.
    pub const ALL: [Self; 3] = [Self::Board, Self::List, Self::Columns];

    /// The view's lowercase name, as `view.json` and the menu bar's ids say it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::List => "list",
            Self::Columns => "columns",
        }
    }

    /// The view's place in `ALL`: the number a folder menu's id (`view:2`) and a check's `state`
    /// line say it by.
    pub fn number(self) -> u32 {
        match self {
            Self::Board => 0,
            Self::List => 1,
            Self::Columns => 2,
        }
    }

    /// The view `number` names, or `None` past the last one.
    pub fn numbered(number: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|view| view.number() == number)
    }
}

/// Reads `view.json`'s view. An unknown name falls back to columns, so one odd field does not
/// lose the person's other choices.
fn kept_view<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<View, D::Error> {
    let name: String = serde::Deserialize::deserialize(reader)?;
    Ok(View::ALL
        .into_iter()
        .find(|view| view.name() == name)
        .unwrap_or_default())
}

/// What stands in the list for an open folder's files when it shows none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placeholder {
    /// The folder holds nothing: "Empty folder", as its own column says too.
    Empty,
    /// The folder could not be read.
    Unreadable,
}

impl Placeholder {
    /// What the row says, in `language`.
    pub fn words(self, language: &str) -> &'static str {
        crate::strings::text(
            language,
            match self {
                Self::Empty => crate::strings::key::COLUMNS_EMPTY_FOLDER,
                Self::Unreadable => crate::strings::key::COLUMNS_FOLDER_UNREADABLE,
            },
        )
    }
}

/// One row of the list view, with its cells already written out.
pub struct ListRow {
    pub entry: Entry,
    /// What each column after Name says, in the columns' order.
    pub cells: Vec<String>,
    /// How many open folders the row is inside; 0 for the folder shown's own files.
    pub depth: u32,
    /// Whether the row is a folder whose files are listed under it.
    pub expanded: bool,
    /// The folder the row's file is in.
    pub parent: Location,
    /// What stands in for an open folder's files ("Empty folder"); such a row is no file.
    pub placeholder: Option<Placeholder>,
}

/// What ← or → does on a row of the list (`list_expand_step`, `list_collapse_step`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListStep {
    /// The folder with this id opens in place, or closes.
    Toggle(String),
    /// The keys move to this row.
    Focus(usize),
    Stay,
}

/// → on row `row`: a folder opens in place, or the keys go to its first row when
/// it is open.
pub fn list_expand_step(rows: &[ListRow], row: usize) -> ListStep {
    let Some(at) = rows.get(row) else {
        return ListStep::Stay;
    };
    if !at.entry.kind.holds_files() || at.placeholder.is_some() {
        return ListStep::Stay;
    }
    if !at.expanded {
        return ListStep::Toggle(at.entry.id.clone());
    }
    match rows.get(row + 1) {
        Some(next) if next.depth > at.depth && next.placeholder.is_none() => {
            ListStep::Focus(row + 1)
        }
        _ => ListStep::Stay,
    }
}

/// ← on row `row`: an open folder closes, or the keys go to the row of the folder it is in.
pub fn list_collapse_step(rows: &[ListRow], row: usize) -> ListStep {
    let Some(at) = rows.get(row) else {
        return ListStep::Stay;
    };
    if at.expanded {
        return ListStep::Toggle(at.entry.id.clone());
    }
    match rows[..row].iter().rposition(|above| above.depth < at.depth) {
        Some(parent) if at.depth > 0 => ListStep::Focus(parent),
        _ => ListStep::Stay,
    }
}

/// Where a drag of the drive's files lands in the columns (`Columns::column_drop_place`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnDrop {
    Nowhere,
    /// Into the folder at this row.
    Folder(usize),
    /// To the running terminal at this row, which is handed the files as references.
    Terminal(usize),
    /// Into the folder the column lists.
    Column,
}

/// What a drop on `entry` at `row` does when it is not one of the dropped files. A folder takes
/// them, as does a running terminal that `takes_references`.
fn row_drop(
    row: usize,
    entry: &Entry,
    takes_references: impl Fn(&Entry) -> bool,
) -> Option<ColumnDrop> {
    if entry.kind == EntryKind::Folder {
        return Some(ColumnDrop::Folder(row));
    }
    (entry.kind == EntryKind::Terminal && takes_references(entry))
        .then_some(ColumnDrop::Terminal(row))
}

/// The new name for a rename typed as `typed`, trimmed. `None` when that is empty or unchanged,
/// so the file is left alone. Every view renames by this rule.
pub fn renamed_title<'a>(current: &str, typed: &'a str) -> Option<&'a str> {
    let title = typed.trim();
    (!title.is_empty() && title != current).then_some(title)
}

/// How a person left the drive looking, kept for the drive so it is the same next time.
#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ViewChoices {
    #[serde(deserialize_with = "kept_view")]
    view: View,
    /// The list's columns after Name, by `list::Column::number`.
    list_columns: Vec<u32>,
    list_widths: Vec<(u32, f64)>,
    list_sort: Option<(u32, bool)>,
    /// "Keep folders on top".
    list_folders_first: bool,
    hidden_shown: bool,
    /// How many columns the board is zoomed to; absent means the default.
    board_columns: Option<usize>,
}

const VIEW_CHOICES_FILE: &str = "view.json";

pub struct Columns {
    workspace: Workspace,
    tabs: Vec<Tab>,
    active: usize,
    /// How many tabs have been opened: the next one's id.
    tabs_opened: u64,
    view: View,
    list_sort: Option<crate::list::Sort>,
    folders_first: bool,
    /// The list's columns, Name first, and the widths a person dragged any to.
    list_columns: Vec<crate::list::Column>,
    list_widths: std::collections::HashMap<crate::list::Column, f64>,
    /// The board's zoom: how many columns it shows when it is wide enough for them.
    board_columns: usize,
    /// The folders opened in place in the list, by id.
    expanded: std::collections::HashSet<String>,
    /// The folder a command names when it is not a column's: an open folder of the list.
    scope: Option<Location>,
    /// The list's rows as the shell last showed them, what is chosen among them, and the
    /// folder its commands act in (`None`: the folder shown; see `list_scope`).
    list_shown: Vec<ListShown>,
    list_chosen: Chosen,
    list_scope: Option<Location>,
    /// The letters typed lately to find a row by its name.
    typed: crate::list::row_keys::Typed,
    /// Goes up whenever a column, the open file or a tab changes, so a shell knows to redraw.
    revision: u64,
    /// A file opened that goes to its own application (`FileRead::External`): its path,
    /// until the shell takes it to open.
    elsewhere: Option<std::path::PathBuf>,
    /// The drive changes made here, to undo and redo (`drive::history`).
    history: History,
    /// The tabs closed lately, the last closed last (`reopen_closed_tab`).
    closed: Vec<ClosedTab>,
}

impl Columns {
    pub fn open(mut workspace: Workspace) -> DriveResult<Self> {
        // What was chosen last time, before the first listing: hidden files may be shown.
        let kept: Option<ViewChoices> =
            std::fs::read_to_string(workspace.metadata_directory().join(VIEW_CHOICES_FILE))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok());
        if let Some(kept) = &kept {
            workspace.set_hidden_shown(kept.hidden_shown);
        }
        let mut columns = Self {
            workspace,
            tabs: Vec::new(),
            active: 0,
            tabs_opened: 0,
            // A new drive shows as columns; the view a person picks is kept for next time.
            view: View::Columns,
            list_sort: None,
            folders_first: false,
            list_columns: crate::list::Column::DEFAULT.to_vec(),
            list_widths: std::collections::HashMap::new(),
            board_columns: crate::board::DEFAULT_COLUMN_COUNT,
            expanded: std::collections::HashSet::new(),
            scope: None,
            list_shown: Vec::new(),
            list_chosen: Chosen::default(),
            list_scope: None,
            typed: crate::list::row_keys::Typed::default(),
            revision: 1,
            elsewhere: None,
            history: History::default(),
            closed: Vec::new(),
        };
        if let Some(kept) = kept {
            use crate::list::Column;
            columns.view = kept.view;
            let chosen: Vec<Column> = kept
                .list_columns
                .iter()
                .filter_map(|number| Column::numbered(*number))
                .collect();
            if !kept.list_columns.is_empty() || kept.view == View::List {
                columns.list_columns = std::iter::once(Column::Name)
                    .chain(chosen.into_iter().filter(|column| *column != Column::Name))
                    .collect();
            }
            columns.list_widths = kept
                .list_widths
                .iter()
                .filter_map(|(number, width)| {
                    Column::numbered(*number).map(|column| (column, column.clamp_width(*width)))
                })
                .collect();
            columns.list_sort = kept.list_sort.and_then(|(number, ascending)| {
                Column::numbered(number).map(|key| crate::list::Sort { key, ascending })
            });
            columns.folders_first = kept.list_folders_first;
            if let Some(count) = kept.board_columns {
                columns.board_columns = crate::board::normalize_column_count(count as f64);
            }
        }
        // List the drive once on open, in the kept order, asking the file index only when the
        // kept view shows a column that needs it.
        let wanted = columns.wants_index();
        columns.workspace.set_index_wanted(wanted);
        let first = columns.new_tab(&Location::ROOT)?;
        columns.tabs.push(first);
        Ok(columns)
    }

    /// Keeps how the drive looks, so it opens the same way next time. A failure to write is
    /// only reported: the choice still holds for this run.
    fn keep_choices(&self) {
        let mut widths: Vec<(u32, f64)> = self
            .list_widths
            .iter()
            .map(|(column, width)| (column.number(), *width))
            .collect();
        widths.sort_by_key(|(number, _)| *number);
        let choices = ViewChoices {
            view: self.view,
            list_columns: self.list_columns[1..]
                .iter()
                .map(|column| column.number())
                .collect(),
            list_widths: widths,
            list_sort: self
                .list_sort
                .map(|sort| (sort.key.number(), sort.ascending)),
            list_folders_first: self.folders_first,
            hidden_shown: self.workspace.hidden_shown(),
            board_columns: Some(self.board_columns),
        };
        let directory = self.workspace.metadata_directory();
        let written = serde_json::to_string_pretty(&choices)
            .map_err(std::io::Error::other)
            .and_then(|text| {
                std::fs::create_dir_all(directory)?;
                let staged = directory.join(format!("{VIEW_CHOICES_FILE}.writing"));
                std::fs::write(&staged, text)?;
                std::fs::rename(staged, directory.join(VIEW_CHOICES_FILE))
            });
        if let Err(error) = written {
            eprintln!("the view was not kept: {error}");
        }
    }

    /// Shows the files whose names start with a dot, or hides them again, as Finder's ⇧⌘.
    /// does. Returns whether they are shown now.
    pub fn toggle_hidden(&mut self) -> DriveResult<bool> {
        let shown = !self.workspace.hidden_shown();
        self.workspace.set_hidden_shown(shown);
        self.keep_choices();
        self.refresh()?;
        Ok(shown)
    }

    /// Moves a list column to place `to` among the columns (1 is the first after Name).
    pub fn move_list_column(&mut self, column: crate::list::Column, to: usize) {
        let Some(from) = self.list_columns.iter().position(|shown| *shown == column) else {
            return;
        };
        if from == 0 {
            return;
        }
        let to = to.clamp(1, self.list_columns.len() - 1);
        if from != to {
            let moved = self.list_columns.remove(from);
            self.list_columns.insert(to, moved);
            self.revision += 1;
            self.keep_choices();
        }
    }

    /// A tab not opened before, on the folder at `location`.
    fn new_tab(&mut self, location: &Location) -> DriveResult<Tab> {
        let order = self.order();
        let columns = Self::columns_at(&mut self.workspace, location, order)?;
        let id = TabId(self.tabs_opened);
        self.tabs_opened += 1;
        Ok(Tab {
            id,
            columns,
            opened: None,
            past: Vec::new(),
            future: Vec::new(),
            filling: false,
            changes: None,
        })
    }

    /// A tab showing the folder at `location`: one column per folder from the drive's root down
    /// to it, each with the next folder selected.
    fn columns_at(
        workspace: &mut Workspace,
        location: &Location,
        order: Order,
    ) -> DriveResult<Vec<Column>> {
        let crumbs = workspace.path(location)?;
        let mut columns = Vec::with_capacity(crumbs.len());
        for (index, crumb) in crumbs.iter().enumerate() {
            let mut rows = workspace.all_contents(&crumb.location)?;
            crate::list::sort_entries(&mut rows, order.0, order.1);
            let next = crumbs
                .get(index + 1)
                .and_then(|next| next.location.folder_id.as_deref());
            let selected = next.and_then(|id| rows.iter().position(|entry| entry.id == id));
            columns.push(Column::new(crumb.location.clone(), rows, selected));
        }
        Ok(columns)
    }

    fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// Shows the folder at `location` in the active tab, keeping its history.
    fn show(&mut self, location: &Location) -> DriveResult<()> {
        let order = self.order();
        let columns = Self::columns_at(&mut self.workspace, location, order)?;
        let tab = self.tab_mut();
        tab.columns = columns;
        tab.opened = None;
        tab.changes = None;
        self.revision += 1;
        Ok(())
    }

    /// Back closes an open file first, then walks the visited folders, then goes up to the
    /// enclosing folder. It is unavailable only at the drive's root with nothing open.
    pub fn can_go_back(&self) -> bool {
        let tab = self.tab();
        if tab.changes.is_some() {
            return false;
        }
        // As columns, a file selected beside its column is not a place to go back from.
        let file_shown = tab.opened.is_some() && (self.view != View::Columns || tab.filling);
        file_shown || !tab.past.is_empty() || tab.columns.len() > 1
    }

    pub fn view(&self) -> View {
        self.view
    }

    pub fn set_view(&mut self, view: View) {
        if self.view != view {
            self.view = view;
            // The list starts again with nothing chosen.
            self.list_chosen = Chosen::default();
            self.list_scope = None;
            self.revision += 1;
            self.keep_choices();
        }
    }

    /// The board's zoom: how many columns it shows when it is wide enough for them.
    pub fn board_columns(&self) -> usize {
        self.board_columns
    }

    /// Zooms the board to `count` columns, held between one and eight. The board lays itself
    /// out again; nothing else on screen changes, so the revision stays.
    pub fn set_board_columns(&mut self, count: usize) {
        let count = crate::board::normalize_column_count(count as f64);
        if self.board_columns != count {
            self.board_columns = count;
            self.keep_choices();
        }
    }

    pub fn list_sort(&self) -> Option<crate::list::Sort> {
        self.list_sort
    }

    /// The list's order, which the columns follow too.
    fn order(&self) -> Order {
        (self.list_sort, self.folders_first)
    }

    /// The columns take the list's order again after it changed.
    fn reorder(&mut self) {
        if let Err(error) = self.refresh() {
            eprintln!("the folders were not listed again: {error}");
        }
    }

    /// A header click: a new column sorts ascending, the same column flips direction.
    pub fn toggle_list_sort(&mut self, key: crate::list::SortKey) {
        self.list_sort = Some(crate::list::toggle_sort(self.list_sort, key));
        self.revision += 1;
        self.keep_choices();
        self.reorder();
    }

    /// Sort By in a folder's menu: the board and the list both by `key`, in the direction
    /// Finder takes for it: names and kinds A to Z, dates newest first, sizes largest first.
    pub fn sort_by(&mut self, key: crate::list::SortKey) {
        use crate::list::SortKey;
        let ascending = matches!(key, SortKey::Name | SortKey::Kind);
        self.list_sort = Some(crate::list::Sort { key, ascending });
        self.revision += 1;
        self.keep_choices();
        self.reorder();
    }

    /// Whether the list keeps its folders above its files.
    pub fn folders_first(&self) -> bool {
        self.folders_first
    }

    /// "Keep folders on top", on or off.
    pub fn toggle_folders_first(&mut self) {
        self.folders_first = !self.folders_first;
        self.revision += 1;
        self.keep_choices();
        self.reorder();
    }

    /// "Restore default view": the default columns at their own widths, the folder's own
    /// order, and folders among the files.
    pub fn reset_list_options(&mut self) {
        self.list_columns = crate::list::Column::DEFAULT.to_vec();
        self.list_widths.clear();
        self.list_sort = None;
        self.folders_first = false;
        self.revision += 1;
        self.keep_choices();
        self.reorder();
    }

    /// Whether listings ask the file index: only while the list shows a column that needs it,
    /// since it is asked once per file.
    fn wants_index(&self) -> bool {
        self.view == View::List && self.list_columns.iter().any(|column| column.needs_index())
    }

    /// The folder shown as the list view lists it: its files in the chosen order with cells
    /// written as of `now_seconds`, and the files of each folder opened in place nested one
    /// level deeper, as in Finder.
    pub fn list_rows(&mut self, now_seconds: i64) -> Vec<ListRow> {
        // When the columns that need the file index come or go, the folder is listed again
        // with or without it.
        let wanted = self.wants_index();
        if wanted != self.workspace.index_wanted() {
            self.workspace.set_index_wanted(wanted);
            let revision = self.revision;
            let _ = self.refresh();
            self.revision = revision;
        }
        let Some(shown) = self.tab().columns.last() else {
            return Vec::new();
        };
        let (location, entries) = (shown.location.clone(), shown.rows.clone());
        let mut rows = Vec::new();
        self.push_list_rows(&mut rows, entries, &location, 0, now_seconds);
        rows
    }

    fn push_list_rows(
        &mut self,
        rows: &mut Vec<ListRow>,
        mut entries: Vec<Entry>,
        parent: &Location,
        depth: u32,
        now_seconds: i64,
    ) {
        crate::list::sort_entries(&mut entries, self.list_sort, self.folders_first);
        for entry in entries {
            let expanded = entry.kind.holds_files() && self.expanded.contains(&entry.id);
            let inside = Location {
                folder_id: Some(entry.id.clone()),
            };
            rows.push(ListRow {
                cells: self.list_columns[1..]
                    .iter()
                    .map(|column| crate::list::cell(&entry, *column, now_seconds))
                    .collect(),
                depth,
                expanded,
                parent: parent.clone(),
                placeholder: None,
                entry: entry.clone(),
            });
            if !expanded {
                continue;
            }
            let placeholder = match self.workspace.all_contents(&inside) {
                Ok(children) if children.is_empty() => Some(Placeholder::Empty),
                Ok(children) => {
                    self.push_list_rows(rows, children, &inside, depth + 1, now_seconds);
                    None
                }
                Err(_) => Some(Placeholder::Unreadable),
            };
            if let Some(placeholder) = placeholder {
                rows.push(ListRow {
                    entry,
                    cells: Vec::new(),
                    depth: depth + 1,
                    expanded: false,
                    parent: inside,
                    placeholder: Some(placeholder),
                });
            }
        }
    }

    /// Opens a file listed under a folder opened in place: its folder becomes the folder
    /// shown, as a visit, and then the file opens in it.
    pub fn open_in(&mut self, parent: &Location, id: &str) -> DriveResult<()> {
        let before = self.tab().location();
        if *parent != before {
            self.show(parent)?;
            let tab = self.tab_mut();
            tab.past.push(before);
            tab.future.clear();
        }
        self.open_in_folder(id)
    }

    /// The list's columns, Name first.
    pub fn list_columns(&self) -> &[crate::list::Column] {
        &self.list_columns
    }

    /// Shows a column at the list's right end, or takes it away. Name stays.
    pub fn toggle_list_column(&mut self, column: crate::list::Column) {
        if column == crate::list::Column::Name {
            return;
        }
        match self.list_columns.iter().position(|shown| *shown == column) {
            Some(index) => {
                self.list_columns.remove(index);
                // A list is not left sorted by a column it no longer shows.
                if self.list_sort.is_some_and(|sort| sort.key == column) {
                    self.list_sort = None;
                    self.reorder();
                }
            }
            None => self.list_columns.push(column),
        }
        self.revision += 1;
        self.keep_choices();
    }

    /// A column's width: what a person dragged it to, or what it starts at.
    pub fn list_width(&self, column: crate::list::Column) -> f64 {
        self.list_widths
            .get(&column)
            .copied()
            .unwrap_or(column.widths().0)
    }

    /// Sets a column's width, within its limits; a width of 0 gives it back its own.
    pub fn set_list_width(&mut self, column: crate::list::Column, width: f64) {
        if width <= 0.0 {
            self.list_widths.remove(&column);
        } else {
            self.list_widths.insert(column, column.clamp_width(width));
        }
        self.revision += 1;
        self.keep_choices();
    }

    /// Opens a folder in place in the list, or closes it.
    pub fn toggle_expanded(&mut self, id: &str) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_owned());
        }
        self.revision += 1;
    }

    /// Opens a file of the folder shown by its id, as a double click in the list does: a
    /// folder becomes the folder shown, a file fills the window (`open_at`).
    pub fn open_in_folder(&mut self, id: &str) -> DriveResult<()> {
        let column = self.tab().columns.len().saturating_sub(1);
        let Some(row) = self
            .tab()
            .columns
            .last()
            .and_then(|last| last.rows.iter().position(|entry| entry.id == id))
        else {
            return Ok(());
        };
        // A link to a file outside the drive is listed but not read: it does not open.
        if self.unreadable(column, row) {
            return Ok(());
        }
        if self.open_terminals_together(column, row)? {
            return Ok(());
        }
        // A file the window cannot show opens in its own application; the folder stays.
        if let Some(path) = self.opens_elsewhere(column, row) {
            self.elsewhere = Some(path);
            return Ok(());
        }
        self.open_at(column, row)
    }

    /// Goes to `path` in the active tab as a visit Back returns from. A folder becomes the
    /// folder shown; a file opens in its folder as a double click opens it. Handles paths other
    /// programs and agents ask the app to open (`open -a Northroom <path>`). Refused when `path`
    /// is outside the drive.
    pub fn go_to_path(&mut self, path: &std::path::Path) -> DriveResult<()> {
        if path.is_dir() {
            let folder = self.workspace.location_at(path)?;
            return self.visit(&folder);
        }
        let id = self.workspace.id_at(path)?;
        let folder = self.workspace.location_at(path.parent().unwrap_or(path))?;
        self.visit(&folder)?;
        self.open_in_folder(&id)
    }

    /// Shows the folder at `location` as a visit: Back returns to the folder shown before.
    fn visit(&mut self, location: &Location) -> DriveResult<()> {
        let before = self.tab().location();
        self.show(location)?;
        let tab = self.tab_mut();
        tab.filling = false;
        if *location != before {
            tab.past.push(before);
            tab.future.clear();
        }
        Ok(())
    }

    /// Opens the film or video `id` as `open_in_folder` does and starts it playing. Used by the
    /// play button on its card.
    pub fn play_in_folder(&mut self, id: &str) -> DriveResult<()> {
        self.open_in_folder(id)?;
        if let Some(opened) = self
            .tab_mut()
            .opened
            .as_mut()
            .filter(|opened| opened.id == id)
        {
            opened.starts_playing = true;
        }
        Ok(())
    }

    /// Whether the open file's view should start playing as it opens. Asking clears the flag, so
    /// playback starts once.
    pub fn take_starts_playing(&mut self) -> bool {
        self.tab_mut()
            .opened
            .as_mut()
            .is_some_and(|opened| std::mem::take(&mut opened.starts_playing))
    }

    /// The path of the file at `row` of `column` when it opens in its own application rather
    /// than being shown: a plain file that `workspace::read_file` reads as nothing else
    /// (`FileRead::External`).
    fn opens_elsewhere(&mut self, column: usize, row: usize) -> Option<std::path::PathBuf> {
        let entry = self.columns().get(column)?.rows.get(row)?.clone();
        if entry.kind != EntryKind::File {
            return None;
        }
        let path = self.workspace.path_of(&entry.id).ok()?;
        (crate::workspace::read_file(&path) == crate::workspace::FileRead::External).then_some(path)
    }

    /// Whether the file at `row` of `column` is one whose bytes may not be read: a link that
    /// leads outside the drive, or one whose protection could not be read.
    fn unreadable(&self, column: usize, row: usize) -> bool {
        self.columns()
            .get(column)
            .and_then(|shown| shown.rows.get(row))
            .is_some_and(|entry| {
                entry.kind != EntryKind::Folder
                    && entry.protection == Some(crate::workspace::Protection::Unavailable)
            })
    }

    /// The file last opened that goes to its own application, once: its path.
    pub fn take_opened_elsewhere(&mut self) -> Option<std::path::PathBuf> {
        self.elsewhere.take()
    }

    pub fn can_go_forward(&self) -> bool {
        self.tab().changes.is_none() && !self.tab().future.is_empty()
    }

    /// Goes to the folder shown before this one. A folder that has gone since is skipped.
    pub fn go_back(&mut self) -> DriveResult<()> {
        if !self.can_go_back() {
            return Ok(());
        }
        if self.view == View::Columns && self.tab().filling {
            // As columns, a file that fills the tab goes back to being the file selected,
            // beside its column, as it was before it was opened. Forward does not open a file
            // again after Back has closed it.
            self.tab_mut().filling = false;
        } else if self.tab().opened.is_some() && self.view != View::Columns {
            self.tab_mut().close_opened();
        } else if self.tab().past.is_empty() && self.tab().columns.len() > 1 {
            // Nothing visited before this: go up to the enclosing folder.
            self.tab_mut().go_up();
        } else {
            return self.go_to_past();
        }
        self.revision += 1;
        Ok(())
    }

    /// Shows the folder visited before this one, skipping any that has gone since.
    fn go_to_past(&mut self) -> DriveResult<()> {
        while let Some(destination) = self.tab_mut().past.pop() {
            let current = self.tab().location();
            if self.show(&destination).is_ok() {
                self.tab_mut().future.insert(0, current);
                return Ok(());
            }
        }
        Ok(())
    }

    pub fn go_forward(&mut self) -> DriveResult<()> {
        while self.can_go_forward() {
            let destination = self.tab_mut().future.remove(0);
            let current = self.tab().location();
            if self.show(&destination).is_ok() {
                self.tab_mut().past.push(current);
                return Ok(());
            }
        }
        Ok(())
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab(&self) -> usize {
        self.active
    }

    /// Which tab the active one is: another id when another tab becomes active, the same
    /// when a tab beside it closes and it only moves along the row.
    pub fn active_tab_id(&self) -> TabId {
        self.tab().id
    }

    /// Whether a tab's open file fills it in the folder's place, as it does in board and list
    /// views. As columns the file shows beside its folder and the tab stays the folder's.
    fn file_fills_tab(&self, index: usize) -> Option<&Opened> {
        let tab = self.tabs.get(index)?;
        if self.view == View::Columns && !tab.filling {
            return None;
        }
        tab.opened.as_ref()
    }

    /// What a tab holds, for its glyph: 0 the drive's own folder, 1 another folder, 3 a file, 5 a
    /// terminal, 6 a root's changes.
    pub fn tab_kind(&self, index: usize) -> u32 {
        if self.tab_changes(index).is_some() {
            return 6;
        }
        if let Some(opened) = self.file_fills_tab(index) {
            return if opened.kind == EntryKind::Terminal {
                5
            } else {
                3
            };
        }
        let Some(tab) = self.tabs.get(index) else {
            return 0;
        };
        u32::from(tab.columns.len() > 1)
    }

    /// The terminal file filling tab `index`, for the mark and the activity its tab shows;
    /// `None` when the tab holds anything else.
    pub fn tab_terminal(&self, index: usize) -> Option<&Opened> {
        self.file_fills_tab(index)
            .filter(|opened| opened.kind == EntryKind::Terminal)
    }

    /// Brings the file with `id` to the front from anywhere in the drive, as the sessions list
    /// does. Selects the tab it fills, or opens a new tab on its folder with the file filling
    /// it, leaving the current tab as it was.
    pub fn show_in_tab(&mut self, id: &str) -> DriveResult<()> {
        let filled = (0..self.tabs.len()).find(|&tab| {
            self.file_fills_tab(tab)
                .is_some_and(|opened| opened.id == id)
        });
        if let Some(tab) = filled {
            self.select_tab(tab);
            return Ok(());
        }
        let placed = self.workspace.place_of(id)?;
        let tab = self.new_tab(&placed.folder)?;
        self.tabs.insert(self.active + 1, tab);
        self.active += 1;
        self.revision += 1;
        let column = self.columns().len() - 1;
        let listed = self.columns()[column]
            .rows
            .iter()
            .position(|entry| entry.id == id);
        // A file its folder does not list (a hidden one) leaves the new tab on the folder.
        match listed {
            Some(row) => self.open_row(column, row),
            None => Ok(()),
        }
    }

    /// The glyph a tab shows before its name: what it holds, the same in the active tab as in
    /// the others.
    pub fn tab_glyph(&self, index: usize) -> crate::list::Glyph {
        use crate::list::Glyph;
        match self.tab_kind(index) {
            1 => Glyph::Folder,
            3 => Glyph::File,
            5 => Glyph::TerminalWindow,
            6 => Glyph::Diff,
            _ => Glyph::House,
        }
    }

    /// The name a tab shows: the file that fills it, or the folder it is in.
    pub fn tab_title(&self, index: usize) -> String {
        if let Some(opened) = self.file_fills_tab(index) {
            return opened.title.clone();
        }
        let Some(location) = self.tabs.get(index).map(Tab::location) else {
            return String::new();
        };
        self.workspace
            .folder_title(&location)
            .unwrap_or_else(|_| self.workspace.root_label().to_owned())
    }

    /// The root tab `index` shows the changes of, when it is a Changes tab.
    pub fn tab_changes(&self, index: usize) -> Option<&std::path::Path> {
        self.tabs.get(index)?.changes.as_deref()
    }

    /// The root the active tab shows the changes of, when it is a Changes tab.
    pub fn active_changes(&self) -> Option<&std::path::Path> {
        self.tab_changes(self.active)
    }

    /// Whether a tab shows the changes of `root`.
    pub fn shows_changes(&self, root: &std::path::Path) -> bool {
        self.tabs
            .iter()
            .any(|tab| tab.changes.as_deref() == Some(root))
    }

    /// Selects the Changes tab of `root`, or opens one beside the active tab and makes it
    /// active.
    pub fn open_changes_tab(&mut self, root: &std::path::Path) -> DriveResult<()> {
        let shown = self
            .tabs
            .iter()
            .position(|tab| tab.changes.as_deref() == Some(root));
        if let Some(index) = shown {
            self.select_tab(index);
            return Ok(());
        }
        let mut tab = self.new_tab(&Location::ROOT)?;
        tab.changes = Some(root.to_path_buf());
        self.tabs.insert(self.active + 1, tab);
        self.active += 1;
        self.revision += 1;
        Ok(())
    }

    /// Opens a new tab beside the active one, on the drive's own folder, and makes it active.
    pub fn open_tab(&mut self) -> DriveResult<()> {
        let tab = self.new_tab(&Location::ROOT)?;
        self.tabs.insert(self.active + 1, tab);
        self.active += 1;
        self.revision += 1;
        Ok(())
    }

    pub fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() && index != self.active {
            self.active = index;
            self.revision += 1;
        }
    }

    /// Moves the tab at `from` to `to` among the tabs, as a tab carried along the title bar
    /// settles; the active tab stays active wherever it now is.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        let count = self.tabs.len();
        if from >= count || to >= count || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = match self.active {
            active if active == from => to,
            active if from < active && active <= to => active - 1,
            active if to <= active && active < from => active + 1,
            active => active,
        };
        self.revision += 1;
    }

    /// The terminal files filling tab `index`: the one open, or each opened together.
    pub fn tab_terminal_ids(&self, index: usize) -> Vec<String> {
        let Some(opened) = self.tabs.get(index).and_then(|tab| tab.opened.as_ref()) else {
            return Vec::new();
        };
        if opened.kind != EntryKind::Terminal {
            return Vec::new();
        }
        if opened.together.is_empty() {
            return vec![opened.id.clone()];
        }
        opened.together.iter().map(|(id, _)| id.clone()).collect()
    }

    /// Whether a tab shows the terminal file `id`, alone or beside others.
    pub fn shows_terminal(&self, id: &str) -> bool {
        (0..self.tabs.len()).any(|index| self.tab_terminal_ids(index).iter().any(|each| each == id))
    }

    /// Whether the terminal file `id` is a throwaway: it still has the default name, `label`
    /// alone or numbered ("Terminal", "Terminal 2"). Renaming it keeps it.
    pub fn is_throwaway_terminal(&mut self, id: &str, label: &str) -> bool {
        let Ok(path) = self.workspace.path_of(id) else {
            return false;
        };
        if path.extension().and_then(|extension| extension.to_str()) != Some("nterm") {
            return false;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return false;
        };
        stem == label
            || stem
                .strip_prefix(label)
                .and_then(|rest| rest.strip_prefix(' '))
                .is_some_and(|number| {
                    !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                })
    }

    /// Closes every tab the file `id` fills, alone or beside other terminals, as ending its
    /// session does. The last tab stays open on its folder.
    pub fn close_tabs_of(&mut self, id: &str) {
        let shows = |tab: &Tab| {
            tab.opened.as_ref().is_some_and(|opened| {
                opened.id == id || opened.together.iter().any(|(each, _)| each == id)
            })
        };
        for index in (0..self.tabs.len()).rev() {
            if !shows(&self.tabs[index]) {
                continue;
            }
            if self.can_close_tab(index) {
                self.close_tab(index);
            } else {
                self.tabs[index].close_opened();
                self.revision += 1;
            }
        }
    }

    /// Whether a closed tab is there to reopen.
    pub fn can_reopen_closed_tab(&self) -> bool {
        !self.closed.is_empty()
    }

    /// Reopens the last closed tab in its old position with its folder and open file, and makes
    /// it active. A terminal comes back live, since closing its tab never ends its session. If
    /// the folder is gone the tab shows the drive's root; if the file is gone, its folder.
    pub fn reopen_closed_tab(&mut self) -> DriveResult<()> {
        let Some(closed) = self.closed.pop() else {
            return Ok(());
        };
        let tab = match self.new_tab(&closed.location) {
            Ok(tab) => tab,
            Err(_) => self.new_tab(&Location::ROOT)?,
        };
        let at = closed.at.min(self.tabs.len());
        let mut tab = tab;
        tab.changes = closed.changes;
        self.tabs.insert(at, tab);
        self.active = at;
        self.revision += 1;
        let Some((id, filling)) = closed.opened else {
            return Ok(());
        };
        let column = self.columns().len() - 1;
        let Some(row) = self.columns()[column]
            .rows
            .iter()
            .position(|entry| entry.id == id)
        else {
            return Ok(());
        };
        if filling {
            self.open_row(column, row)
        } else {
            self.select(column, row)
        }
    }

    /// Whether the tab at `index` can close: any but the last one open.
    pub fn can_close_tab(&self, index: usize) -> bool {
        self.tabs.len() > 1 && index < self.tabs.len()
    }

    /// Closes a tab; closing the active one activates its neighbour. The last tab stays open.
    /// It is kept for Reopen Closed Tab.
    pub fn close_tab(&mut self, index: usize) {
        if !self.can_close_tab(index) {
            return;
        }
        let tab = self.tabs.remove(index);
        if self.closed.len() == CLOSED_TABS_KEPT {
            self.closed.remove(0);
        }
        self.closed.push(ClosedTab {
            at: index,
            location: tab.location(),
            opened: tab.opened.map(|opened| (opened.id, tab.filling)),
            changes: tab.changes,
        });
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1).min(self.tabs.len() - 1);
        }
        self.revision += 1;
    }

    pub fn workspace(&mut self) -> &mut Workspace {
        &mut self.workspace
    }

    pub fn columns(&self) -> &[Column] {
        &self.tab().columns
    }

    pub fn opened(&self) -> Option<&Opened> {
        self.tab().opened.as_ref()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The folder each open column lists, for watching.
    pub fn directories(&mut self) -> Vec<std::path::PathBuf> {
        let locations: Vec<Location> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.columns.iter().map(|column| column.location.clone()))
            .collect();
        locations
            .iter()
            .filter_map(|location| self.workspace.require_directory(location).ok())
            .collect()
    }

    /// Makes a folder named "New folder" in `language` ("New folder 2"… when taken) in
    /// `column`'s folder, as Finder's New Folder does. It is listed and selected at once, ready
    /// to rename in place. Returns its id; none when `column` shows no folder.
    pub fn create_folder(&mut self, column: usize, language: &str) -> DriveResult<Option<String>> {
        let Some(location) = self.location_of(column) else {
            return Ok(None);
        };
        let title = crate::strings::text(language, crate::strings::key::DIALOG_NEW_FOLDER_NAME);
        let id = self.workspace.create_numbered_folder(&location, title)?;
        self.refresh_and_select(column, &id)?;
        Ok(Some(id))
    }

    /// Renames a row's file as typed (`renamed_title`).
    pub fn rename(&mut self, column: usize, row: usize, typed: &str) -> DriveResult<()> {
        let Some(id) = self
            .columns()
            .get(column)
            .and_then(|shown| shown.rows.get(row))
            .map(|entry| entry.id.clone())
        else {
            return Ok(());
        };
        self.rename_id(column, &id, typed)
    }

    pub(crate) fn location_of(&self, column: usize) -> Option<Location> {
        if column == usize::MAX {
            return self.scope.clone();
        }
        self.columns()
            .get(column)
            .map(|column| column.location.clone())
    }

    /// Relists the folders with `id` newly made in `column`. As columns it is selected there (a
    /// folder opens as the next column); as a board or list it is listed and the folder shown
    /// stays.
    fn refresh_and_select(&mut self, column: usize, id: &str) -> DriveResult<()> {
        self.refresh()?;
        if self.view != View::Columns {
            return Ok(());
        }
        let Some(row) = self
            .columns()
            .get(column)
            .and_then(|shown| shown.rows.iter().position(|entry| entry.id == id))
        else {
            return Ok(());
        };
        self.select(column, row)
    }

    /// Makes an empty page named "Untitled" in `language` ("Untitled 2"… when taken) in
    /// `column`'s folder, listed and selected, ready to rename in place. The file gets the `.md`
    /// extension people never type. Returns its id.
    pub fn create_page(&mut self, column: usize, language: &str) -> DriveResult<Option<String>> {
        let Some(location) = self.location_of(column) else {
            return Ok(None);
        };
        let title = crate::strings::text(language, crate::strings::key::DIALOG_NEW_PAGE_NAME);
        let id = self
            .workspace
            .create_file(&location, &format!("{title}.md"), b"")?;
        self.refresh_and_select(column, &id)?;
        Ok(Some(id))
    }

    /// Makes a terminal file in `column`'s folder, named with the terminal word in `language`
    /// (numbered when taken), listed and selected, ready to rename in place. Returns its id.
    pub fn create_terminal(
        &mut self,
        column: usize,
        language: &str,
    ) -> DriveResult<Option<String>> {
        let Some(location) = self.location_of(column) else {
            return Ok(None);
        };
        let name = format!(
            "{}.nterm",
            crate::strings::text(language, crate::strings::key::TERMINAL_SHELL_LABEL)
        );
        let id = self.workspace.create_file(
            &location,
            &name,
            crate::terminal::launch::TERMINAL_FILE.as_bytes(),
        )?;
        self.refresh_and_select(column, &id)?;
        Ok(Some(id))
    }

    /// Copies files and folders from anywhere on the computer into `column`'s folder, for an
    /// import or a drop from another app. A file already in that folder is skipped, as Finder
    /// does, so dragging the drive's files out and back onto their folder copies nothing.
    pub fn import(&mut self, column: usize, sources: &[PathBuf]) -> DriveResult<()> {
        let Some(location) = self.location_of(column) else {
            return Ok(());
        };
        let elsewhere: Vec<PathBuf> = sources
            .iter()
            .filter(|source| {
                source.parent().is_none_or(|parent| {
                    // A folder outside the drive is not this one.
                    self.workspace.location_at(parent).ok().as_ref() != Some(&location)
                })
            })
            .cloned()
            .collect();
        if elsewhere.is_empty() {
            return Ok(());
        }
        self.copy_in(&location, &elsewhere)
    }

    /// Copies the files `sources` into the folder `table` pastes into (`table_folder`).
    pub fn paste(&mut self, table: Table, sources: &[PathBuf]) -> DriveResult<()> {
        let Some(location) = self.table_folder(table) else {
            return Ok(());
        };
        self.copy_in(&location, sources)
    }

    /// Copies `sources` into the folder at `location` (`Workspace::copy_in`), keeps the copies
    /// as one change to undo, and lists the folders again.
    fn copy_in(&mut self, location: &Location, sources: &[PathBuf]) -> DriveResult<()> {
        let copied = self.workspace.copy_in(location, sources);
        if let Ok(ids) = &copied {
            let steps = self
                .paths_now(ids)
                .into_iter()
                .map(|(_, path)| Step::Made { path })
                .collect();
            self.history.record(steps);
        }
        // Files copied before a failure are in the folder and show.
        self.refresh()?;
        copied.map(|_| ())
    }

    /// Copies each of the files `ids` beside itself, numbered ("stem 2.ext"), as Duplicate
    /// does, and keeps the copies as one change to undo.
    pub fn duplicate(&mut self, ids: &[String]) -> DriveResult<()> {
        let mut made = Vec::new();
        let mut outcome = Ok(());
        for (_, path) in self.paths_now(ids) {
            let copied = path
                .parent()
                .map_or(Ok(Location::ROOT), |parent| {
                    self.workspace.location_at(parent)
                })
                .and_then(|location| {
                    self.workspace
                        .copy_in(&location, std::slice::from_ref(&path))
                });
            match copied {
                Ok(ids) => made.extend(ids),
                Err(error) => {
                    outcome = Err(error);
                    break;
                }
            }
        }
        let steps = self
            .paths_now(&made)
            .into_iter()
            .map(|(_, path)| Step::Made { path })
            .collect();
        self.history.record(steps);
        self.refresh()?;
        outcome
    }

    /// Moves the files `ids` to the trash and records one undoable change for those whose trash
    /// location is known. Files the trash gave no location for cannot be restored.
    pub fn trash(&mut self, ids: &[String]) -> DriveResult<()> {
        let mut steps = Vec::new();
        let mut outcome = Ok(());
        for id in ids {
            let trashed = self.workspace.path_of(id).and_then(|path| {
                let trashed = self.workspace.trash_path(&path)?;
                Ok((path, trashed))
            });
            match trashed {
                Ok((
                    path,
                    Trashed {
                        landed: Some(landed),
                        kept,
                    },
                )) => steps.push(Step::Trashed { path, landed, kept }),
                Ok((_, Trashed { landed: None, .. })) => {}
                Err(error) => {
                    outcome = Err(error);
                    break;
                }
            }
        }
        self.history.record(steps);
        self.refresh()?;
        outcome
    }

    /// The archive Compress writes of the files `ids`: beside the first, named in `language`.
    /// Refused where that folder takes no new file; `None` when none of the files is there.
    pub fn plan_compress(
        &mut self,
        ids: &[String],
        language: &str,
    ) -> DriveResult<Option<Compress>> {
        let sources = self
            .paths_now(ids)
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        let Some(plan) = Compress::of(sources, language) else {
            return Ok(None);
        };
        match self.workspace.refusal_in(&plan.directory) {
            Some(protection) => Err(DriveError::Refused(Message::new(protection.words()))),
            None => Ok(Some(plan)),
        }
    }

    /// Records the archive Compress wrote at `archive` as an undoable change (undo trashes it)
    /// when it is in this drive, then relists. The drive may have changed during the write.
    pub fn record_made(&mut self, archive: PathBuf) -> DriveResult<()> {
        if archive.starts_with(self.workspace.root()) {
            self.history.record(vec![Step::Made { path: archive }]);
        }
        self.refresh()
    }

    /// Takes the newest drive change made here back and lists the folders again. `None` when
    /// there is none.
    pub fn undo(&mut self) -> Option<DriveResult<()>> {
        let outcome = self.history.undo(&mut self.workspace)?;
        Some(self.refresh().and(outcome))
    }

    /// Does the drive change last undone again and lists the folders again. `None` when there
    /// is none.
    pub fn redo(&mut self) -> Option<DriveResult<()>> {
        let outcome = self.history.redo(&mut self.workspace)?;
        Some(self.refresh().and(outcome))
    }

    /// Where each of the files `ids` is now, by id; one that has gone is left out.
    fn paths_now(&mut self, ids: &[String]) -> Vec<(String, PathBuf)> {
        ids.iter()
            .filter_map(|id| Some((id.clone(), self.workspace.path_of(id).ok()?)))
            .collect()
    }

    /// Keeps as one change to undo the moves of the files that were at `before` and are
    /// somewhere else now.
    fn record_moves(&mut self, before: Vec<(String, PathBuf)>) {
        let steps = before
            .into_iter()
            .filter_map(|(id, from)| {
                let to = self.workspace.path_of(&id).ok()?;
                (to != from).then_some(Step::Moved { from, to })
            })
            .collect();
        self.history.record(steps);
    }

    /// The files selected in `table`, by id: the list's selected rows that are files, or a
    /// column's selected rows.
    pub fn selected_ids(&self, table: Table) -> Vec<String> {
        let rows = self.table_selection(table).rows;
        match table {
            Table::List => rows
                .iter()
                .filter_map(|&row| self.list_shown.get(row))
                .filter(|shown| !shown.placeholder)
                .map(|shown| shown.key.clone())
                .collect(),
            Table::Column(column) => self.columns().get(column).map_or_else(Vec::new, |shown| {
                rows.iter()
                    .filter_map(|&row| shown.rows.get(row))
                    .map(|entry| entry.id.clone())
                    .collect()
            }),
        }
    }

    /// The folder a paste in `table` copies into: for the list, the folder its selected files
    /// are in when they share one opened in place, else the folder shown; a column's own folder.
    pub fn table_folder(&self, table: Table) -> Option<Location> {
        match table {
            Table::List => {
                let selected = self.table_selection(Table::List).rows;
                list_scope(&self.list_shown, &selected).or_else(|| {
                    self.tab()
                        .columns
                        .last()
                        .map(|shown| shown.location.clone())
                })
            }
            Table::Column(column) => self
                .columns()
                .get(column)
                .map(|shown| shown.location.clone()),
        }
    }

    /// Renames the file with `id` in `column`'s folder as typed (`renamed_title`).
    pub fn rename_id(&mut self, column: usize, id: &str, typed: &str) -> DriveResult<()> {
        let Some(location) = self.location_of(column) else {
            return Ok(());
        };
        let rows = self.folder_rows(column)?;
        let Some(current) = rows.iter().find(|entry| entry.id == id) else {
            return Ok(());
        };
        let Some(title) = renamed_title(&current.title, typed) else {
            return Ok(());
        };
        let before = self.paths_now(&[id.to_owned()]);
        let outcome = self.workspace.rename(&location, id, title);
        self.record_moves(before);
        outcome?;
        self.refresh()
    }

    /// The files of `column`'s folder: the column's rows, or for the folder a command was
    /// scoped to (`list_command_column`) what it holds.
    fn folder_rows(&mut self, column: usize) -> DriveResult<Vec<Entry>> {
        if let Some(shown) = self.columns().get(column) {
            return Ok(shown.rows.clone());
        }
        match self.location_of(column) {
            Some(location) => self.workspace.all_contents(&location),
            None => Ok(Vec::new()),
        }
    }

    /// The files `ids` of `column`'s folder as it lists them, in the order of `ids`; one that
    /// has gone is left out.
    pub fn entries_of(&mut self, column: usize, ids: &[String]) -> DriveResult<Vec<Entry>> {
        let rows = self.folder_rows(column)?;
        Ok(ids
            .iter()
            .filter_map(|id| rows.iter().find(|entry| &entry.id == id).cloned())
            .collect())
    }

    /// The column that lists the file with `id`.
    pub fn column_of(&self, id: &str) -> Option<usize> {
        self.columns()
            .iter()
            .position(|column| column.rows.iter().any(|entry| entry.id == id))
    }

    /// Where a drag of `ids` lands over `row` of `column` (`None`: its empty space). A folder
    /// row takes it, as does a running terminal's (`takes_references`), unless the row is one
    /// of the dragged files; elsewhere in another column it lands in that column's folder.
    /// Nothing drops into a dragged folder, which rules out every column after it.
    pub fn column_drop_place(
        &self,
        ids: &[String],
        column: usize,
        row: Option<usize>,
        takes_references: impl Fn(&Entry) -> bool,
    ) -> ColumnDrop {
        let Some(source) = ids.first().and_then(|id| self.column_of(id)) else {
            return ColumnDrop::Nowhere;
        };
        let open = self.columns()[source]
            .selected
            .and_then(|selected| self.columns()[source].rows.get(selected));
        if column > source && open.is_some_and(|open| ids.contains(&open.id)) {
            return ColumnDrop::Nowhere;
        }
        let over = row
            .and_then(|row| Some((row, self.columns().get(column)?.rows.get(row)?)))
            .filter(|(_, entry)| !ids.contains(&entry.id));
        if let Some(drop) = over.and_then(|(row, entry)| row_drop(row, entry, &takes_references)) {
            return drop;
        }
        if source != column && column < self.columns().len() {
            ColumnDrop::Column
        } else {
            ColumnDrop::Nowhere
        }
    }

    /// Drops the files `ids` where `column_drop_place` puts them: into a folder's row or the
    /// column's own folder. A terminal's row is handed them by its session instead.
    pub fn column_drop(
        &mut self,
        ids: &[String],
        column: usize,
        row: Option<usize>,
        takes_references: impl Fn(&Entry) -> bool,
    ) -> DriveResult<()> {
        let Some(source) = ids.first().and_then(|id| self.column_of(id)) else {
            return Ok(());
        };
        let destination = match self.column_drop_place(ids, column, row, takes_references) {
            ColumnDrop::Folder(row) => Some(self.columns()[column].rows[row].id.clone()),
            ColumnDrop::Column => self.columns()[column].location.folder_id.clone(),
            ColumnDrop::Terminal(_) | ColumnDrop::Nowhere => return Ok(()),
        };
        self.move_ids(source, ids, destination.as_deref())
    }

    /// Moves files of `column`'s folder into the folder with `folder_id` (`None`: the drive's
    /// own). A folder is never moved into itself.
    pub fn move_ids(
        &mut self,
        column: usize,
        ids: &[String],
        folder_id: Option<&str>,
    ) -> DriveResult<()> {
        let Some(source) = self.location_of(column) else {
            return Ok(());
        };
        let destination = Location {
            folder_id: folder_id.map(str::to_owned),
        };
        if destination == source {
            return Ok(());
        }
        let moving: Vec<String> = ids
            .iter()
            .filter(|id| Some(id.as_str()) != folder_id)
            .cloned()
            .collect();
        if moving.is_empty() {
            return Ok(());
        }
        let before = self.paths_now(&moving);
        let outcome = self.workspace.move_items(&source, &moving, &destination);
        self.record_moves(before);
        self.refresh()?;
        outcome
    }

    /// The list's rows named by `ids` as a drop reads them, and the row with `target`'s id.
    fn list_drag_rows(
        &mut self,
        ids: &[String],
        target: Option<&str>,
    ) -> (
        Vec<crate::list::moves::Row>,
        Option<crate::list::moves::Row>,
    ) {
        let named: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        let mut inside: Vec<String> = Vec::new();
        let mut dragged = Vec::new();
        let mut found = None;
        for row in self.list_rows(0) {
            inside.truncate(row.depth as usize);
            if row.placeholder.is_some() {
                continue;
            }
            let read = crate::list::moves::Row {
                id: row.entry.id.clone(),
                parent: row.parent.folder_id.clone(),
                inside: inside.clone(),
                folder: row.entry.kind == EntryKind::Folder,
            };
            if row.expanded {
                inside.push(row.entry.id.clone());
            }
            if Some(read.id.as_str()) == target {
                found = Some(read.clone());
            }
            if named.contains(read.id.as_str()) {
                dragged.push(read);
            }
        }
        (dragged, found)
    }

    /// Whether a drop of the list's rows `ids` on the folder row `target` (`None`: the list's
    /// empty space, which is the folder shown) would move anything (`list::moves`).
    pub fn list_drop_takes(&mut self, ids: &[String], target: Option<&str>) -> bool {
        let shown = self
            .tab()
            .columns
            .last()
            .map(|shown| shown.location.folder_id.clone());
        let (dragged, found) = self.list_drag_rows(ids, target);
        match (target, found, shown) {
            (Some(_), Some(row), _) => crate::list::moves::takes(&dragged, &row),
            (None, _, Some(shown)) => {
                !crate::list::moves::plan(&dragged, shown.as_deref()).is_empty()
            }
            _ => false,
        }
    }

    /// Drops the list's rows `ids` on the folder row `target`, or on the list's empty space:
    /// each moves from the folder it is in, rows opened inside other folders included.
    pub fn list_drop(&mut self, ids: &[String], target: Option<&str>) -> DriveResult<()> {
        if !self.list_drop_takes(ids, target) {
            return Ok(());
        }
        let Some(shown) = self
            .tab()
            .columns
            .last()
            .map(|shown| shown.location.clone())
        else {
            return Ok(());
        };
        let destination = match target {
            Some(id) => Location {
                folder_id: Some(id.to_owned()),
            },
            None => shown,
        };
        let (dragged, _) = self.list_drag_rows(ids, target);
        let moves = crate::list::moves::plan(&dragged, destination.folder_id.as_deref());
        let moving: Vec<String> = moves
            .iter()
            .flat_map(|planned| planned.ids.iter().cloned())
            .collect();
        let before = self.paths_now(&moving);
        let outcome = moves.iter().try_for_each(|planned| {
            let source = Location {
                folder_id: planned.source.clone(),
            };
            self.workspace
                .move_items(&source, &planned.ids, &destination)
        });
        self.record_moves(before);
        self.refresh()?;
        outcome
    }

    /// The folder shown as the board shows it: in the list's sort order (Sort By or a list
    /// column), with ties and unsorted folders by name as Finder sorts names, and folders on top
    /// when the list keeps them there.
    pub fn board_rows(&self) -> Vec<Entry> {
        let mut rows = self
            .tab()
            .columns
            .last()
            .map(|column| column.rows.clone())
            .unwrap_or_default();
        let name = |entry: &Entry| match &entry.extension {
            Some(extension) if !entry.title.ends_with(&format!(".{extension}")) => {
                format!("{}.{extension}", entry.title)
            }
            _ => entry.title.clone(),
        };
        rows.sort_by(|left, right| crate::workspace::compare_names(&name(left), &name(right)));
        crate::list::sort_entries(&mut rows, self.list_sort, self.folders_first);
        rows
    }

    /// Selects one row, as a click in a column does. A folder shows its files as
    /// the next column; a file closes the columns after this one and shows beside it.
    pub fn select(&mut self, column: usize, row: usize) -> DriveResult<()> {
        self.show_row(column, row, EntryKind::holds_files)
    }

    /// Opens one row of the folder shown, as a double click on a board or in a list does. A
    /// folder becomes the folder shown; a file fills the window.
    fn open_at(&mut self, column: usize, row: usize) -> DriveResult<()> {
        self.show_row(column, row, EntryKind::holds_files)
    }

    /// When the row at `row` of `column` is in a group of two or more terminal files and
    /// nothing else, opens them side by side in the tab. Returns whether it did; otherwise the
    /// caller opens the row.
    pub fn open_terminals_together(&mut self, column: usize, row: usize) -> DriveResult<bool> {
        let table = if self.view == View::List {
            Table::List
        } else {
            Table::Column(column)
        };
        let Some(rows) = self.columns().get(column).map(|shown| shown.rows.clone()) else {
            return Ok(false);
        };
        let Some(id) = rows.get(row).map(|entry| entry.id.clone()) else {
            return Ok(false);
        };
        let together = self.terminals_together(table, &rows, &id);
        if together.is_empty() {
            return Ok(false);
        }
        self.open_at(column, row)?;
        let tab = self.tab_mut();
        if let Some(opened) = tab.opened.as_mut() {
            let names: Vec<&str> = together.iter().map(|(_, title)| title.as_str()).collect();
            opened.title = names.join(", ");
            opened.together = together;
        }
        tab.filling = tab.opened.is_some();
        self.revision += 1;
        Ok(true)
    }

    /// The group `table` has selected, by id and name in its order, when it holds two or more
    /// files, `id` among them, and every one is a terminal file of `rows`; empty otherwise.
    fn terminals_together(&self, table: Table, rows: &[Entry], id: &str) -> Vec<(String, String)> {
        let (keys, chosen) = self.table_chosen(table);
        if chosen.ids.len() < 2 || !chosen.ids.iter().any(|chosen| chosen == id) {
            return Vec::new();
        }
        let together: Vec<(String, String)> = keys
            .iter()
            .filter(|key| chosen.ids.contains(key))
            .filter_map(|key| {
                rows.iter()
                    .find(|entry| entry.id == *key && entry.kind == EntryKind::Terminal)
            })
            .map(|entry| (entry.id.clone(), entry.title.clone()))
            .collect();
        if together.len() == chosen.ids.len() {
            together
        } else {
            Vec::new()
        }
    }

    /// Selects the row at `row` of `column`: its files become the next column when `lists`
    /// says so of its kind, else it is the open file and the columns after this one close.
    fn show_row(
        &mut self,
        column: usize,
        row: usize,
        lists: fn(EntryKind) -> bool,
    ) -> DriveResult<()> {
        // What a selection selects shows beside its column until it is opened.
        self.tab_mut().filling = false;
        let Some(entry) = self
            .columns()
            .get(column)
            .and_then(|column| column.rows.get(row))
            .cloned()
        else {
            return Ok(());
        };
        let before = self.tab().location();
        // One row chosen: the column holds no group.
        self.tab_mut().columns[column].chosen.ids.clear();
        if lists(entry.kind) {
            self.list_inside(column, row, entry.id)?;
        } else {
            let tab = self.tab_mut();
            tab.columns.truncate(column + 1);
            tab.columns[column].selected = Some(row);
            tab.opened = Some(Opened {
                id: entry.id,
                kind: entry.kind,
                title: entry.title,
                together: Vec::new(),
                starts_playing: false,
            });
        }
        // A different folder is a visit: it joins the history and ends any way forward.
        let tab = self.tab_mut();
        if tab.location() != before {
            tab.past.push(before);
            tab.future.clear();
        }
        self.revision += 1;
        Ok(())
    }

    /// Lists the files of the folder `id`, at `row` of `column`, as the column
    /// after it; no file is open.
    fn list_inside(&mut self, column: usize, row: usize, id: String) -> DriveResult<()> {
        let location = Location {
            folder_id: Some(id),
        };
        let mut rows = self.workspace.all_contents(&location)?;
        crate::list::sort_entries(&mut rows, self.list_sort, self.folders_first);
        let tab = self.tab_mut();
        tab.columns.truncate(column + 1);
        tab.columns[column].selected = Some(row);
        tab.columns.push(Column::new(location, rows, None));
        tab.opened = None;
        Ok(())
    }

    /// Opens a row as a double click, ⌘↓ or ⌘O does: a folder becomes the next column, as selecting
    /// it does; a file fills the tab in the columns' place until Back closes it.
    pub fn open_row(&mut self, column: usize, row: usize) -> DriveResult<()> {
        if self.open_terminals_together(column, row)? {
            return Ok(());
        }
        self.open_at(column, row)?;
        // A file the window cannot show stays beside its column and opens in its application.
        if let Some(path) = self.opens_elsewhere(column, row) {
            self.elsewhere = Some(path);
            return Ok(());
        }
        let tab = self.tab_mut();
        tab.filling = tab.opened.is_some();
        self.revision += 1;
        Ok(())
    }

    /// Whether the open file fills the active tab in its folder's place: always as a board or
    /// a list, and as columns once it has been opened, not only selected.
    pub fn opened_fills_tab(&self) -> bool {
        self.file_fills_tab(self.active).is_some()
    }

    /// How many columns the window draws: none as a board or a list, nor while a file opened
    /// from them fills the tab in their place.
    pub fn drawn_columns(&self) -> usize {
        if self.view == View::Columns && !self.opened_fills_tab() {
            self.columns().len()
        } else {
            0
        }
    }

    /// Leaves `column` with no single row chosen: several are selected in it, or none. The
    /// columns after it close and no file is open, as when Finder's column holds a group.
    pub fn settle(&mut self, column: usize) {
        if column >= self.columns().len() {
            return;
        }
        let before = self.tab().location();
        let tab = self.tab_mut();
        tab.columns.truncate(column + 1);
        tab.columns[column].selected = None;
        tab.columns[column].chosen.ids.clear();
        tab.opened = None;
        if tab.location() != before {
            tab.past.push(before);
            tab.future.clear();
        }
        self.revision += 1;
    }

    /// Shows the folder the shown folder is in, as ⌘↑ in the list does; a visit like any
    /// other. The drive's own folder has none and stays.
    pub fn open_parent(&mut self) {
        let count = self.columns().len();
        if count > 1 {
            self.settle(count - 2);
        }
    }

    /// The rows the list now shows, as handed to the shell. Selection of rows that are gone is
    /// dropped, as is selection of placeholder rows, which a relisting never keeps.
    pub fn show_list(&mut self, rows: &[ListRow]) {
        self.list_shown = rows.iter().map(ListShown::of).collect();
        let files: std::collections::HashSet<&str> = self
            .list_shown
            .iter()
            .filter(|row| !row.placeholder)
            .map(|row| row.key.as_str())
            .collect();
        self.list_chosen.retain(|id| files.contains(id));
    }

    /// The keys of a table's rows, top first, borrowed from the rows.
    fn table_key_refs(&self, table: Table) -> Vec<&str> {
        match table {
            Table::List => self.list_shown.iter().map(|row| row.key.as_str()).collect(),
            Table::Column(column) => self
                .columns()
                .get(column)
                .map_or_else(Vec::new, Column::keys),
        }
    }

    /// The keys of a table's rows, top first, as owned strings for a change that follows.
    fn table_keys(&self, table: Table) -> Vec<String> {
        self.table_key_refs(table)
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    /// The rows a table shows as selected, with the row a range grows from and the row keys
    /// act from. A column with a row open shows that row, unless another was just picked up.
    pub fn table_selection(&self, table: Table) -> Selected {
        let keys = self.table_key_refs(table);
        match table {
            Table::List => self.list_chosen.on(&keys),
            Table::Column(column) => {
                let Some(shown) = self.columns().get(column) else {
                    return Selected::default();
                };
                let chosen = shown.chosen.on(&keys);
                match shown.selected {
                    Some(row) if shown.chosen.ids.is_empty() || shown.group_at != self.revision => {
                        Selected::read(&[row], chosen.anchor, chosen.focus)
                    }
                    _ => chosen,
                }
            }
        }
    }

    /// The selection a table shows, kept by its rows' keys.
    fn table_chosen(&self, table: Table) -> (Vec<String>, Chosen) {
        let keys = self.table_keys(table);
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
        let chosen = Chosen::of(&self.table_selection(table), &borrowed);
        (keys, chosen)
    }

    /// Sets a table's selection to `chosen`. In a column, a single row opens (a folder as the
    /// next column, a file beside it), while a group or nothing opens nothing and closes the
    /// columns after, as in Finder. Choosing the rows already shown only moves the anchor and
    /// focus.
    pub fn choose(&mut self, table: Table, chosen: Chosen) -> DriveResult<()> {
        let Table::Column(column) = table else {
            self.list_chosen = chosen;
            return Ok(());
        };
        if column >= self.columns().len() {
            return Ok(());
        }
        let (keys, _) = self.table_chosen(table);
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let after = chosen.on(&keys).rows;
        let (anchor, focus) = (chosen.anchor.clone(), chosen.focus.clone());
        if after != self.table_selection(table).rows {
            if let [row] = after[..] {
                self.select(column, row)?;
            } else {
                self.settle(column);
                let revision = self.revision;
                let shown = &mut self.tab_mut().columns[column];
                shown.chosen.ids = chosen.ids;
                shown.group_at = revision;
            }
        }
        let shown = &mut self.tab_mut().columns[column];
        shown.chosen.anchor = anchor;
        shown.chosen.focus = focus;
        Ok(())
    }

    /// A press on `row` of a table (`None`: the space under its rows) with `mode`'s keys held
    /// (`row_pointer`). The space under a column's rows is the column's own folder: what is
    /// selected in it goes and the columns after it close.
    pub fn press_row(&mut self, table: Table, row: Option<usize>, mode: Mode) -> DriveResult<()> {
        let (keys, chosen) = self.table_chosen(table);
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        match (row, table) {
            (Some(row), _) if row < keys.len() => {
                self.choose(table, chosen.press(&keys, row, mode))
            }
            (_, Table::Column(column)) => {
                self.settle(column);
                Ok(())
            }
            (_, Table::List) => self.choose(
                table,
                Chosen {
                    ids: Vec::new(),
                    ..chosen
                },
            ),
        }
    }

    /// A key that moves the selection (`row_keys::Intent::Move`). Returns the row it moved to.
    pub fn step_rows(
        &mut self,
        table: Table,
        direction: crate::list::row_keys::Direction,
        range: bool,
    ) -> DriveResult<Option<usize>> {
        let (keys, chosen) = self.table_chosen(table);
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        self.choose(table, chosen.step(&keys, direction, range))?;
        Ok(self.table_selection(table).focus)
    }

    /// The row at `row` alone, which the keys then act from.
    pub fn focus_row(&mut self, table: Table, row: usize) -> DriveResult<()> {
        let keys = self.table_keys(table);
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        if row >= keys.len() {
            return Ok(());
        }
        self.choose(table, Chosen::alone(&keys, row))
    }

    /// Every row of a table, as ⌘A selects them.
    pub fn select_all_rows(&mut self, table: Table) -> DriveResult<()> {
        let (keys, chosen) = self.table_chosen(table);
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        self.choose(table, chosen.all(&keys))
    }

    /// Escape: nothing is selected. A column's own folder is then the one worked in, and the
    /// columns after it close.
    pub fn clear_rows(&mut self, table: Table) -> DriveResult<()> {
        match table {
            Table::Column(column) => {
                self.settle(column);
                Ok(())
            }
            Table::List => {
                let (_, chosen) = self.table_chosen(table);
                self.choose(
                    table,
                    Chosen {
                        ids: Vec::new(),
                        ..chosen
                    },
                )
            }
        }
    }

    /// The table selected `rows` itself (a check selects rows): the core takes them as its
    /// choice, anchor and focus as they were.
    pub fn set_rows(&mut self, table: Table, rows: &[usize]) -> DriveResult<()> {
        let (keys, chosen) = self.table_chosen(table);
        let ids = rows
            .iter()
            .filter_map(|&row| keys.get(row).cloned())
            .collect();
        self.choose(table, Chosen { ids, ..chosen })
    }

    /// Handles `letter` typed in a table at `now` (`row_keys::Typed`). Selects alone the first
    /// row whose name starts with the recent letters, via `set_rows` (which opens it in a
    /// column). Returns that row, or `None` with nothing changed when no name matches.
    pub fn type_ahead(
        &mut self,
        table: Table,
        letter: char,
        now: std::time::Instant,
    ) -> DriveResult<Option<usize>> {
        let typed = self.typed.add(table, letter, now).to_owned();
        let names: Vec<Option<&str>> = match table {
            Table::List => self
                .list_shown
                .iter()
                .map(|row| (!row.placeholder).then_some(row.name.as_str()))
                .collect(),
            Table::Column(column) => self.columns().get(column).map_or_else(Vec::new, |shown| {
                shown
                    .rows
                    .iter()
                    .map(|entry| Some(entry.title.as_str()))
                    .collect()
            }),
        };
        let Some(row) = crate::list::row_keys::typed_row(&names, &typed) else {
            return Ok(None);
        };
        self.set_rows(table, &[row])?;
        Ok(Some(row))
    }

    /// Whether the file at `row` of a table may be picked up: one the system lets move, and a
    /// row of the list that is a file.
    fn movable(&self, table: Table, row: usize) -> bool {
        match table {
            Table::List => self.list_shown.get(row).is_some_and(|shown| shown.movable),
            Table::Column(column) => self
                .columns()
                .get(column)
                .and_then(|shown| shown.rows.get(row))
                .is_some_and(|entry| entry.protection.is_none()),
        }
    }

    /// What a drag starting on `row` of a table carries (`Selected::pick_up`), selecting without
    /// opening anything. Nothing when the row or any selected file is fixed by the system. A
    /// column whose folder is open in the next column keeps only that folder selected.
    pub fn pick_up_rows(&mut self, table: Table, row: usize) -> Vec<usize> {
        if !self.movable(table, row) {
            return Vec::new();
        }
        let shown = self.table_selection(table);
        let carried = if self.keeps_open_folder(table) && !shown.rows.contains(&row) {
            vec![row]
        } else {
            self.pick_up_selected(table, &shown, row)
        };
        // One file the system keeps in place stops the whole drag.
        if carried.iter().all(|&row| self.movable(table, row)) {
            carried
        } else {
            Vec::new()
        }
    }

    /// A column whose selected folder is open in the next one keeps that folder selected.
    fn keeps_open_folder(&self, table: Table) -> bool {
        match table {
            Table::List => false,
            Table::Column(column) => {
                self.columns()[column].selected.is_some() && column + 1 < self.columns().len()
            }
        }
    }

    /// Picks up `row` and what is selected with it from `shown`, the table's selection, and
    /// leaves the rest selected (`Selected::pick_up`).
    fn pick_up_selected(&mut self, table: Table, shown: &Selected, row: usize) -> Vec<usize> {
        let keys = self.table_keys(table);
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let (carried, left) = shown.pick_up(row);
        let changed = left.rows != shown.rows;
        let left = Chosen::of(&left, &keys);
        let revision = self.revision;
        match table {
            Table::List if changed => self.list_chosen = left,
            Table::List => {
                (self.list_chosen.anchor, self.list_chosen.focus) = (left.anchor, left.focus)
            }
            Table::Column(column) => {
                let shown = &mut self.tab_mut().columns[column];
                if changed {
                    shown.chosen.ids = left.ids;
                    shown.group_at = revision;
                }
                (shown.chosen.anchor, shown.chosen.focus) = (left.anchor, left.focus);
            }
        }
        carried
    }

    /// A right click on `row` of a table (`None`: its empty space). A row outside the
    /// selection is selected alone first. In a column this opens nothing, as in Finder: the row
    /// shows selected while its menu is open, and the open row shows again at the next change.
    /// The list's commands then act in the clicked file's folder. A column's empty space is its
    /// own folder and clears the selection. Returns the rows the menu is about: the selected
    /// files (in the list, only those in the clicked file's folder); none for empty space.
    pub fn context_rows(&mut self, table: Table, row: Option<usize>) -> DriveResult<Vec<usize>> {
        let Some(row) = row.filter(|&row| self.is_file_row(table, row)) else {
            self.right_click_empty(table);
            return Ok(Vec::new());
        };
        if !self.table_selection(table).rows.contains(&row) {
            self.select_alone(table, row)?;
        }
        let selected = self.table_selection(table).rows;
        if table != Table::List {
            return Ok(selected);
        }
        let (scope, group) = list_menu(&self.list_shown, &selected, row);
        self.list_scope = scope;
        Ok(group)
    }

    /// Whether `row` of `table` is a file, not a placeholder or past the rows.
    fn is_file_row(&self, table: Table, row: usize) -> bool {
        match table {
            Table::List => self
                .list_shown
                .get(row)
                .is_some_and(|shown| !shown.placeholder),
            Table::Column(column) => self
                .columns()
                .get(column)
                .is_some_and(|shown| row < shown.rows.len()),
        }
    }

    /// A right click on `table`'s empty space: the list's commands act in the folder shown
    /// again, and a column's selection clears.
    fn right_click_empty(&mut self, table: Table) {
        match table {
            Table::List => self.list_scope = None,
            Table::Column(column) => {
                if !self.table_selection(table).rows.is_empty() {
                    self.settle(column);
                }
            }
        }
    }

    /// Selects `row` of `table` alone, for its menu. A column only marks it chosen, opening
    /// nothing (`context_rows`).
    fn select_alone(&mut self, table: Table, row: usize) -> DriveResult<()> {
        let (keys, chosen) = self.table_chosen(table);
        let ids = vec![keys[row].clone()];
        match table {
            Table::List => self.choose(table, Chosen { ids, ..chosen })?,
            Table::Column(column) => {
                let revision = self.revision;
                let shown = &mut self.tab_mut().columns[column];
                shown.chosen.ids = ids;
                shown.group_at = revision;
            }
        }
        Ok(())
    }

    /// The single selected file to rename on Return or F2. None when zero or several are
    /// selected, or the system fixes the file's name. The list's commands then act in that
    /// file's folder.
    pub fn rename_row(&mut self, table: Table) -> Option<usize> {
        let selected = self.table_selection(table).rows;
        if table == Table::List {
            self.list_scope = list_scope(&self.list_shown, &selected);
        }
        let files: Vec<usize> = match table {
            Table::List => selected
                .iter()
                .copied()
                .filter(|&row| !self.list_shown[row].placeholder)
                .collect(),
            Table::Column(_) => selected,
        };
        match files[..] {
            [row] if self.movable(table, row) => Some(row),
            _ => None,
        }
    }

    /// The column the list's commands act in: the folder shown, or the folder opened in place
    /// a menu or key last named (`usize::MAX`, with the scope set to it).
    pub fn list_command_column(&mut self) -> usize {
        match self.list_scope.clone() {
            Some(folder) => {
                self.scope = Some(folder);
                usize::MAX
            }
            None => self.columns().len().saturating_sub(1),
        }
    }

    /// ← (`previous`) or → in `column`: the column the keys move to, if any. ← reselects this
    /// column's folder in the column before; → moves to the first row of the open folder's
    /// column when one row is selected here and that column has rows.
    pub fn step_column(&mut self, column: usize, previous: bool) -> DriveResult<Option<usize>> {
        if previous {
            let Some(before) = column.checked_sub(1) else {
                return Ok(None);
            };
            let Some(open) = self.columns().get(before).and_then(|shown| shown.selected) else {
                return Ok(None);
            };
            self.select(before, open)?;
            return Ok(Some(before));
        }
        let shown = self.table_selection(Table::Column(column)).rows;
        let one_open = matches!(shown[..], [row] if self.columns().get(column).is_some_and(|at| at.selected == Some(row)));
        let next_has_rows = self
            .columns()
            .get(column + 1)
            .is_some_and(|next| !next.rows.is_empty());
        if !(one_open && next_has_rows) {
            return Ok(None);
        }
        self.select(column + 1, 0)?;
        Ok(Some(column + 1))
    }

    /// Lists every open column again, keeping each selection by the file's id.
    pub fn refresh(&mut self) -> DriveResult<()> {
        for tab in 0..self.tabs.len() {
            self.refresh_tab(tab);
        }
        self.revision += 1;
        Ok(())
    }

    fn refresh_tab(&mut self, tab: usize) {
        for index in 0..self.tabs[tab].columns.len() {
            let column = &self.tabs[tab].columns[index];
            let location = column.location.clone();
            let had_selection = column.selected.is_some();
            let selected_id = column
                .selected
                .and_then(|row| column.rows.get(row))
                .map(|entry| entry.id.clone());
            let (sort, folders_first) = self.order();
            match self.workspace.all_contents(&location) {
                Ok(mut rows) => {
                    crate::list::sort_entries(&mut rows, sort, folders_first);
                    let selected =
                        selected_id.and_then(|id| rows.iter().position(|entry| entry.id == id));
                    let tab = &mut self.tabs[tab];
                    let listed: std::collections::HashSet<&str> =
                        rows.iter().map(|entry| entry.id.as_str()).collect();
                    tab.columns[index].chosen.retain(|id| listed.contains(id));
                    tab.columns[index].rows = rows;
                    tab.columns[index].selected = selected;
                    if had_selection && selected.is_none() {
                        // What was selected has gone, and with it the columns it opened.
                        tab.columns.truncate(index + 1);
                        tab.opened = None;
                        return;
                    }
                }
                Err(_) => {
                    // The folder itself has gone: close it and everything after it.
                    let tab = &mut self.tabs[tab];
                    tab.columns.truncate(index.max(1));
                    if let Some(parent) = tab.columns.last_mut() {
                        parent.selected = None;
                    }
                    tab.opened = None;
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_view_is_kept_by_its_lowercase_name_and_menus_number_it() {
        for (view, name) in [
            (View::Board, "board"),
            (View::List, "list"),
            (View::Columns, "columns"),
        ] {
            let kept = serde_json::to_string(&ViewChoices {
                view,
                ..ViewChoices::default()
            })
            .expect("choices");
            assert!(kept.contains(&format!("\"view\":\"{name}\"")), "{kept}");
            assert_eq!(view.name(), name);
            assert_eq!(View::numbered(view.number()), Some(view));
        }
        assert_eq!(View::numbered(3), None);
        // A view this version does not know shows as columns and keeps the other choices.
        let odd: ViewChoices =
            serde_json::from_str(r#"{"view":"grid","hiddenShown":true}"#).expect("choices");
        assert_eq!((odd.view, odd.hidden_shown), (View::Columns, true));
    }

    fn columns() -> (tempfile::TempDir, tempfile::TempDir, Columns) {
        let root = tempfile::tempdir().expect("root");
        let metadata = tempfile::tempdir().expect("metadata");
        fs::write(root.path().join("AGENTS.md"), b"x").expect("page");
        fs::write(root.path().join("Claude Code.nterm"), b"{}").expect("terminal");
        fs::create_dir_all(root.path().join("Films").join("Shots")).expect("folders");
        fs::write(root.path().join("Films").join("cut.mp4"), b"x").expect("clip");
        let workspace = Workspace::open(root.path(), metadata.path()).expect("open");
        (root, metadata, Columns::open(workspace).expect("columns"))
    }

    fn titles(column: &Column) -> Vec<&str> {
        column
            .rows
            .iter()
            .map(|entry| entry.title.as_str())
            .collect()
    }

    #[test]
    fn a_folder_opens_the_next_column_and_a_file_fills_the_window() {
        let (_root, _metadata, mut columns) = columns();
        assert_eq!(
            titles(&columns.columns()[0]),
            ["AGENTS", "Claude Code", "Films"]
        );

        columns.select(0, 2).expect("select folder");
        assert_eq!(columns.columns().len(), 2);
        assert_eq!(titles(&columns.columns()[1]), ["Shots", "cut.mp4"]);
        assert_eq!(columns.opened(), None);

        columns.select(1, 0).expect("select inner folder");
        assert_eq!(columns.columns().len(), 3);

        columns.select(0, 1).expect("select terminal");
        assert_eq!(columns.columns().len(), 1);
        let opened = columns.opened().expect("opened");
        assert_eq!(
            (opened.kind, opened.title.as_str()),
            (EntryKind::Terminal, "Claude Code")
        );
    }

    #[test]
    fn a_column_holding_a_group_closes_the_columns_after_it() {
        let (_root, _metadata, mut columns) = columns();
        columns.select(0, 2).expect("Films");
        columns.select(1, 0).expect("Shots");
        assert_eq!(columns.columns().len(), 3);
        columns.settle(1);
        assert_eq!(columns.columns().len(), 2);
        assert_eq!(columns.columns()[1].selected, None);
        assert_eq!(
            columns.columns()[0].selected,
            Some(2),
            "the folder it is in stays open"
        );
        columns.select(1, 1).expect("the clip");
        columns.settle(1);
        assert_eq!(columns.opened(), None);
        columns.settle(9);
        assert_eq!(
            columns.columns().len(),
            2,
            "a column that is not there changes nothing"
        );
    }

    #[test]
    fn the_enclosing_folder_opens_as_a_visit_and_the_drive_has_none() {
        let (_root, _metadata, mut columns) = columns();
        columns.open_parent();
        assert_eq!(columns.columns().len(), 1);
        assert!(
            !columns.can_go_back(),
            "the drive's own folder stays, and nothing was visited"
        );
        columns.select(0, 2).expect("Films");
        columns.select(1, 0).expect("Shots");
        columns.open_parent();
        assert_eq!(columns.columns().len(), 2);
        assert_eq!(columns.columns()[1].selected, None);
        columns.go_back().expect("back");
        assert_eq!(
            columns.columns().len(),
            3,
            "Back returns to the folder it came from"
        );
        // An open file closes with it.
        columns.select(2, 0).ok();
        columns.go_back().expect("back");
        columns.select(1, 1).expect("the clip");
        columns.open_parent();
        assert_eq!(columns.opened(), None);
        assert_eq!(columns.columns().len(), 1);
    }

    #[test]
    fn a_tab_is_named_and_marked_for_what_it_holds() {
        let (_root, _metadata, mut columns) = columns();
        columns.set_view(View::Board);
        assert_eq!(columns.tab_kind(0), 0);
        columns.select(0, 2).expect("Films");
        assert_eq!(
            (columns.tab_kind(0), columns.tab_title(0).as_str()),
            (1, "Films")
        );
        // As a board the open file fills the tab, which takes its name and a file's glyph.
        columns.select(1, 1).expect("the clip");
        assert_eq!(
            (columns.tab_kind(0), columns.tab_title(0).as_str()),
            (3, "cut.mp4")
        );
        // As columns the file shows beside its folder, and the tab stays the folder's.
        columns.set_view(View::Columns);
        assert_eq!(
            (columns.tab_kind(0), columns.tab_title(0).as_str()),
            (1, "Films")
        );
        columns.select(0, 1).expect("the terminal");
        columns.set_view(View::Board);
        assert_eq!(columns.tab_kind(0), 5);
    }

    /// A page five folders down, in folders named with spaces and other alphabets: selected,
    /// it shows beside its column; opened, it fills the tab; Back puts it beside its column
    /// again; and its path is the file's own.
    #[test]
    fn a_page_deep_in_folders_opens_and_fills_the_tab() {
        let (root, _metadata, mut columns) = columns();
        let deep = root
            .path()
            .join("Deep one/Två mapp/三 folder/Four four/Five");
        std::fs::create_dir_all(&deep).expect("folders");
        std::fs::write(deep.join("Deep page.md"), "# Deep page\n\nWords.\n").expect("page");
        columns.refresh().expect("refresh");
        columns.set_view(View::Columns);
        for name in ["Deep one", "Två mapp", "三 folder", "Four four", "Five"] {
            let last = columns.columns().len() - 1;
            let row = columns.columns()[last]
                .rows
                .iter()
                .position(|entry| entry.title == name)
                .expect(name);
            columns.select(last, row).expect("the folder opens");
        }
        assert_eq!(columns.columns().len(), 6);
        let row = columns.columns()[5]
            .rows
            .iter()
            .position(|entry| entry.title == "Deep page")
            .expect("the page");
        columns.select(5, row).expect("selected");
        assert_eq!(
            columns.opened().map(|opened| opened.kind),
            Some(EntryKind::Page)
        );
        assert!(
            !columns.opened_fills_tab(),
            "selected, it shows beside its column"
        );
        assert_eq!(
            (columns.tab_kind(0), columns.tab_title(0).as_str()),
            (1, "Five")
        );

        columns.open_row(5, row).expect("opened");
        assert!(columns.opened_fills_tab(), "opened, it fills the tab");
        assert_eq!(
            (columns.tab_kind(0), columns.tab_title(0).as_str()),
            (3, "Deep page")
        );
        let id = columns.opened().expect("open").id.clone();
        assert_eq!(
            columns.workspace().path_of(&id).expect("path"),
            std::fs::canonicalize(deep.join("Deep page.md")).expect("real")
        );

        columns.go_back().expect("back");
        assert_eq!(
            columns.opened().map(|opened| opened.title.as_str()),
            Some("Deep page"),
            "still the file selected"
        );
        assert!(!columns.opened_fills_tab(), "beside its column again");
        assert_eq!(columns.columns().len(), 6, "the columns are as they were");
        assert!(
            !columns.can_go_forward(),
            "Forward does not open a closed file again"
        );

        // A folder opened is the folder selected: its column, and nothing fills the tab.
        columns
            .open_row(
                0,
                columns.columns()[0]
                    .rows
                    .iter()
                    .position(|entry| entry.title == "Deep one")
                    .expect("row"),
            )
            .expect("folder");
        assert!(columns.opened().is_none() && !columns.opened_fills_tab());
        assert_eq!(columns.columns().len(), 2);
    }

    #[test]
    fn back_and_forward_walk_the_folders_visited() {
        let (_root, _metadata, mut columns) = columns();
        columns.set_view(View::Board);
        assert!(!columns.can_go_back());
        columns.select(0, 2).expect("Films");
        columns.select(1, 0).expect("Shots");
        assert_eq!(columns.columns().len(), 3);
        // Opening a file is not a visit: the folder shown is still Films.
        columns.select(1, 1).expect("the clip");
        assert_eq!(columns.columns().len(), 2);

        columns.go_back().expect("back");
        assert_eq!(columns.opened(), None, "back first closes the open file");
        assert_eq!(columns.columns().len(), 2);
        columns.go_back().expect("back");
        assert_eq!(columns.columns().len(), 3, "back to Shots");
        columns.go_back().expect("back");
        assert_eq!(columns.columns().len(), 2, "back to Films");
        assert_eq!(columns.columns()[0].selected, Some(2));
        columns.go_back().expect("back");
        assert_eq!(columns.columns().len(), 1);
        assert!(!columns.can_go_back());

        columns.go_forward().expect("forward");
        assert_eq!(columns.columns().len(), 2);
        // A new visit ends the way forward.
        columns.select(0, 2).expect("Films again");
        columns.select(1, 0).expect("Shots");
        assert!(!columns.can_go_forward());
    }

    /// A file the window cannot show goes to its own application: the folder stays as it is
    /// and nothing fills the tab.
    // It makes symbolic links, as Unix makes them.
    #[cfg(unix)]
    #[test]
    fn a_file_with_no_viewer_opens_in_its_application() {
        let (root, _metadata, mut columns) = columns();
        fs::write(root.path().join("archive.zip"), b"PK\x03\x04\x00\x00binary").expect("zip");
        fs::write(root.path().join("notes.log"), b"words").expect("text");
        columns.refresh().expect("refresh");
        let id = |columns: &Columns, name: &str| {
            columns.columns()[0]
                .rows
                .iter()
                .find(|entry| entry.title == name)
                .expect(name)
                .id
                .clone()
        };
        let zip = id(&columns, "archive.zip");
        columns.open_in_folder(&zip).expect("open");
        assert!(columns.opened().is_none(), "nothing opens in the window");
        let path = columns
            .take_opened_elsewhere()
            .expect("handed to its application");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("archive.zip")
        );
        assert!(columns.take_opened_elsewhere().is_none(), "once");
        // As columns, it stays selected beside its column.
        columns.set_view(View::Columns);
        let row = columns.columns()[0]
            .rows
            .iter()
            .position(|entry| entry.id == zip)
            .expect("row");
        columns.open_row(0, row).expect("open");
        assert!(!columns.opened_fills_tab());
        assert!(columns.take_opened_elsewhere().is_some());
        // A link to a page outside the drive is listed and does not open.
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("Elsewhere.md"), b"# Elsewhere\n").expect("page");
        std::os::unix::fs::symlink(
            outside.path().join("Elsewhere.md"),
            root.path().join("Elsewhere.md"),
        )
        .expect("link");
        columns.refresh().expect("refresh");
        columns.set_view(View::List);
        let link = id(&columns, "Elsewhere");
        columns.open_in_folder(&link).expect("open");
        assert!(
            columns.opened().is_none_or(|opened| opened.id != link)
                && columns.take_opened_elsewhere().is_none()
        );
        // Text shows in the window.
        let log = id(&columns, "notes.log");
        columns.open_in_folder(&log).expect("open");
        assert!(columns.take_opened_elsewhere().is_none());
        assert_eq!(
            columns.opened().map(|opened| opened.title.as_str()),
            Some("notes.log")
        );
    }

    /// The header menu's options: folders on top and everything restored; Sort By puts the
    /// list back in name order.
    #[test]
    fn the_list_header_options_change_the_rows() {
        let (_root, metadata, mut columns) = columns();
        let names = |columns: &mut Columns| -> Vec<String> {
            columns
                .list_rows(0)
                .into_iter()
                .map(|row| row.entry.title)
                .collect()
        };
        columns.toggle_list_sort(crate::list::SortKey::Name);
        columns.toggle_list_sort(crate::list::SortKey::Name);
        assert_eq!(names(&mut columns), ["Films", "Claude Code", "AGENTS"]);
        columns.sort_by(crate::list::SortKey::Name);
        assert_eq!(names(&mut columns), ["AGENTS", "Claude Code", "Films"]);
        columns.toggle_folders_first();
        assert_eq!(
            names(&mut columns),
            ["Films", "AGENTS", "Claude Code"],
            "the folder first"
        );

        columns.toggle_list_column(crate::list::Column::Extension);
        columns.set_list_width(crate::list::Column::Size, 150.0);
        columns.reset_list_options();
        assert_eq!(names(&mut columns), ["AGENTS", "Claude Code", "Films"]);
        assert_eq!(columns.list_columns(), crate::list::Column::DEFAULT);
        assert_eq!(
            columns.list_width(crate::list::Column::Size),
            crate::list::Column::Size.widths().0
        );
        assert!(!columns.folders_first());

        // "Keep folders on top" is kept with the view.
        columns.toggle_folders_first();
        let kept = std::fs::read_to_string(metadata.path().join(VIEW_CHOICES_FILE)).expect("kept");
        assert!(kept.contains("\"listFoldersFirst\": true"), "{kept}");
    }

    #[test]
    fn the_list_shows_the_folder_in_the_chosen_order() {
        let (_root, _metadata, mut columns) = columns();
        let names = |columns: &mut Columns| -> Vec<String> {
            columns
                .list_rows(0)
                .into_iter()
                .map(|row| row.entry.title)
                .collect()
        };
        assert_eq!(names(&mut columns), ["AGENTS", "Claude Code", "Films"]);
        columns.toggle_list_sort(crate::list::SortKey::Name);
        columns.toggle_list_sort(crate::list::SortKey::Name);
        assert_eq!(names(&mut columns), ["Films", "Claude Code", "AGENTS"]);
        // The cells follow the columns after Name: Date Modified, Size, Kind.
        assert_eq!(columns.list_rows(0)[0].cells[2], "Folder");
        assert_eq!(columns.list_rows(0)[0].cells[1], "--");

        // A chosen column joins at the right end, sorts like any other, and can be dragged
        // only within its limits.
        columns.toggle_list_column(crate::list::Column::Extension);
        assert_eq!(columns.list_columns().len(), 5);
        let agents = columns
            .list_rows(0)
            .into_iter()
            .find(|row| row.entry.title == "AGENTS")
            .expect("page");
        assert_eq!(agents.cells[3], "md");
        columns.toggle_list_sort(crate::list::Column::Extension);
        assert_eq!(
            names(&mut columns),
            ["AGENTS", "Claude Code", "Films"],
            "md, nterm, then the folder with none"
        );
        columns.set_list_width(crate::list::Column::Extension, 5000.0);
        assert_eq!(columns.list_width(crate::list::Column::Extension), 200.0);
        columns.toggle_list_column(crate::list::Column::Extension);
        assert_eq!(
            columns.list_sort(),
            None,
            "a hidden column no longer sorts the list"
        );
        columns.toggle_list_sort(crate::list::SortKey::Name);
        columns.toggle_list_sort(crate::list::SortKey::Name);

        let films = columns.list_rows(0)[0].entry.id.clone();
        columns.open_in_folder(&films).expect("open the folder");
        assert_eq!(
            names(&mut columns),
            ["Shots", "cut.mp4"],
            "descending by name"
        );
        assert!(columns.can_go_back());
    }

    #[test]
    fn a_folder_opens_in_place_in_the_list_and_its_files_take_commands() {
        let (root, _metadata, mut columns) = columns();
        let films = columns.list_rows(0)[2].entry.id.clone();
        columns.toggle_expanded(&films);
        let shown: Vec<(String, u32)> = columns
            .list_rows(0)
            .into_iter()
            .map(|row| (row.entry.title, row.depth))
            .collect();
        assert_eq!(
            shown,
            [
                ("AGENTS".into(), 0),
                ("Claude Code".into(), 0),
                ("Films".into(), 0),
                ("Shots".into(), 1),
                ("cut.mp4".into(), 1)
            ]
        );
        assert!(columns.list_rows(0)[2].expanded);

        // An open folder with nothing in it says so.
        let shots = columns.list_rows(0)[3].entry.id.clone();
        columns.toggle_expanded(&shots);
        let rows = columns.list_rows(0);
        assert_eq!(
            (rows[4].placeholder, rows[4].depth),
            (Some(Placeholder::Empty), 2)
        );
        assert_eq!(Placeholder::Empty.words("en"), "Empty folder");
        assert_eq!(
            Placeholder::Unreadable.words("en"),
            "Couldn’t open this folder"
        );

        // A nested file is renamed in the folder it is in.
        let clip = rows[5].entry.id.clone();
        assert_eq!(rows[5].parent.folder_id.as_deref(), Some(films.as_str()));
        columns.scope = Some(Location {
            folder_id: Some(films.clone()),
        });
        columns
            .rename_id(usize::MAX, &clip, "final.mp4")
            .expect("rename");
        assert!(root.path().join("Films").join("final.mp4").exists());

        columns.toggle_expanded(&films);
        assert_eq!(columns.list_rows(0).len(), 3);
    }

    #[test]
    fn opening_one_folder_in_place_opens_no_other() {
        let root = tempfile::tempdir().expect("root");
        let metadata = tempfile::tempdir().expect("metadata");
        fs::create_dir_all(root.path().join("Empty")).expect("folder");
        fs::create_dir_all(root.path().join("Folder A").join("Deeper")).expect("folder");
        fs::create_dir_all(root.path().join("Folder A").join("Inner")).expect("folder");
        let mut columns =
            Columns::open(Workspace::open(root.path(), metadata.path()).expect("open"))
                .expect("columns");
        for title in ["Empty", "Folder A"] {
            let id = columns
                .list_rows(0)
                .into_iter()
                .find(|row| row.entry.title == title)
                .expect(title)
                .entry
                .id;
            columns.toggle_expanded(&id);
        }
        let shown: Vec<(String, u32, bool)> = columns
            .list_rows(0)
            .into_iter()
            .map(|row| {
                (
                    row.placeholder.map_or(row.entry.title, |placeholder| {
                        placeholder.words("en").to_owned()
                    }),
                    row.depth,
                    row.expanded,
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("Empty".to_owned(), 0, true),
                ("Empty folder".to_owned(), 1, false),
                ("Folder A".to_owned(), 0, true),
                ("Deeper".to_owned(), 1, false),
                ("Inner".to_owned(), 1, false),
            ]
        );
    }

    #[test]
    fn a_path_asked_for_is_gone_to_as_a_visit_back_returns_from() {
        let root = tempfile::tempdir().expect("root");
        let metadata = tempfile::tempdir().expect("metadata");
        let deep = root.path().join("Docs").join("Deep");
        fs::create_dir_all(&deep).expect("folders");
        fs::write(deep.join("plan.md"), b"# Plan").expect("page");
        let mut columns =
            Columns::open(Workspace::open(root.path(), metadata.path()).expect("open"))
                .expect("columns");
        columns.set_view(View::Board);

        columns.go_to_path(&deep).expect("a folder");
        let shown = |columns: &Columns| -> Vec<String> {
            titles(columns.columns().last().expect("shown"))
                .into_iter()
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(shown(&columns), ["plan"]);
        assert!(columns.opened().is_none());

        columns.go_to_path(&deep.join("plan.md")).expect("a file");
        assert_eq!(columns.opened().expect("opened").title, "plan");
        columns.go_back().expect("back");
        assert!(columns.opened().is_none(), "Back closes the file first");
        assert_eq!(shown(&columns), ["plan"]);
        columns.go_back().expect("back");
        assert_eq!(shown(&columns), ["Docs"], "then returns to where it was");

        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("x.txt"), b"x").expect("outside file");
        assert!(columns.go_to_path(&outside.path().join("x.txt")).is_err());
        assert!(columns.go_to_path(&root.path().join("missing")).is_err());
        assert_eq!(shown(&columns), ["Docs"], "a refused path changes nothing");
    }

    #[test]
    fn the_view_is_as_it_was_left_and_hidden_files_show_when_asked() {
        use crate::list::Column;
        let root = tempfile::tempdir().expect("root");
        let metadata = tempfile::tempdir().expect("metadata");
        fs::write(root.path().join("notes.txt"), b"x").expect("file");
        fs::write(root.path().join(".env"), b"x").expect("hidden");
        let open = || {
            Columns::open(Workspace::open(root.path(), metadata.path()).expect("open"))
                .expect("columns")
        };

        let mut columns = open();
        assert_eq!(
            columns.view(),
            View::Columns,
            "a drive never opened before shows as columns"
        );
        assert_eq!(titles(&columns.columns()[0]), ["notes.txt"]);
        columns.set_view(View::List);
        columns.toggle_list_column(Column::Extension);
        columns.toggle_list_column(Column::Owner);
        columns.move_list_column(Column::Owner, 1);
        columns.set_list_width(Column::Size, 120.0);
        columns.toggle_list_sort(Column::Size);
        assert!(columns.toggle_hidden().expect("show hidden"));
        assert_eq!(titles(&columns.columns()[0]), [".env", "notes.txt"]);

        let again = open();
        assert_eq!(again.view(), View::List);
        assert_eq!(
            again.list_columns(),
            [
                Column::Name,
                Column::Owner,
                Column::Modified,
                Column::Size,
                Column::Kind,
                Column::Extension
            ]
        );
        assert_eq!(again.list_width(Column::Size), 120.0);
        assert_eq!(
            again.list_sort().map(|sort| (sort.key, sort.ascending)),
            Some((Column::Size, true))
        );
        assert_eq!(
            titles(&again.columns()[0]),
            [".env", "notes.txt"],
            "hidden files are still shown"
        );
    }

    #[test]
    fn the_file_index_fills_its_columns_only_while_one_is_shown() {
        use crate::list::Column;
        use crate::workspace::Indexed;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let (_root, _metadata, mut columns) = columns();
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&asked);
        columns.workspace().set_file_index(Box::new(move |path| {
            counter.fetch_add(1, Ordering::SeqCst);
            path.ends_with("AGENTS.md").then(|| Indexed::parse("width\t1920\nheight\t1080\nduration\t12.04\nauthor\tSam\nauthor\tLee\nbogus\tx\n"))
        }));
        columns.set_view(View::List);
        columns.list_rows(0);
        assert_eq!(
            asked.load(Ordering::SeqCst),
            0,
            "nothing shown needs the index"
        );

        columns.toggle_list_column(Column::Dimensions);
        columns.toggle_list_column(Column::Duration);
        columns.toggle_list_column(Column::Author);
        let rows = columns.list_rows(0);
        assert!(asked.load(Ordering::SeqCst) >= 3);
        let page = rows
            .iter()
            .find(|row| row.entry.title == "AGENTS")
            .expect("page");
        assert_eq!(&page.cells[3..], ["1920 × 1080", "12 s", "Sam, Lee"]);
        let folder = rows
            .iter()
            .find(|row| row.entry.title == "Films")
            .expect("folder");
        assert_eq!(&folder.cells[3..], ["--", "--", "--"]);
        assert_eq!(crate::list::format_duration(3.46), "3.5 s");

        for column in [Column::Dimensions, Column::Duration, Column::Author] {
            columns.toggle_list_column(column);
        }
        let before = asked.load(Ordering::SeqCst);
        columns.list_rows(0);
        columns.refresh().expect("refresh");
        assert_eq!(
            asked.load(Ordering::SeqCst),
            before,
            "the index is left alone again"
        );
    }

    #[test]
    fn each_tab_keeps_its_own_place_and_the_last_one_stays() {
        let (_root, _metadata, mut columns) = columns();
        columns.select(0, 2).expect("Films");
        columns.open_tab().expect("new tab");
        assert_eq!((columns.tab_count(), columns.active_tab()), (2, 1));
        assert_eq!(
            columns.columns().len(),
            1,
            "a new tab starts at the drive's folder"
        );
        assert_eq!(columns.tab_title(0), "Films");

        columns.select_tab(0);
        assert_eq!(columns.columns().len(), 2);
        assert!(columns.can_go_back());

        columns.close_tab(0);
        assert_eq!((columns.tab_count(), columns.active_tab()), (1, 0));
        assert_eq!(columns.columns().len(), 1);
        columns.close_tab(0);
        assert_eq!(columns.tab_count(), 1);
    }

    #[test]
    fn a_closed_tab_reopens_where_it_stood_with_its_file_open() {
        let (_root, _metadata, mut columns) = columns();
        assert!(!columns.can_reopen_closed_tab());
        // A second tab with the terminal filling it, and a third on the drive's folder.
        let terminal = columns.columns()[0].rows[1].id.clone();
        columns.open_tab().expect("second");
        columns.open_row(0, 1).expect("terminal");
        columns.open_tab().expect("third");
        assert_eq!(columns.tab_count(), 3);

        columns.close_tab(1);
        columns.close_tab(1);
        assert_eq!(columns.tab_count(), 1);
        assert!(columns.can_reopen_closed_tab());

        // The last closed comes back first, then the one before it, each where it stood.
        columns.reopen_closed_tab().expect("third back");
        assert_eq!((columns.tab_count(), columns.active_tab()), (2, 1));
        assert_eq!(columns.tab_terminal(1), None);
        columns.reopen_closed_tab().expect("second back");
        assert_eq!((columns.tab_count(), columns.active_tab()), (3, 1));
        assert_eq!(
            columns.tab_terminal(1).map(|opened| opened.id.clone()),
            Some(terminal)
        );
        assert!(!columns.can_reopen_closed_tab());
        // Nothing left: nothing changes.
        columns.reopen_closed_tab().expect("none");
        assert_eq!(columns.tab_count(), 3);
    }

    #[test]
    fn ending_a_session_closes_its_tabs_and_the_last_tab_shows_its_folder() {
        let (_root, _metadata, mut columns) = columns();
        let terminal = columns.columns()[0].rows[1].id.clone();
        columns.open_row(0, 1).expect("in the first tab");
        columns.open_tab().expect("second");
        columns.open_row(0, 1).expect("in the second tab");
        columns.open_tab().expect("third, on the folder");
        columns.close_tabs_of(&terminal);
        assert_eq!(columns.tab_count(), 1, "both tabs it filled closed");
        assert!(columns.opened().is_none(), "the one left is on the folder");
        // Alone in the last tab: the tab stays and shows the terminal's folder.
        columns.open_row(0, 1).expect("in the last tab");
        columns.close_tabs_of(&terminal);
        assert_eq!(columns.tab_count(), 1);
        assert!(columns.opened().is_none());
        assert_eq!(columns.columns().len(), 1);
    }

    #[test]
    fn a_terminal_under_the_name_new_terminal_gave_it_is_a_throwaway() {
        let (_root, _metadata, mut columns) = columns();
        columns.create_terminal(0, "en").expect("Terminal");
        columns.create_terminal(0, "en").expect("Terminal 2");
        let id_of = |columns: &Columns, title: &str| {
            columns.columns()[0]
                .rows
                .iter()
                .find(|entry| entry.title == title)
                .map(|entry| entry.id.clone())
                .expect(title)
        };
        let first = id_of(&columns, "Terminal");
        let second = id_of(&columns, "Terminal 2");
        let kept = id_of(&columns, "Claude Code");
        assert!(columns.is_throwaway_terminal(&first, "Terminal"));
        assert!(columns.is_throwaway_terminal(&second, "Terminal"));
        assert!(
            !columns.is_throwaway_terminal(&kept, "Terminal"),
            "named by a person"
        );
        columns.rename_id(0, &second, "Build").expect("renamed");
        assert!(
            !columns.is_throwaway_terminal(&second, "Terminal"),
            "renaming keeps it"
        );
        let readme = columns.columns()[0]
            .rows
            .iter()
            .find(|entry| entry.kind != EntryKind::Terminal)
            .map(|entry| entry.id.clone())
            .expect("a file");
        assert!(!columns.is_throwaway_terminal(&readme, "Terminal"));
    }

    #[test]
    fn a_tab_names_the_terminals_it_shows() {
        let (_root, _metadata, mut columns) = columns();
        let terminal = columns.columns()[0].rows[1].id.clone();
        assert!(columns.tab_terminal_ids(0).is_empty());
        columns.open_row(0, 1).expect("terminal");
        assert_eq!(columns.tab_terminal_ids(0), std::slice::from_ref(&terminal));
        assert!(columns.shows_terminal(&terminal));
    }

    #[test]
    fn a_changes_tab_opens_once_per_root_is_selected_again_and_closes_like_any_tab() {
        let (root, _metadata, mut columns) = columns();
        let (drive, other) = (root.path().to_path_buf(), root.path().join("Films"));
        columns.select(0, 2).expect("Films");
        columns.open_changes_tab(&drive).expect("opened");
        assert_eq!((columns.tab_count(), columns.active_tab()), (2, 1));
        assert_eq!(columns.active_changes(), Some(drive.as_path()));
        assert_eq!(
            (columns.tab_kind(1), columns.tab_glyph(1)),
            (6, crate::list::Glyph::Diff)
        );
        assert!(!columns.can_go_back() && !columns.can_go_forward());
        // The folder's tab is as it was left; choosing Changes again selects the tab.
        columns.select_tab(0);
        assert_eq!(columns.active_changes(), None);
        assert_eq!(columns.columns().len(), 2);
        columns.open_changes_tab(&drive).expect("selected");
        assert_eq!((columns.tab_count(), columns.active_tab()), (2, 1));
        // Another root has its own tab.
        columns.open_changes_tab(&other).expect("another");
        assert_eq!(columns.tab_count(), 3);
        assert!(columns.shows_changes(&drive) && columns.shows_changes(&other));
        // It closes with its cross, and Reopen Closed Tab brings it back.
        columns.close_tab(1);
        assert!(!columns.shows_changes(&drive));
        columns.reopen_closed_tab().expect("back");
        assert_eq!(columns.tab_changes(1), Some(drive.as_path()));
        // Going to a folder in it makes it that folder's tab.
        columns
            .go_to_path(&root.path().join("Films"))
            .expect("visited");
        assert_eq!(columns.active_changes(), None);
        assert_eq!(columns.tab_kind(1), 1);
    }

    #[test]
    fn only_the_last_twenty_closed_tabs_are_kept() {
        let (_root, _metadata, mut columns) = columns();
        for _ in 0..25 {
            columns.open_tab().expect("tab");
            columns.close_tab(1);
        }
        let mut reopened = 0;
        while columns.can_reopen_closed_tab() {
            columns.reopen_closed_tab().expect("back");
            reopened += 1;
        }
        assert_eq!(reopened, CLOSED_TABS_KEPT);
    }

    #[test]
    fn a_moved_tab_keeps_its_place_and_the_active_tab_stays_active() {
        let (_root, _metadata, mut columns) = columns();
        columns.select(0, 2).expect("Films");
        columns.open_tab().expect("second tab");
        columns.open_tab().expect("third tab");
        let drive = columns.tab_title(1);
        assert_eq!(
            (columns.tab_title(0), columns.active_tab()),
            ("Films".to_owned(), 2)
        );

        // The first tab carried to the end: the active third tab is now second.
        columns.move_tab(0, 2);
        assert_eq!(
            (0..3).map(|tab| columns.tab_title(tab)).collect::<Vec<_>>(),
            [drive.clone(), drive.clone(), "Films".to_owned()]
        );
        assert_eq!(columns.active_tab(), 1);
        assert_eq!(
            columns.columns().len(),
            1,
            "the active tab is still the new one"
        );

        // The active tab carried to the front goes on being active, its folder with it.
        columns.select_tab(2);
        columns.move_tab(2, 0);
        assert_eq!(
            (columns.tab_title(0), columns.active_tab()),
            ("Films".to_owned(), 0)
        );
        assert_eq!(columns.columns().len(), 2);

        // A tab carried past the active one from its right moves the active one right.
        columns.move_tab(2, 0);
        assert_eq!(columns.active_tab(), 1);
        let revision = columns.revision();
        columns.move_tab(1, 1);
        columns.move_tab(0, 3);
        assert_eq!(
            columns.revision(),
            revision,
            "a move to nowhere changes nothing"
        );
    }

    #[test]
    fn file_operations_show_in_the_columns_at_once() {
        let (root, _metadata, mut columns) = columns();
        columns.set_view(View::Board);
        // As a board, a new folder is listed and the folder shown stays.
        columns.create_folder(0, "en").expect("create");
        assert_eq!(columns.columns().len(), 1);
        assert_eq!(columns.columns()[0].selected, None);
        std::fs::remove_dir(root.path().join("New folder")).expect("remove");
        columns.set_view(View::Columns);
        columns.create_folder(0, "en").expect("create");
        assert_eq!(
            titles(&columns.columns()[0]),
            ["AGENTS", "Claude Code", "Films", "New folder"]
        );
        assert_eq!(columns.columns()[0].selected, Some(3));
        assert_eq!(columns.columns().len(), 2, "the new folder is open");
        // Another is numbered, as Finder numbers them.
        columns.create_folder(0, "en").expect("create");
        assert!(root.path().join("New folder 2").is_dir());

        columns.rename(0, 0, "Rules").expect("rename");
        assert!(root.path().join("Rules.md").exists());
        assert_eq!(
            titles(&columns.columns()[0]),
            [
                "Claude Code",
                "Films",
                "New folder",
                "New folder 2",
                "Rules"
            ]
        );
    }

    #[test]
    fn a_new_page_or_terminal_is_made_in_the_folder_and_selected() {
        let (root, _metadata, mut columns) = columns();
        let page = columns.create_page(0, "en").expect("page");
        assert!(root.path().join("Untitled.md").exists());
        assert!(page.is_some(), "its id, for its name to be typed over");
        // Selected in the columns, it shows beside them.
        assert_eq!(
            columns.opened().map(|opened| opened.kind),
            Some(EntryKind::Page)
        );

        // A terminal is named with its word, and numbered once that name is taken.
        columns.create_terminal(0, "en").expect("terminal");
        assert_eq!(
            fs::read_to_string(root.path().join("Terminal.nterm")).expect("terminal file"),
            "{\n  \"terminal\": 1\n}\n"
        );
        columns.create_terminal(0, "en").expect("terminal");
        let opened = columns.opened().expect("opened");
        assert_eq!(
            (opened.kind, opened.title.as_str()),
            (EntryKind::Terminal, "Terminal 2")
        );
    }

    #[test]
    fn a_tab_filled_by_a_terminal_is_known_as_one() {
        let (_root, _metadata, mut columns) = columns();
        columns.set_view(View::Board);
        let page = columns.create_page(0, "en").expect("page").expect("made");
        assert!(
            columns.opened().is_none(),
            "a new file is listed, not opened"
        );
        columns.open_in_folder(&page).expect("open");
        assert_eq!(
            columns.tab_terminal(columns.active),
            None,
            "a page is no terminal"
        );

        let terminal = columns.create_terminal(0, "en").expect("terminal");
        columns
            .open_in_folder(&terminal.expect("made"))
            .expect("open");
        assert_eq!(
            columns
                .tab_terminal(columns.active)
                .map(|opened| opened.title.as_str()),
            Some("Terminal")
        );
    }

    #[test]
    fn terminals_selected_together_open_side_by_side_in_one_tab() {
        let (_root, _metadata, mut columns) = columns();
        columns.create_terminal(0, "en").expect("terminal");
        let row = |columns: &Columns, title: &str| {
            columns.columns()[0]
                .rows
                .iter()
                .position(|entry| entry.title == title)
                .expect(title)
        };
        let id = |columns: &Columns, title: &str| {
            columns.columns()[0].rows[row(columns, title)].id.clone()
        };
        let group = |ids: &[&String]| Chosen {
            ids: ids.iter().map(|id| (*id).clone()).collect(),
            anchor: None,
            focus: None,
        };
        let (claude, terminal, agents) = (
            id(&columns, "Claude Code"),
            id(&columns, "Terminal"),
            id(&columns, "AGENTS"),
        );
        let mut both = vec![
            (claude.clone(), "Claude Code".to_owned()),
            (terminal.clone(), "Terminal".to_owned()),
        ];
        both.sort();
        let opened_together = |columns: &Columns| {
            let mut together = columns.opened().expect("opened").together.clone();
            together.sort();
            together
        };

        // As columns: two terminals chosen, one opened, both fill the tab.
        columns
            .choose(Table::Column(0), group(&[&claude, &terminal]))
            .expect("choose");
        columns
            .open_row(0, row(&columns, "Terminal"))
            .expect("open");
        assert_eq!(opened_together(&columns), both);
        assert!(columns.opened_fills_tab());
        let title = columns.opened().expect("opened").title.clone();
        assert!(title == "Claude Code, Terminal" || title == "Terminal, Claude Code");

        // A group with anything else in it opens only the row.
        columns.go_back().expect("back");
        columns
            .choose(Table::Column(0), group(&[&claude, &agents]))
            .expect("choose");
        columns
            .open_row(0, row(&columns, "Claude Code"))
            .expect("open");
        assert!(columns.opened().expect("opened").together.is_empty());

        // As a list, the list's group.
        columns.go_back().expect("back");
        columns.set_view(View::List);
        let rows = columns.list_rows(0);
        columns.show_list(&rows);
        columns
            .choose(Table::List, group(&[&claude, &terminal]))
            .expect("choose");
        columns.open_in_folder(&claude).expect("open");
        assert_eq!(opened_together(&columns), both);
    }

    #[test]
    fn a_file_shown_from_anywhere_takes_its_own_tab_and_keeps_it() {
        let (root, _metadata, mut columns) = columns();
        fs::write(
            root.path().join("Films").join("Shots").join("Codex.nterm"),
            crate::terminal::launch::TERMINAL_FILE,
        )
        .expect("terminal");
        columns.refresh().expect("refresh");
        let shots = fs::canonicalize(root.path().join("Films").join("Shots")).expect("path");
        let folder = columns.workspace().location_at(&shots).expect("folder");
        let id = columns
            .workspace()
            .all_contents(&folder)
            .expect("contents")
            .into_iter()
            .find(|entry| entry.title == "Codex")
            .expect("listed")
            .id;
        let placed = columns.workspace().place_of(&id).expect("placed");
        assert_eq!(placed.title, "Codex");
        assert_eq!(placed.folder, folder);
        assert_eq!(
            placed
                .folders
                .iter()
                .skip(1)
                .map(|crumb| crumb.title.as_str())
                .collect::<Vec<_>>(),
            ["Films", "Shots"]
        );

        // From the drive's own folder: a second tab opens on Shots with the terminal filling it.
        columns.show_in_tab(&id).expect("shown");
        assert_eq!((columns.tab_count(), columns.active_tab()), (2, 1));
        assert_eq!(
            columns.tab_terminal(1).map(|opened| opened.id.clone()),
            Some(id.clone())
        );
        assert_eq!(columns.tab_terminal(0), None, "the first tab stays");
        // Shown again from the first tab, its own tab is chosen; no third opens.
        columns.select_tab(0);
        columns.show_in_tab(&id).expect("shown again");
        assert_eq!((columns.tab_count(), columns.active_tab()), (2, 1));
        // The same as columns: the file fills its tab there too.
        columns.close_tab(1);
        columns.set_view(View::Columns);
        columns.show_in_tab(&id).expect("shown as columns");
        assert_eq!(
            columns
                .tab_terminal(columns.active_tab())
                .map(|opened| opened.id.clone()),
            Some(id)
        );
    }

    #[test]
    fn a_drop_of_files_already_in_the_folder_leaves_them_as_they_are() {
        let (root, _metadata, mut columns) = columns();
        // The drive's own files dragged out and let go over their own folder: nothing happens.
        columns
            .import(
                0,
                &[root.path().join("AGENTS.md"), root.path().join("Films")],
            )
            .expect("dropped back");
        assert_eq!(
            titles(&columns.columns()[0]),
            ["AGENTS", "Claude Code", "Films"]
        );
        // With a file from another folder of the drive, only that one is copied in.
        columns
            .import(
                0,
                &[
                    root.path().join("AGENTS.md"),
                    root.path().join("Films").join("cut.mp4"),
                ],
            )
            .expect("dropped");
        assert_eq!(
            titles(&columns.columns()[0]),
            ["AGENTS", "Claude Code", "Films", "cut.mp4"]
        );
        assert!(root.path().join("Films").join("cut.mp4").exists());
        assert!(!root.path().join("AGENTS 2.md").exists());
    }

    #[test]
    fn files_are_imported_moved_and_trashed_by_id() {
        let (root, _metadata, mut columns) = columns();
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("cut.mp4"), b"new").expect("source");
        fs::write(outside.path().join("AGENTS.md"), b"other").expect("source");
        columns
            .import(
                0,
                &[
                    outside.path().join("cut.mp4"),
                    outside.path().join("AGENTS.md"),
                ],
            )
            .expect("import");
        assert_eq!(
            fs::read(root.path().join("AGENTS 2.md")).expect("numbered"),
            b"other"
        );
        // Listed by file name: "AGENTS 2.md" sorts before "AGENTS.md".
        assert_eq!(
            titles(&columns.columns()[0]),
            ["AGENTS 2", "AGENTS", "Claude Code", "Films", "cut.mp4"]
        );

        let id = |columns: &Columns, title: &str| -> String {
            columns.columns()[0]
                .rows
                .iter()
                .find(|entry| entry.title == title)
                .expect(title)
                .id
                .clone()
        };
        let (page, clip, films) = (
            id(&columns, "AGENTS"),
            id(&columns, "cut.mp4"),
            id(&columns, "Films"),
        );
        // A folder dropped on itself stays; the page goes in.
        columns
            .move_ids(0, &[page, films.clone()], Some(&films))
            .expect("move");
        assert!(root.path().join("Films").join("AGENTS.md").exists());
        assert!(root.path().join("Films").is_dir());

        // Nothing can be trashed until the platform says how.
        assert!(columns.trash(&[clip]).is_err());
    }

    /// The id of the file named `title` in the drive's own folder, as its column lists it.
    fn root_id(columns: &Columns, title: &str) -> String {
        columns.columns()[0]
            .rows
            .iter()
            .find(|entry| entry.title == title)
            .expect(title)
            .id
            .clone()
    }

    #[test]
    fn each_drive_change_made_here_is_undone_and_redone() {
        let (root, _metadata, mut columns) = columns();
        let bin = tempfile::tempdir_in(root.path().parent().expect("tmp")).expect("bin");
        columns
            .workspace()
            .set_trash(crate::workspace::trash_into(bin.path().to_path_buf()));
        let at = |name: &str| root.path().join(name);
        let page = root_id(&columns, "AGENTS");
        let films = root_id(&columns, "Films");
        assert!(columns.undo().is_none(), "nothing made yet");

        // A rename.
        columns.rename_id(0, &page, "Rules").expect("rename");
        assert!(at("Rules.md").exists() && !at("AGENTS.md").exists());
        columns.undo().expect("a change").expect("undo");
        assert!(at("AGENTS.md").exists() && !at("Rules.md").exists());
        columns.redo().expect("a change").expect("redo");
        assert!(at("Rules.md").exists());
        columns.undo().expect("a change").expect("undo again");
        assert_eq!(root_id(&columns, "AGENTS"), page, "the file keeps its id");

        // A move by a drag.
        columns
            .move_ids(0, std::slice::from_ref(&page), Some(&films))
            .expect("move");
        assert!(at("Films/AGENTS.md").exists());
        columns.undo().expect("a change").expect("undo");
        assert!(at("AGENTS.md").exists() && !at("Films/AGENTS.md").exists());
        columns.redo().expect("a change").expect("redo");
        assert!(at("Films/AGENTS.md").exists());
        columns.undo().expect("a change").expect("undo again");

        // Files copied in: undone, the copy goes to the trash; redone, it comes back.
        let outside = tempfile::tempdir().expect("outside");
        fs::create_dir(outside.path().join("Album")).expect("album");
        fs::write(outside.path().join("Album/shot.png"), b"png").expect("shot");
        columns
            .import(0, &[outside.path().join("Album")])
            .expect("import");
        assert!(at("Album/shot.png").exists());
        columns.undo().expect("a change").expect("undo");
        assert!(!at("Album").exists() && bin.path().join("Album/shot.png").exists());
        columns.redo().expect("a change").expect("redo");
        assert_eq!(fs::read(at("Album/shot.png")).expect("back"), b"png");
        assert!(!bin.path().join("Album").exists());

        // A duplicate, beside its file.
        columns
            .duplicate(std::slice::from_ref(&page))
            .expect("duplicate");
        assert!(at("AGENTS 2.md").exists());
        columns.undo().expect("a change").expect("undo");
        assert!(!at("AGENTS 2.md").exists());

        // A move to the trash comes back with its id.
        columns.trash(std::slice::from_ref(&page)).expect("trash");
        assert!(!at("AGENTS.md").exists() && bin.path().join("AGENTS.md").exists());
        columns.undo().expect("a change").expect("undo");
        assert!(at("AGENTS.md").exists() && !bin.path().join("AGENTS.md").exists());
        let kept = columns.workspace().root().join("AGENTS.md");
        assert_eq!(columns.workspace().path_of(&page).expect("kept"), kept);
        columns.redo().expect("a change").expect("redo");
        assert!(!at("AGENTS.md").exists());
        columns.undo().expect("a change").expect("undo again");

        // What is left, the copy in, is undone too: the drive is as it started.
        while let Some(undone) = columns.undo() {
            undone.expect("undo");
        }
        assert_eq!(
            titles(&columns.columns()[0]),
            ["AGENTS", "Claude Code", "Films"]
        );
    }

    #[test]
    fn a_change_whose_file_moved_since_refuses_and_is_let_go() {
        let (root, _metadata, mut columns) = columns();
        let page = root_id(&columns, "AGENTS");
        columns.rename_id(0, &page, "Rules").expect("rename");
        // An agent, or another app, moves the file on.
        fs::rename(root.path().join("Rules.md"), root.path().join("Moved.md")).expect("move");
        let refused = columns.undo().expect("a change").expect_err("refused");
        assert_eq!(
            refused.to_string(),
            "“Rules.md” has moved or gone since, so this can’t be undone"
        );
        assert!(columns.undo().is_none(), "the change is let go");
        assert!(root.path().join("Moved.md").exists());

        // A redo whose place has been taken since.
        let clip = root_id(&columns, "Claude Code");
        columns.rename_id(0, &clip, "Codex").expect("rename");
        columns.undo().expect("a change").expect("undo");
        fs::write(root.path().join("Codex.nterm"), b"{}").expect("taken");
        let refused = columns.redo().expect("a change").expect_err("refused");
        assert_eq!(
            refused.to_string(),
            "“Codex.nterm” has moved or gone since, so this can’t be redone"
        );
        assert!(columns.redo().is_none());
    }

    #[test]
    fn a_compressed_archive_lands_beside_its_files_and_is_undone_to_the_trash() {
        let (root, _metadata, mut columns) = columns();
        let bin = tempfile::tempdir_in(root.path().parent().expect("tmp")).expect("bin");
        columns
            .workspace()
            .set_trash(crate::workspace::trash_into(bin.path().to_path_buf()));
        let films = root_id(&columns, "Films");
        let page = root_id(&columns, "AGENTS");
        let drive = columns.workspace().root().to_path_buf();
        let plan = columns
            .plan_compress(std::slice::from_ref(&films), "en")
            .expect("allowed")
            .expect("a plan");
        assert_eq!(
            (plan.directory.as_path(), plan.name.as_str()),
            (drive.as_path(), "Films.zip")
        );
        let several = columns
            .plan_compress(&[page, films], "en")
            .expect("allowed")
            .expect("a plan");
        assert_eq!(several.name, "Archive.zip");
        assert!(columns
            .plan_compress(&["gone".to_owned()], "en")
            .expect("nothing to refuse")
            .is_none());

        let archive = plan.run().expect("compressed");
        assert_eq!(archive, drive.join("Films.zip"));
        columns.record_made(archive.clone()).expect("kept");
        assert!(titles(&columns.columns()[0]).contains(&"Films.zip"));
        columns.undo().expect("a change").expect("undo");
        assert!(!archive.exists() && bin.path().join("Films.zip").exists());
        columns.redo().expect("a change").expect("redo");
        assert!(archive.exists());

        // An archive written while another drive opened is not kept to undo here.
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        columns
            .record_made(elsewhere.path().join("Films.zip"))
            .expect("listed");
        columns
            .undo()
            .expect("a change")
            .expect("the redone archive");
        assert!(!archive.exists());
    }

    #[test]
    fn a_trash_the_platform_cannot_place_is_not_kept_to_undo() {
        let (root, _metadata, mut columns) = columns();
        let bin = tempfile::tempdir().expect("bin");
        let into = bin.path().to_path_buf();
        // A trash that moves files but cannot say where.
        columns.workspace().set_trash(Box::new(move |path| {
            fs::rename(path, into.join(path.file_name().expect("a name")))?;
            Ok(None)
        }));
        let page = root_id(&columns, "AGENTS");
        columns.trash(std::slice::from_ref(&page)).expect("trash");
        assert!(bin.path().join("AGENTS.md").exists());
        assert!(columns.undo().is_none(), "nothing to undo");

        // A copy is undone into that trash, and then cannot come back.
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("note.txt"), b"x").expect("note");
        columns
            .import(0, &[outside.path().join("note.txt")])
            .expect("import");
        columns.undo().expect("a change").expect("undo");
        assert!(!root.path().join("note.txt").exists());
        assert!(columns.redo().is_none(), "nothing to redo");
    }

    #[test]
    fn a_paste_lands_in_the_folder_of_the_table_it_was_made_in() {
        let (root, _metadata, mut columns) = columns();
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("note.txt"), b"x").expect("note");
        let films = root_id(&columns, "Films");
        let row = columns.columns()[0]
            .rows
            .iter()
            .position(|entry| entry.id == films)
            .expect("films");
        columns.select(0, row).expect("open Films");
        columns
            .paste(Table::Column(1), &[outside.path().join("note.txt")])
            .expect("paste");
        assert!(root.path().join("Films/note.txt").exists());
        columns
            .paste(Table::Column(0), &[outside.path().join("note.txt")])
            .expect("paste");
        assert!(root.path().join("note.txt").exists());
        assert_eq!(
            columns.selected_ids(Table::Column(0)),
            [films],
            "a column's selection is its open row"
        );
    }

    #[test]
    fn the_board_follows_the_sort_the_list_has() {
        let (_root, _metadata, mut columns) = columns();
        let names = |columns: &Columns| -> Vec<String> {
            columns
                .board_rows()
                .into_iter()
                .map(|entry| entry.title)
                .collect()
        };
        assert_eq!(names(&columns), ["AGENTS", "Claude Code", "Films"]);
        columns.sort_by(crate::list::Column::Kind);
        assert_eq!(
            columns.list_sort().map(|sort| (sort.key, sort.ascending)),
            Some((crate::list::Column::Kind, true))
        );
        let listed: Vec<String> = columns
            .list_rows(0)
            .into_iter()
            .map(|row| row.entry.title)
            .collect();
        assert_eq!(names(&columns), listed, "the board in the list's order");
        assert_ne!(
            listed,
            ["AGENTS", "Claude Code", "Films"],
            "kind is not name"
        );
        columns.sort_by(crate::list::Column::Modified);
        assert_eq!(
            columns.list_sort().map(|sort| (sort.key, sort.ascending)),
            Some((crate::list::Column::Modified, false)),
            "dates newest first, as Finder"
        );
        columns.sort_by(crate::list::Column::Name);
        assert_eq!(names(&columns), ["AGENTS", "Claude Code", "Films"]);
    }

    #[test]
    fn the_board_zoom_is_kept_for_next_time() {
        let (root, metadata, mut columns) = columns();
        assert_eq!(columns.board_columns(), crate::board::DEFAULT_COLUMN_COUNT);
        columns.set_board_columns(4);
        columns.set_board_columns(40);
        assert_eq!(columns.board_columns(), 8);
        columns.set_board_columns(3);
        drop(columns);
        let reopened = Columns::open(Workspace::open(root.path(), metadata.path()).expect("open"))
            .expect("columns");
        assert_eq!(reopened.board_columns(), 3);
    }

    #[test]
    fn the_drives_own_folder_stays_where_it_is() {
        use crate::workspace::{path_protection, Protection};
        use std::path::Path;
        let (root, home) = (Path::new("/Users/sam/Work"), Path::new("/Users/sam"));
        assert_eq!(
            path_protection(root, root, home, true),
            Some(Protection::Location)
        );
        assert_eq!(path_protection(&root.join("Films"), root, home, true), None);
        // A drive that is the whole disk protects the system's folders and the way home.
        let disk = Path::new("/");
        assert_eq!(
            path_protection(Path::new("/System/Library"), disk, home, true),
            Some(Protection::System)
        );
        assert_eq!(
            path_protection(Path::new("/usr/local/bin"), disk, home, true),
            None
        );
        assert_eq!(
            path_protection(Path::new("/usr/local"), disk, home, true),
            Some(Protection::Location)
        );
        assert_eq!(
            path_protection(Path::new("/Users"), disk, home, true),
            Some(Protection::Location)
        );
        assert_eq!(
            path_protection(Path::new("/System/Volumes/Data/Users"), disk, home, true),
            Some(Protection::Location)
        );
        assert_eq!(
            path_protection(home, disk, home, true),
            Some(Protection::Location)
        );
        assert_eq!(
            path_protection(&home.join("Desktop"), disk, home, true),
            None
        );
    }

    fn id_in(columns: &Columns, column: usize, title: &str) -> String {
        columns.columns()[column]
            .rows
            .iter()
            .find(|entry| entry.title == title)
            .expect(title)
            .id
            .clone()
    }

    #[test]
    fn a_rename_is_trimmed_and_leaves_an_unchanged_or_blank_name_alone() {
        assert_eq!(renamed_title("cut", "  final cut \n"), Some("final cut"));
        assert_eq!(renamed_title("cut", " cut "), None);
        assert_eq!(renamed_title("cut", " \t "), None);
        let (root, _metadata, mut columns) = columns();
        let page = id_in(&columns, 0, "AGENTS");
        columns.rename(0, 0, "  AGENTS ").expect("same name");
        columns.rename_id(0, &page, "   ").expect("blank");
        assert!(root.path().join("AGENTS.md").exists());
        columns.rename(0, 0, " Rules ").expect("rename");
        assert!(root.path().join("Rules.md").exists());
    }

    #[test]
    fn a_drag_in_the_columns_lands_in_a_folder_a_running_terminal_or_another_column() {
        let (root, _metadata, mut columns) = columns();
        columns.select(0, 2).expect("Films");
        let (films, page) = (id_in(&columns, 0, "Films"), id_in(&columns, 0, "AGENTS"));
        let clip = vec![id_in(&columns, 1, "cut.mp4")];
        let never = |_: &Entry| false;
        let running = |_: &Entry| true;
        assert_eq!(
            columns.column_drop_place(&clip, 0, Some(2), never),
            ColumnDrop::Folder(2)
        );
        assert_eq!(
            columns.column_drop_place(&clip, 0, None, never),
            ColumnDrop::Column
        );
        assert_eq!(
            columns.column_drop_place(&clip, 1, None, never),
            ColumnDrop::Nowhere,
            "its own column"
        );
        assert_eq!(
            columns.column_drop_place(&clip, 0, Some(1), running),
            ColumnDrop::Terminal(1)
        );
        assert_eq!(
            columns.column_drop_place(&clip, 0, Some(1), never),
            ColumnDrop::Column,
            "a terminal not running is the column's"
        );
        assert_eq!(
            columns.column_drop_place(std::slice::from_ref(&films), 1, Some(0), never),
            ColumnDrop::Nowhere,
            "never into itself"
        );
        assert_eq!(
            columns.column_drop_place(std::slice::from_ref(&films), 0, Some(2), never),
            ColumnDrop::Nowhere,
            "nor onto itself"
        );
        assert_eq!(
            columns.column_drop_place(std::slice::from_ref(&page), 1, Some(0), never),
            ColumnDrop::Folder(0)
        );
        assert_eq!(
            columns.column_drop_place(&["gone".to_owned()], 0, Some(2), never),
            ColumnDrop::Nowhere
        );

        columns
            .column_drop(&clip, 0, Some(1), running)
            .expect("to the terminal");
        assert!(
            root.path().join("Films").join("cut.mp4").exists(),
            "a terminal's row moves nothing"
        );
        columns
            .column_drop(&clip, 0, None, never)
            .expect("to the drive's folder");
        assert!(root.path().join("cut.mp4").exists());
        assert_eq!(
            titles(&columns.columns()[0]),
            ["AGENTS", "Claude Code", "Films", "cut.mp4"]
        );
        assert_eq!(titles(&columns.columns()[1]), ["Shots"], "Films stays open");
        columns
            .column_drop(&[page], 1, Some(0), never)
            .expect("into Shots");
        assert!(root
            .path()
            .join("Films")
            .join("Shots")
            .join("AGENTS.md")
            .exists());
    }

    #[test]
    fn left_and_right_open_close_and_walk_the_folders_of_the_list() {
        let (_root, _metadata, mut columns) = columns();
        let films = columns.list_rows(0)[2].entry.id.clone();
        assert_eq!(
            list_expand_step(&columns.list_rows(0), 2),
            ListStep::Toggle(films.clone())
        );
        assert_eq!(
            list_expand_step(&columns.list_rows(0), 0),
            ListStep::Stay,
            "a page opens nothing"
        );
        assert_eq!(list_collapse_step(&columns.list_rows(0), 0), ListStep::Stay);
        columns.toggle_expanded(&films);
        let rows = columns.list_rows(0);
        assert_eq!(
            list_expand_step(&rows, 2),
            ListStep::Focus(3),
            "an open folder: its first row"
        );
        assert_eq!(
            list_collapse_step(&rows, 4),
            ListStep::Focus(2),
            "a row inside: its folder's row"
        );
        assert_eq!(list_collapse_step(&rows, 2), ListStep::Toggle(films));
        let shots = rows[3].entry.id.clone();
        columns.toggle_expanded(&shots);
        let rows = columns.list_rows(0);
        assert_eq!(
            list_expand_step(&rows, 3),
            ListStep::Stay,
            "an empty open folder has no first row"
        );
        assert_eq!(
            list_expand_step(&rows, 4),
            ListStep::Stay,
            "its placeholder is no folder"
        );
        assert_eq!(list_collapse_step(&rows, 4), ListStep::Focus(3));
        assert_eq!(list_expand_step(&rows, 99), ListStep::Stay);
    }

    #[test]
    fn a_tab_is_the_same_tab_wherever_it_sits_and_whatever_it_shows() {
        let (_root, _metadata, mut columns) = columns();
        let first = columns.active_tab_id();
        columns.select(0, 2).expect("Films");
        assert_eq!(
            columns.active_tab_id(),
            first,
            "another folder, the same tab"
        );
        columns.open_tab().expect("second tab");
        let second = columns.active_tab_id();
        columns.open_tab().expect("third tab");
        let third = columns.active_tab_id();
        assert!(first != second && second != third && first != third);

        // A tab before the active one closes: the active tab moves along the row, and is
        // still the third one opened.
        columns.close_tab(0);
        assert_eq!((columns.active_tab(), columns.active_tab_id()), (1, third));
        // The active tab closes: its neighbour is active.
        columns.close_tab(1);
        assert_eq!((columns.active_tab(), columns.active_tab_id()), (0, second));
        // A tab opened later is never given a closed tab's id.
        columns.open_tab().expect("fourth tab");
        let fourth = columns.active_tab_id();
        assert!(fourth != first && fourth != third);
        columns.select_tab(0);
        assert_eq!(columns.active_tab_id(), second);
    }

    #[test]
    fn every_tab_but_the_last_one_open_can_close() {
        use crate::list::Glyph;
        let (_root, _metadata, mut columns) = columns();
        assert!(!columns.can_close_tab(0));
        assert_eq!(columns.tab_glyph(0), Glyph::House);
        columns.select(0, 2).expect("Films");
        columns.open_tab().expect("new tab");
        assert!(columns.can_close_tab(0) && columns.can_close_tab(1) && !columns.can_close_tab(2));
        // The active tab (the new one, at the drive's top) shows the same glyph as any other.
        assert_eq!(
            (columns.tab_glyph(0), columns.tab_glyph(1)),
            (Glyph::Folder, Glyph::House)
        );
    }

    #[test]
    fn a_new_drive_opens_as_columns_and_then_in_the_view_last_picked() {
        let root = tempfile::tempdir().expect("root");
        let metadata = tempfile::tempdir().expect("metadata");
        let open = || {
            Columns::open(Workspace::open(root.path(), metadata.path()).expect("open"))
                .expect("columns")
        };
        let mut first = open();
        assert_eq!(
            first.view(),
            View::Columns,
            "a person's first look is columns"
        );
        first.set_view(View::Board);
        drop(first);
        assert_eq!(
            open().view(),
            View::Board,
            "the view picked last opens next time"
        );
    }

    #[test]
    fn a_drive_opens_with_one_listing_in_the_view_it_kept() {
        use crate::list::Column;
        use crate::workspace::Indexed;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let root = tempfile::tempdir().expect("root");
        let metadata = tempfile::tempdir().expect("metadata");
        let names = ["a.txt", "b.png", "c.csv"];
        for name in names {
            fs::write(root.path().join(name), b"x").expect("file");
        }
        // Kept as a list sorted by name, newest last, showing a column the file index fills.
        let kept = ViewChoices {
            view: View::List,
            list_columns: vec![Column::Kind.number(), Column::Dimensions.number()],
            list_sort: Some((Column::Name.number(), false)),
            ..ViewChoices::default()
        };
        fs::write(
            metadata.path().join(VIEW_CHOICES_FILE),
            serde_json::to_string(&kept).expect("choices"),
        )
        .expect("kept");
        let mut workspace = Workspace::open(root.path(), metadata.path()).expect("open");
        let asked = [(); 2].map(|()| Arc::new(AtomicUsize::new(0)));
        let [kinds, index] = asked.clone();
        workspace.set_kind_describer(Box::new(move |_| {
            kinds.fetch_add(1, Ordering::SeqCst);
            Some("Document".to_owned())
        }));
        workspace.set_file_index(Box::new(move |_| {
            index.fetch_add(1, Ordering::SeqCst);
            Some(Indexed::default())
        }));

        let mut columns = Columns::open(workspace).expect("columns");
        let rows = columns.list_rows(0);

        let titles: Vec<&str> = rows.iter().map(|row| row.entry.title.as_str()).collect();
        assert_eq!(
            titles,
            ["c.csv", "b.png", "a.txt"],
            "listed in the order kept"
        );
        for (what, count) in ["kinds", "index"].iter().zip(&asked) {
            assert_eq!(
                count.load(Ordering::SeqCst),
                names.len(),
                "each file's {what} read once"
            );
        }
    }

    #[test]
    fn refreshing_keeps_the_selection_and_closes_what_has_gone() {
        let (root, _metadata, mut columns) = columns();
        columns.select(0, 2).expect("select folder");
        fs::write(root.path().join("A new file.txt"), b"x").expect("new file");
        columns.refresh().expect("refresh");
        assert_eq!(
            titles(&columns.columns()[0]),
            ["A new file.txt", "AGENTS", "Claude Code", "Films"]
        );
        assert_eq!(columns.columns()[0].selected, Some(3));
        assert_eq!(columns.columns().len(), 2);

        fs::remove_dir_all(root.path().join("Films")).expect("remove");
        columns.refresh().expect("refresh");
        assert_eq!(columns.columns().len(), 1);
        assert_eq!(columns.columns()[0].selected, None);
    }

    fn chosen_titles(columns: &Columns, table: Table) -> Vec<String> {
        let rows = columns.table_selection(table).rows;
        match table {
            Table::List => rows
                .iter()
                .map(|&row| columns.list_shown[row].key.clone())
                .collect(),
            Table::Column(column) => rows
                .iter()
                .map(|&row| columns.columns()[column].rows[row].title.clone())
                .collect(),
        }
    }

    #[test]
    fn one_row_chosen_in_a_column_opens_and_a_group_closes_what_follows() {
        let (_root, _metadata, mut columns) = columns();
        columns
            .press_row(Table::Column(0), Some(2), Mode::Replace)
            .expect("Films");
        assert_eq!(
            (columns.columns().len(), columns.columns()[0].selected),
            (2, Some(2))
        );
        columns
            .press_row(Table::Column(0), Some(0), Mode::Toggle)
            .expect("AGENTS too");
        assert_eq!(
            (columns.columns().len(), columns.columns()[0].selected),
            (1, None)
        );
        assert_eq!(
            chosen_titles(&columns, Table::Column(0)),
            ["AGENTS", "Films"]
        );
        // Taking a row out of a group of two leaves one row chosen: it opens.
        columns
            .press_row(Table::Column(0), Some(0), Mode::Toggle)
            .expect("AGENTS out");
        assert_eq!(
            (columns.columns().len(), columns.columns()[0].selected),
            (2, Some(2))
        );
        // Pressing the row already shown changes nothing; the space under the rows settles.
        let revision = columns.revision();
        columns
            .press_row(Table::Column(0), Some(2), Mode::Replace)
            .expect("again");
        assert_eq!(columns.revision(), revision);
        columns
            .press_row(Table::Column(0), None, Mode::Replace)
            .expect("empty");
        assert_eq!(
            (columns.columns().len(), columns.columns()[0].selected),
            (1, None)
        );
        assert!(columns.table_selection(Table::Column(0)).rows.is_empty());
    }

    #[test]
    fn a_columns_group_is_kept_by_its_files_through_a_re_listing() {
        let (root, _metadata, mut columns) = columns();
        columns
            .press_row(Table::Column(0), Some(1), Mode::Replace)
            .expect("terminal");
        columns
            .press_row(Table::Column(0), Some(2), Mode::Range)
            .expect("to Films");
        assert_eq!(
            chosen_titles(&columns, Table::Column(0)),
            ["Claude Code", "Films"]
        );
        fs::write(root.path().join("A new file.txt"), b"x").expect("new file");
        columns.refresh().expect("refresh");
        assert_eq!(
            chosen_titles(&columns, Table::Column(0)),
            ["Claude Code", "Films"]
        );
        // A folder chosen elsewhere starts its column with nothing chosen.
        columns.select(0, 3).expect("Films");
        assert!(columns.table_selection(Table::Column(1)).rows.is_empty());
    }

    #[test]
    fn a_pick_up_selects_without_opening_and_a_column_with_a_folder_open_keeps_it() {
        let (_root, _metadata, mut columns) = columns();
        columns.select(0, 2).expect("Films");
        // Films is open in the next column: another row goes alone and Films stays shown.
        assert_eq!(columns.pick_up_rows(Table::Column(0), 0), [0]);
        assert_eq!(chosen_titles(&columns, Table::Column(0)), ["Films"]);
        assert_eq!(columns.columns().len(), 2);
        // A file shown beside its column: another row picked up shows alone until the next
        // change, when the open file shows again.
        columns.select(0, 0).expect("AGENTS");
        assert_eq!(columns.pick_up_rows(Table::Column(0), 1), [1]);
        assert_eq!(chosen_titles(&columns, Table::Column(0)), ["Claude Code"]);
        assert_eq!(
            columns.opened().map(|opened| opened.title.as_str()),
            Some("AGENTS")
        );
        columns.refresh().expect("refresh");
        assert_eq!(chosen_titles(&columns, Table::Column(0)), ["AGENTS"]);
        // A selected row carries the whole group.
        columns
            .press_row(Table::Column(0), Some(1), Mode::Toggle)
            .expect("group");
        assert_eq!(columns.pick_up_rows(Table::Column(0), 1), [0, 1]);
    }

    #[test]
    fn one_file_the_system_keeps_in_place_stops_the_drag() {
        let (_root, _metadata, mut columns) = columns();
        columns
            .press_row(Table::Column(0), Some(0), Mode::Replace)
            .expect("AGENTS");
        columns
            .press_row(Table::Column(0), Some(1), Mode::Toggle)
            .expect("terminal");
        columns.tab_mut().columns[0].rows[1].protection =
            Some(crate::workspace::Protection::System);
        assert!(
            columns.pick_up_rows(Table::Column(0), 0).is_empty(),
            "the group holds a kept file"
        );
        assert!(columns.pick_up_rows(Table::Column(0), 1).is_empty());
        assert_eq!(
            columns.pick_up_rows(Table::Column(0), 2),
            [2],
            "another row goes alone"
        );
        assert_eq!(columns.rename_row(Table::Column(0)), Some(2));
        columns
            .press_row(Table::Column(0), Some(1), Mode::Replace)
            .expect("the kept file");
        assert_eq!(
            columns.rename_row(Table::Column(0)),
            None,
            "a kept file is not renamed"
        );
    }

    #[test]
    fn left_and_right_step_between_columns() {
        let (_root, _metadata, mut columns) = columns();
        assert_eq!(
            columns.step_column(0, false).expect("right"),
            None,
            "nothing open"
        );
        columns.select(0, 2).expect("Films");
        assert_eq!(columns.step_column(0, false).expect("right"), Some(1));
        assert_eq!(
            columns.columns()[1].selected,
            Some(0),
            "on the open folder's first row"
        );
        assert_eq!(columns.columns().len(), 3, "Shots opens");
        assert_eq!(columns.step_column(1, true).expect("left"), Some(0));
        assert_eq!(
            (columns.columns().len(), columns.columns()[0].selected),
            (2, Some(2))
        );
        assert_eq!(
            columns.columns()[1].selected,
            None,
            "the column left has nothing chosen"
        );
        assert_eq!(
            columns.step_column(0, true).expect("left"),
            None,
            "no column before the first"
        );
        // A group in the column: → goes nowhere.
        columns
            .press_row(Table::Column(0), Some(1), Mode::Toggle)
            .expect("group");
        assert_eq!(columns.step_column(0, false).expect("right"), None);
    }

    #[test]
    fn keys_move_the_selection_and_escape_settles_a_column() {
        use crate::list::row_keys::Direction;
        let (_root, _metadata, mut columns) = columns();
        assert_eq!(
            columns
                .step_rows(Table::Column(0), Direction::Next, false)
                .expect("down"),
            Some(0)
        );
        assert_eq!(
            columns.opened().map(|opened| opened.title.as_str()),
            Some("AGENTS")
        );
        assert_eq!(
            columns
                .step_rows(Table::Column(0), Direction::Next, true)
                .expect("shift down"),
            Some(1)
        );
        assert_eq!(
            chosen_titles(&columns, Table::Column(0)),
            ["AGENTS", "Claude Code"]
        );
        assert_eq!(columns.opened(), None);
        columns.select_all_rows(Table::Column(0)).expect("all");
        assert_eq!(columns.table_selection(Table::Column(0)).rows, [0, 1, 2]);
        columns.clear_rows(Table::Column(0)).expect("escape");
        assert!(columns.table_selection(Table::Column(0)).rows.is_empty());
        // Typing a name selects the row it starts, which opens it.
        let now = std::time::Instant::now();
        assert_eq!(
            columns
                .type_ahead(Table::Column(0), 'f', now)
                .expect("typed"),
            Some(2)
        );
        assert_eq!(columns.columns().len(), 2);
        assert_eq!(chosen_titles(&columns, Table::Column(0)), ["Films"]);
    }

    #[test]
    fn typing_a_name_selects_the_first_row_it_starts_and_passes_over_a_placeholder() {
        use std::time::{Duration, Instant};
        let (_root, _metadata, mut columns) = columns();
        let films = columns.columns()[0].rows[2].id.clone();
        let inside = columns
            .workspace()
            .all_contents(&Location {
                folder_id: Some(films.clone()),
            })
            .expect("films");
        let shots = inside
            .iter()
            .find(|entry| entry.title == "Shots")
            .expect("Shots")
            .id
            .clone();
        columns.toggle_expanded(&films);
        columns.toggle_expanded(&shots);
        assert_eq!(
            listed(&mut columns),
            [
                "AGENTS",
                "Claude Code",
                "Films",
                "Shots",
                "Empty folder",
                "cut.mp4"
            ]
        );
        let start = Instant::now();
        assert_eq!(
            columns.type_ahead(Table::List, 'C', start).expect("c"),
            Some(1)
        );
        // Letters typed together make one name, letters' case aside.
        assert_eq!(
            columns
                .type_ahead(Table::List, 'U', start + Duration::from_millis(300))
                .expect("cu"),
            Some(5)
        );
        assert_eq!(
            chosen_titles(&columns, Table::List),
            [inside
                .iter()
                .find(|entry| entry.title == "cut.mp4")
                .expect("cut")
                .id
                .clone()]
        );
        // The placeholder's words are no name; no name keeps the selection as it was.
        assert_eq!(
            columns
                .type_ahead(Table::List, 'e', start + Duration::from_secs(2))
                .expect("e"),
            None
        );
        assert_eq!(columns.table_selection(Table::List).rows, [5]);
        // A second later the letters start a name of their own.
        assert_eq!(
            columns
                .type_ahead(Table::List, 's', start + Duration::from_secs(4))
                .expect("s"),
            Some(3)
        );
    }

    #[test]
    fn a_right_click_selects_the_row_alone_or_settles_the_column() {
        let (_root, _metadata, mut columns) = columns();
        // A right click selects the row but opens nothing.
        assert_eq!(
            columns
                .context_rows(Table::Column(0), Some(1))
                .expect("terminal"),
            [1]
        );
        assert!(columns.opened().is_none());
        assert_eq!(columns.table_selection(Table::Column(0)).rows, [1]);
        columns
            .press_row(Table::Column(0), Some(2), Mode::Toggle)
            .expect("group");
        assert_eq!(
            columns
                .context_rows(Table::Column(0), Some(2))
                .expect("in the group"),
            [1, 2]
        );
        assert_eq!(
            columns.context_rows(Table::Column(0), None).expect("empty"),
            Vec::<usize>::new()
        );
        assert!(columns.table_selection(Table::Column(0)).rows.is_empty());
    }

    #[test]
    fn a_folder_opens_in_a_new_tab_and_the_tab_it_was_in_stays() {
        let (_root, _metadata, mut columns) = columns();
        let films = columns.columns()[0].rows[2].clone();
        assert_eq!(films.title, "Films");
        columns.show_in_tab(&films.id).expect("new tab");
        assert_eq!((columns.tab_count(), columns.active_tab()), (2, 1));
        assert_eq!(columns.tab_title(1), "Films");
        assert_eq!(
            columns.tab_title(0),
            columns.workspace().root_label(),
            "the first tab stays on the drive's folder"
        );
    }

    /// The list of the drive's folder with Films opened in place.
    fn listed(columns: &mut Columns) -> Vec<String> {
        let rows = columns.list_rows(0);
        columns.show_list(&rows);
        rows.iter()
            .map(|row| {
                row.placeholder.map_or_else(
                    || row.entry.title.clone(),
                    |placeholder| placeholder.words("en").to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn the_lists_menu_acts_in_the_clicked_files_folder_on_the_files_selected_there() {
        let (_root, _metadata, mut columns) = columns();
        let films = columns.columns()[0].rows[2].id.clone();
        columns.toggle_expanded(&films);
        assert_eq!(
            listed(&mut columns),
            ["AGENTS", "Claude Code", "Films", "Shots", "cut.mp4"]
        );
        let inside = Location {
            folder_id: Some(films.clone()),
        };
        columns
            .press_row(Table::List, Some(0), Mode::Replace)
            .expect("AGENTS");
        columns
            .press_row(Table::List, Some(3), Mode::Toggle)
            .expect("Shots");
        columns
            .press_row(Table::List, Some(4), Mode::Toggle)
            .expect("cut");
        // A row inside Films: the menu acts in Films, on the files selected there.
        assert_eq!(
            columns.context_rows(Table::List, Some(4)).expect("menu"),
            [3, 4]
        );
        assert_eq!(columns.list_command_column(), usize::MAX);
        assert_eq!(columns.location_of(usize::MAX), Some(inside.clone()));
        // The keys' commands act in the folder the selection shares, or else the one shown.
        assert_eq!(columns.rename_row(Table::List), None, "three are selected");
        assert_eq!(columns.list_command_column(), 0);
        columns
            .press_row(Table::List, Some(0), Mode::Toggle)
            .expect("AGENTS out");
        assert_eq!(
            columns.table_folder(Table::List),
            Some(inside.clone()),
            "a paste lands in the folder the selection shares"
        );
        // A row of the folder shown, or the empty space: the folder shown.
        assert_eq!(
            columns.context_rows(Table::List, Some(1)).expect("menu"),
            [1]
        );
        assert_eq!(columns.list_command_column(), 0);
        assert!(columns
            .context_rows(Table::List, None)
            .expect("background")
            .is_empty());
        assert_eq!(
            chosen_titles(&columns, Table::List).len(),
            1,
            "the background menu selects nothing"
        );
    }

    #[test]
    fn the_lists_selection_holds_its_files_and_lets_go_of_a_placeholder() {
        let (root, _metadata, mut columns) = columns();
        let films = columns.columns()[0].rows[2].id.clone();
        let inside = columns
            .workspace()
            .all_contents(&Location {
                folder_id: Some(films.clone()),
            })
            .expect("films");
        let shots = inside
            .iter()
            .find(|entry| entry.title == "Shots")
            .expect("Shots")
            .id
            .clone();
        columns.toggle_expanded(&films);
        columns.toggle_expanded(&shots);
        assert_eq!(
            listed(&mut columns),
            [
                "AGENTS",
                "Claude Code",
                "Films",
                "Shots",
                "Empty folder",
                "cut.mp4"
            ]
        );
        // ↓ walks through the row standing in for Shots' files, as through any row.
        columns.focus_row(Table::List, 3).expect("Shots");
        assert_eq!(
            columns
                .step_rows(Table::List, crate::list::row_keys::Direction::Next, false)
                .expect("down"),
            Some(4)
        );
        assert_eq!(
            columns
                .step_rows(Table::List, crate::list::row_keys::Direction::Next, true)
                .expect("down"),
            Some(5)
        );
        assert_eq!(
            columns.rename_row(Table::List),
            Some(5),
            "the one file among them"
        );
        assert!(
            columns.pick_up_rows(Table::List, 4).is_empty(),
            "no file to carry"
        );
        // A re-listing keeps the files selected, by id, and lets the placeholder go.
        fs::write(root.path().join("0 first.txt"), b"x").expect("new file");
        columns.refresh().expect("refresh");
        assert_eq!(listed(&mut columns)[0], "0 first.txt");
        assert_eq!(columns.table_selection(Table::List).rows, [6]);
        // Another view starts the list with nothing selected.
        columns.set_view(View::Board);
        columns.set_view(View::List);
        assert!(columns.table_selection(Table::List).rows.is_empty());
    }
}
