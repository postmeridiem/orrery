//! Core simulation for the orrery wallpaper: where the planets are, how big
//! they are, and how to lay them out on screen.
//!
//! Deliberately free of GPU and platform dependencies so the astronomy can be
//! tested against JPL Horizons on its own.

pub mod almanac;
pub mod bodies;
pub mod config;
pub mod ephemeris;
pub mod lookup;
pub mod scale;
pub mod scene;
pub mod time;

pub use ephemeris::Planet;
pub use lookup::{Lookup, Source};
pub use time::JulianDate;
