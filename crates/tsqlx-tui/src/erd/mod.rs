//! ERD rendering primitives and (in PR #2) the whole-schema canvas.
//!
//! The pieces live here so the giant `lib.rs` stays focused on app
//! state + event handling. Anything in `primitives` is pure: no
//! `AppState`, no async, no I/O — just grids of styled cells.

pub mod primitives;

pub use primitives::render_focus_canvas;
