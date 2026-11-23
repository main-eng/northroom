//! The GPUI shell's components. The core decides what each shows, where it goes and what using
//! it does; a component draws that and forwards input. Positions come from the core's layouts,
//! so components are placed at a frame rather than laid out by GPUI.

pub mod core;
pub mod display;
pub mod draw;
pub mod fonts;
pub mod hosted;
pub mod hosted_input;
pub mod icons;
pub mod nr;
pub mod platform;
pub mod signs;
pub mod svg_picture;
pub mod text;
pub mod theme;
pub mod wake;

pub mod controls;
pub mod lists;

// Overlays: tooltips, menus, dialogs and notices.
pub mod dialog;
pub mod keys;
pub mod layer;
pub mod menu;
pub mod menu_model;
pub mod motion;
pub mod popover;
pub mod toast;
pub mod tooltip;
