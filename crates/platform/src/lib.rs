//! Desktop integration for MorphRing: context menus, XDG paths,
//! reveal-in-folder and the gesture state machine.
//!
//! [`integration`] is the front door: it adds or removes the "Convert with
//! MorphRing" menu entry on whatever desktop it was built for and hands the
//! work to one backend per OS:
//!
//! | OS | Backend |
//! |---|---|
//! | Linux | [`context_menu`] (Dolphin, Nautilus, Nemo, Thunar) |
//! | Windows | [`explorer`] |
//! | macOS | [`finder`] |
//!
//! Every backend compiles on every OS, so a Linux build still type-checks
//! the Windows and macOS code paths.

pub mod context_menu;
pub mod explorer;
pub mod finder;
pub mod gesture;
pub mod integration;
