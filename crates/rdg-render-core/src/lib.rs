//! Shared style resolution, typography, and edge-routing geometry consumed by every
//! `rdg` render backend (draw.io, SVG).
//!
//! Each backend encodes the results into its own output format; this crate is where the
//! two backends can't drift apart on what a given node type or edge style *means*.

pub mod canvas;
pub mod review;
pub mod routing;
pub mod style;
pub mod typography;
