//! renameAnt core: rules, planning, conflict detection, journal and apply.
//!
//! [`scan`] reads the filesystem; [`plan`] is pure and answers existence
//! questions from the [`scan::DirIndex`] snapshot; only [`apply`] renames.
//! The journal records every completed rename.

mod bytes_serde;

pub mod apply;
pub mod conflict;
pub mod filter;
pub mod journal;
pub mod osname;
pub mod plan;
pub mod rule;
pub mod scan;
pub mod xdg;
