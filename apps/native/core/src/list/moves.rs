//! What dropping list rows moves. A drag carries the whole selection, including rows expanded
//! inside other folders, and the drop moves each into the target: a folder's row, or the empty
//! space for the folder shown.

/// A dragged row or a drop target row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    /// The folder containing the row's file; `None` for the drive root.
    pub parent: Option<String>,
    /// The expanded folders the row is listed inside, outermost first.
    pub inside: Vec<String>,
    pub folder: bool,
}

/// Files to move out of one folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    /// Their current folder; `None` for the drive root.
    pub source: Option<String>,
    pub ids: Vec<String>,
}

/// The moves for a drop into `destination`, one per source folder, in first-seen order. Rows
/// already in the destination stay. A dragged folder carries the dragged rows inside it, so they
/// get no move of their own.
pub fn plan(dragged: &[Row], destination: Option<&str>) -> Vec<Move> {
    let mut moves: Vec<Move> = Vec::new();
    for row in dragged {
        let carried = dragged
            .iter()
            .any(|other| other.folder && row.inside.contains(&other.id));
        if carried || row.parent.as_deref() == destination {
            continue;
        }
        match moves
            .iter_mut()
            .find(|planned| planned.source == row.parent)
        {
            Some(planned) => planned.ids.push(row.id.clone()),
            None => moves.push(Move {
                source: row.parent.clone(),
                ids: vec![row.id.clone()],
            }),
        }
    }
    moves
}

/// Whether the dragged rows may drop into the folder at row `target`. Never into one of
/// themselves or their contents, and only when something would move.
pub fn takes(dragged: &[Row], target: &Row) -> bool {
    if !target.folder || dragged.is_empty() {
        return false;
    }
    if dragged
        .iter()
        .any(|row| row.id == target.id || target.inside.contains(&row.id))
    {
        return false;
    }
    !plan(dragged, Some(&target.id)).is_empty()
}

/// The row under the pointer and what it accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Over<'a> {
    pub row: usize,
    pub id: &'a str,
    pub folder: bool,
    /// A running terminal, which receives dropped files as references.
    pub takes_references: bool,
}

/// Where a drag of the list's own rows lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    Nowhere,
    /// Into the folder at this row.
    Folder(usize),
    /// To the running terminal at this row.
    Terminal(usize),
    /// Into the folder shown, over the list's empty space.
    Shown,
}

/// Where `dragged` lands over `over` (`None` for empty space, a folder's placeholder row, or
/// between rows). A folder row accepts when `folder_takes` says something would move; a running
/// terminal row always accepts, unless it is being dragged. Anywhere else targets the folder
/// shown, when `shown_takes` says a row would move there.
pub fn drop_place(
    dragged: &[String],
    over: Option<Over>,
    folder_takes: impl FnOnce(&str) -> bool,
    shown_takes: impl FnOnce() -> bool,
) -> Drop {
    if let Some(over) = over.filter(|over| !dragged.iter().any(|id| id == over.id)) {
        if over.folder && folder_takes(over.id) {
            return Drop::Folder(over.row);
        }
        if !over.folder && over.takes_references {
            return Drop::Terminal(over.row);
        }
    }
    if !dragged.is_empty() && shown_takes() {
        Drop::Shown
    } else {
        Drop::Nowhere
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_lands_in_a_folder_that_takes_it_a_terminal_or_the_folder_shown() {
        let dragged = vec!["tone".to_owned()];
        let folder = Over {
            row: 2,
            id: "assets",
            folder: true,
            takes_references: false,
        };
        let terminal = Over {
            row: 3,
            id: "shell",
            folder: false,
            takes_references: true,
        };
        let file = Over {
            row: 4,
            id: "clip",
            folder: false,
            takes_references: false,
        };
        assert_eq!(
            drop_place(&dragged, Some(folder), |_| true, || true),
            Drop::Folder(2)
        );
        // A folder that would move nothing falls through to the folder shown.
        assert_eq!(
            drop_place(&dragged, Some(folder), |_| false, || true),
            Drop::Shown
        );
        assert_eq!(
            drop_place(&dragged, Some(terminal), |_| false, || false),
            Drop::Terminal(3)
        );
        assert_eq!(
            drop_place(&dragged, Some(file), |_| true, || true),
            Drop::Shown
        );
        assert_eq!(
            drop_place(&dragged, None, |_| true, || false),
            Drop::Nowhere
        );
        // A row never takes itself, and nothing dragged lands nowhere.
        let own = Over {
            row: 0,
            id: "tone",
            folder: true,
            takes_references: false,
        };
        assert_eq!(
            drop_place(&dragged, Some(own), |_| true, || false),
            Drop::Nowhere
        );
        assert_eq!(drop_place(&[], None, |_| true, || true), Drop::Nowhere);
    }

    fn row(id: &str, parent: Option<&str>, inside: &[&str], folder: bool) -> Row {
        Row {
            id: id.to_owned(),
            parent: parent.map(str::to_owned),
            inside: inside.iter().map(|id| (*id).to_owned()).collect(),
            folder,
        }
    }

    #[test]
    fn a_row_inside_an_open_folder_moves_to_the_folder_shown() {
        // Assets is open in place; chart is listed inside it. Dropped on the empty space.
        let dragged = [row("chart", Some("assets"), &["assets"], false)];
        assert_eq!(
            plan(&dragged, None),
            [Move {
                source: Some("assets".into()),
                ids: vec!["chart".into()]
            }]
        );
        // Dropped back on its own folder, nothing moves.
        assert!(plan(&dragged, Some("assets")).is_empty());
        assert!(!takes(&dragged, &row("assets", None, &[], true)));
    }

    #[test]
    fn one_drag_carries_rows_from_several_folders() {
        let dragged = [
            row("tone", None, &[], false),
            row("chart", Some("assets"), &["assets"], false),
            row("portrait", Some("assets"), &["assets"], false),
            row("plan", Some("notes"), &["notes"], false),
        ];
        assert_eq!(
            plan(&dragged, Some("empty")),
            [
                Move {
                    source: None,
                    ids: vec!["tone".into()]
                },
                Move {
                    source: Some("assets".into()),
                    ids: vec!["chart".into(), "portrait".into()]
                },
                Move {
                    source: Some("notes".into()),
                    ids: vec!["plan".into()]
                },
            ]
        );
        // Into Notes: its own row stays where it is.
        assert_eq!(
            plan(&dragged, Some("notes"))
                .iter()
                .flat_map(|planned| planned.ids.clone())
                .collect::<Vec<_>>(),
            ["tone", "chart", "portrait"]
        );
        assert!(takes(&dragged, &row("notes", None, &[], true)));
    }

    #[test]
    fn a_dragged_folder_carries_the_dragged_rows_inside_it() {
        let dragged = [
            row("assets", None, &[], true),
            row("chart", Some("assets"), &["assets"], false),
        ];
        assert_eq!(
            plan(&dragged, Some("notes")),
            [Move {
                source: None,
                ids: vec!["assets".into()]
            }]
        );
        // A file isn't a folder, so a row that only looks nested by name is not carried.
        let files = [
            row("a", None, &[], false),
            row("b", Some("x"), &["a"], false),
        ];
        assert_eq!(plan(&files, Some("notes")).len(), 2);
    }

    #[test]
    fn nothing_drops_into_itself_or_what_it_holds() {
        let assets = row("assets", None, &[], true);
        let pictures = row("pictures", Some("assets"), &["assets"], true);
        assert!(!takes(std::slice::from_ref(&assets), &assets));
        assert!(!takes(std::slice::from_ref(&assets), &pictures));
        assert!(takes(
            std::slice::from_ref(&pictures),
            &row("notes", None, &[], true)
        ));
        // A file takes nothing; nor does anything take an empty drag.
        assert!(!takes(
            std::slice::from_ref(&assets),
            &row("tone", None, &[], false)
        ));
        assert!(!takes(&[], &assets));
    }
}
