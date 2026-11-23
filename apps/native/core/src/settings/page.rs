//! A settings page as a shell draws it: a title, the words under it, then sections of rows,
//! each row a label with its controls. The core says every word, which control goes where,
//! how wide it is and what using it does (`action`, handed back to `app_ffi::nr_settings_act`
//! with the control's new value) and where everything goes (`layout`); a shell builds the
//! controls and draws the page where the layout says. The shell takes these values as they
//! are (`app_ffi::settings_page`, `app_ffi::settings_window`): no JSON between them, so the
//! compiler holds both sides to one description.

pub mod layout;

/// Settings' one page: the appearance choices (`appearance::page`).
pub const TAB_APPEARANCE: u32 = 0;

/// Dropdown width, in points.
pub const SELECT_WIDTH: f64 = 192.0;
/// Gap between controls in one row, in points.
pub const CONTROL_GAP: f64 = 8.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub title: String,
    /// Words under the title, wrapped to the page.
    pub summary: Option<String>,
    pub sections: Vec<Section>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub label: String,
    /// Smaller words under the label.
    pub description: Option<String>,
    /// A control at the right of the label: a preset's dropdown.
    pub action: Option<Control>,
    pub rows: Vec<Row>,
}

impl Section {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            description: None,
            action: None,
            rows: Vec::new(),
        }
    }
}

/// A row: its label at the left, its controls `CONTROL_GAP` apart at the right.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    pub controls: Vec<Control>,
}

impl Row {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            controls: Vec::new(),
        }
    }

    /// Adds a control after the ones already in the row.
    pub fn with(mut self, control: Control) -> Self {
        self.controls.push(control);
        self
    }
}

/// One choice of a dropdown: what `act` is handed, its name, and the colour it stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub id: String,
    pub name: String,
    /// `#rrggbb`, a round swatch before the name.
    pub swatch: Option<String>,
}

impl Choice {
    /// A choice that is its own name.
    pub fn named(name: &str) -> Self {
        Self {
            id: name.to_owned(),
            name: name.to_owned(),
            swatch: None,
        }
    }
}

/// A control of a row or a section's heading.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// Words: a value at the right of its row, or a caption between controls.
    Words { text: String },
    Select {
        action: String,
        enabled: bool,
        choices: Vec<Choice>,
        selected: usize,
        width: f64,
    },
    /// Light, Dark and System side by side, the chosen one filled; `act` is handed its index.
    Theme {
        action: String,
        labels: Vec<String>,
        selected: usize,
    },
    /// A colour's hex beside the swatch that opens the picker; `act` is handed each hex picked.
    Color { action: String, hex: String },
    /// 0 to 100; `act` is handed each value as it is dragged.
    Slider { action: String, value: f64 },
}

impl Control {
    pub fn words(text: &str) -> Self {
        Self::Words {
            text: text.to_owned(),
        }
    }

    /// A dropdown of `choices` showing the one whose id is `value`. A value none of them is
    /// shows as an empty choice added last, which picks nothing.
    pub fn select(
        action: &str,
        enabled: bool,
        mut choices: Vec<Choice>,
        value: &str,
        width: f64,
    ) -> Self {
        let selected = match choices.iter().position(|choice| choice.id == value) {
            Some(index) => index,
            None => {
                choices.push(Choice::named(""));
                choices.len() - 1
            }
        };
        Self::Select {
            action: action.to_owned(),
            enabled,
            choices,
            selected,
            width,
        }
    }

    /// The action a person's use of it hands back; words have none.
    pub fn action(&self) -> &str {
        match self {
            Self::Words { .. } => "",
            Self::Select { action, .. }
            | Self::Theme { action, .. }
            | Self::Color { action, .. }
            | Self::Slider { action, .. } => action,
        }
    }
}

impl Page {
    pub fn titled(title: &str) -> Self {
        Self {
            title: title.to_owned(),
            ..Self::default()
        }
    }

    /// The last section, which rows are added to.
    pub fn section(&mut self) -> &mut Section {
        self.sections.last_mut().expect("a row goes in a section")
    }

    pub fn row(&mut self, row: Row) {
        self.section().rows.push(row);
    }

    /// Every control on the page, the headings' too: each section's heading control, then its
    /// rows' controls.
    pub fn controls(&self) -> impl Iterator<Item = &Control> {
        self.sections.iter().flat_map(|section| {
            section
                .action
                .iter()
                .chain(section.rows.iter().flat_map(|row| row.controls.iter()))
        })
    }

    /// The control whose action is `action`.
    pub fn control(&self, action: &str) -> Option<&Control> {
        self.controls().find(|control| control.action() == action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_none_of_the_choices_is_shows_as_an_empty_choice() {
        let choices = vec![Choice::named("UTC"), Choice::named("Asia/Tokyo")];
        let Control::Select {
            choices: shown,
            selected,
            ..
        } = Control::select("tz", true, choices.clone(), "Asia/Tokyo", SELECT_WIDTH)
        else {
            panic!("a select");
        };
        assert_eq!((shown.len(), selected), (2, 1));
        let Control::Select {
            choices: shown,
            selected,
            ..
        } = Control::select("tz", true, choices, "Mars/Olympus", SELECT_WIDTH)
        else {
            panic!("a select");
        };
        assert_eq!((shown.len(), selected, shown[2].name.as_str()), (3, 2, ""));
    }

    #[test]
    fn a_page_finds_a_control_by_its_action() {
        let mut page = Page::titled("Appearance");
        page.sections.push(Section::new("Colours"));
        let contrast = Control::Slider {
            action: "contrast".to_owned(),
            value: 50.0,
        };
        page.row(Row::new("Contrast").with(contrast.clone()));
        assert_eq!(page.summary, None);
        assert_eq!(page.sections[0].rows[0].label, "Contrast");
        assert_eq!(page.control("contrast"), Some(&contrast));
        assert_eq!(page.control("missing"), None);
    }
}
