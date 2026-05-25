//! Behaviour presets bundled behind single CLI flags.
//!
//! Defines: the preset modules. Currently only [`fast`] (the `--fast` flag's
//! customisable exclude list); future presets live here, one module each.
//! Used by: `main` (reads a preset's lists when its flag is set).
//! Uses: nothing.

pub mod fast;
