//! A graded case of the `--family` gate. The session driver records cases
//! while the family script runs, so the type sits below both modules.

use crate::solve::CaseOutcome;

/// One graded case of the `--family` gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FamilyCase {
    /// Stable case name, for example `revision-plus-one`.
    pub name: &'static str,
    /// `family_id@revision` for a per-revision case, `None` for a session case.
    pub subject: Option<String>,
    /// How it ended.
    pub outcome: CaseOutcome,
}
