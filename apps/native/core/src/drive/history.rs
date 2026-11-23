//! Undo and redo (⌘Z, ⇧⌘Z) for drive changes a person makes in the app, as in Finder: renames,
//! drag moves, copies in (paste, duplicate, import, drop) and moves to the trash. Changes an
//! agent makes through its tools are not recorded.
//!
//! A change is stored as its steps, each by the paths it left. That is enough to reverse and
//! replay it: a moved file moves back, a copy goes to the trash, a trashed file comes back from
//! where the trash put it. If a file has moved or gone since, the change fails with the drive's
//! own error and is dropped.

use crate::strings::{key, Key, Message};
use crate::workspace::{DriveError, DriveResult, Kept, Workspace};
use std::path::{Path, PathBuf};

/// Maximum changes kept for undo; the oldest is dropped first.
pub const HISTORY_DEPTH: usize = 100;

/// One step of a change.
#[derive(Clone, Debug)]
pub enum Step {
    /// The file at `from` moved to `to`: a rename, a drag, or a restore from the trash.
    Moved { from: PathBuf, to: PathBuf },
    /// A copy created at `path`. Undoing it sends it to the trash.
    Made { path: PathBuf },
    /// The file at `path` was trashed to `landed`; the drive `kept` its ids.
    Trashed {
        path: PathBuf,
        landed: PathBuf,
        kept: Kept,
    },
}

impl Step {
    /// The path named in the error when this step can't be undone: the file that must still be
    /// where the step left it, or the place that must still be free.
    fn blocker(&self) -> Option<&Path> {
        let (present, free) = match self {
            Self::Moved { from, to } => (to.as_path(), Some(from.as_path())),
            Self::Made { path } => (path.as_path(), None),
            Self::Trashed { path, landed, .. } => (landed.as_path(), Some(path.as_path())),
        };
        if std::fs::symlink_metadata(present).is_err() {
            return Some(present);
        }
        free.filter(|free| std::fs::symlink_metadata(free).is_ok())
    }

    /// Reverses the step and returns the inverse step. Returns `None` when the platform's trash
    /// did not report where it put a copy, so the copy can't be restored.
    fn take_back(&self, workspace: &mut Workspace) -> DriveResult<Option<Step>> {
        match self {
            Self::Moved { from, to } => {
                workspace.move_path(to, from)?;
                Ok(Some(Self::Moved {
                    from: to.clone(),
                    to: from.clone(),
                }))
            }
            Self::Made { path } => {
                let trashed = workspace.trash_path(path)?;
                Ok(trashed.landed.map(|landed| Self::Trashed {
                    path: path.clone(),
                    landed,
                    kept: trashed.kept,
                }))
            }
            Self::Trashed { path, landed, kept } => {
                workspace.restore(landed, kept, path)?;
                Ok(Some(Self::Made { path: path.clone() }))
            }
        }
    }
}

/// Which direction a change is being reversed in, for the error wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    Undo,
    Redo,
}

impl Way {
    /// The error when a file the change needs has moved or gone.
    fn refusal(self) -> Key {
        match self {
            Self::Undo => key::DRIVE_UNDO_MOVED,
            Self::Redo => key::DRIVE_REDO_MOVED,
        }
    }
}

/// Undo and redo stacks, newest last.
#[derive(Debug, Default)]
pub struct History {
    done: Vec<Vec<Step>>,
    undone: Vec<Vec<Step>>,
}

impl History {
    /// Records a new change and clears the redo stack. Empty changes are ignored.
    pub fn record(&mut self, steps: Vec<Step>) {
        if steps.is_empty() {
            return;
        }
        self.undone.clear();
        Self::push(&mut self.done, steps);
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Undoes the newest change. `None` when there is none.
    pub fn undo(&mut self, workspace: &mut Workspace) -> Option<DriveResult<()>> {
        let change = self.done.pop()?;
        Some(Self::take_back(
            change,
            &mut self.undone,
            workspace,
            Way::Undo,
        ))
    }

    /// Redoes the last undone change. `None` when there is none.
    pub fn redo(&mut self, workspace: &mut Workspace) -> Option<DriveResult<()>> {
        let change = self.undone.pop()?;
        Some(Self::take_back(
            change,
            &mut self.done,
            workspace,
            Way::Redo,
        ))
    }

    fn push(stack: &mut Vec<Vec<Step>>, steps: Vec<Step>) {
        stack.push(steps);
        if stack.len() > HISTORY_DEPTH {
            stack.remove(0);
        }
    }

    /// Reverses `change` last step first and pushes the inverse steps onto `other`. A file that
    /// has moved or gone fails the whole change before anything runs; a failure part way keeps
    /// the steps already reversed.
    fn take_back(
        change: Vec<Step>,
        other: &mut Vec<Vec<Step>>,
        workspace: &mut Workspace,
        way: Way,
    ) -> DriveResult<()> {
        if let Some(blocker) = change.iter().find_map(Step::blocker) {
            let name = blocker
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            return Err(DriveError::Refused(
                Message::new(way.refusal()).with("name", name),
            ));
        }
        let mut reverse = Vec::with_capacity(change.len());
        let mut outcome = Ok(());
        for step in change.iter().rev() {
            match step.take_back(workspace) {
                Ok(taken) => reverse.extend(taken),
                Err(error) => {
                    outcome = Err(error);
                    break;
                }
            }
        }
        if !reverse.is_empty() {
            Self::push(other, reverse);
        }
        outcome
    }
}
