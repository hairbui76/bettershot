//! Platform-agnostic annotation model for bettershot.
//!
//! This crate holds everything that is neither operating-system nor renderer
//! specific: geometry, style, the tool state machines, the drawables they
//! produce, and the undoable document that owns them. It has no windowing, GPU
//! or OS dependencies, which is what lets it compile for every target and hold
//! nearly all of the project's tests.
//!
//! Parts are adapted from [Satty](https://github.com/Satty-org/Satty)
//! (MPL-2.0), with GTK and femtovg types replaced by toolkit-neutral ones.
//!
//! # Coordinate spaces
//!
//! Everything here is in **image-pixel space**: the origin is the top-left of
//! the screenshot and one unit is one pixel of the source image. Zoom, pan and
//! HiDPI scaling belong to the app shell, which applies them when translating
//! input in and when rendering out. Mixing the two spaces is the classic bug
//! in this kind of program, so the boundary is deliberately narrow.
//!
//! # How a stroke becomes a saved pixel
//!
//! 1. The shell turns raw pointer input into [`input::MouseEvent`]s via
//!    [`input::PointerTracker`].
//! 2. The active [`tools::Tool`] consumes them and returns a
//!    [`tools::ToolUpdateResult`], previewing its work as a
//!    [`drawable::Drawable`].
//! 3. On completion the tool emits `Commit`, and [`scene::Scene`] takes
//!    ownership, making the step undoable.
//! 4. Every frame — and again at export — the scene replays its drawables onto
//!    a [`painter::Painter`].

pub mod config;
pub mod drawable;
pub mod input;
pub mod math;
pub mod painter;
pub mod path;
pub mod scene;
pub mod style;
pub mod tools;

pub use config::Config;
pub use drawable::Drawable;
pub use math::{Rect, Vec2D};
pub use painter::{ImageEffect, Painter};
pub use scene::Scene;
pub use style::{Color, Size, Style};
pub use tools::{Tool, ToolEvent, ToolUpdateResult, Tools};
