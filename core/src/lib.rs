//! renameAnt core: rules, planning, conflict detection, journal and apply.
//!
//! [`plan`] is pure — it never touches the filesystem and answers existence
//! questions from the [`DirIndex`] snapshot taken by [`scan`]. Only [`apply`]
//! performs syscalls.

pub mod bytes_serde;

pub mod apply;
pub mod conflict;
pub mod filter;
pub mod journal;
pub mod osname;
pub mod plan;
pub mod rule;
pub mod scan;

pub use apply::{
    ApplyError, ApplyOptions, ApplyReport, ApplyRow, Outcome, apply, is_staged, undo, undo_one,
};
pub use filter::{CompiledFilter, Filter, FilterError, KindFilter};
pub use journal::{
    FileId, Journal, JournalRecord, RecordedRename, RunId, RunOutcome, Session, lock_instance,
};
pub use osname::{NAME_MAX, PATH_MAX};
pub use plan::{ConflictKind, InvalidKind, Plan, PlanRow, Status};
pub use rule::{
    CaseStyle, DateFormat, InsertRemoveOp, NormalizeOp, Rule, RuleError, RuleSet, SequencePlace,
    Target,
};
pub use scan::{DirIndex, Entry, Kind, scan_dir, scan_tree};
