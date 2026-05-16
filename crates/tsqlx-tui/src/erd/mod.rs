//! ERD rendering primitives and (in PR #2) the whole-schema canvas.
//!
//! The pieces live here so the giant `lib.rs` stays focused on app
//! state + event handling. Anything in `primitives` is pure: no
//! `AppState`, no async, no I/O — just grids of styled cells.

pub mod canvas;
pub mod layout;
pub mod mouse;
pub mod primitives;
pub mod viewport;

pub use canvas::render_schema_canvas;
pub use primitives::render_focus_canvas;
pub use viewport::{Viewport, Zoom};
