//! A check report: one row per rule, a stable exit code, and no colour codes to
//! misparse.

use std::fmt::Write as _;

/// The outcome of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Pass,
    Fail,
    /// The check could not be evaluated here (a tool or corpus is absent). Callers
    /// decide whether that is acceptable; the gauntlet reports it either way.
    Skip,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "FAIL",
            Status::Skip => "skip",
        }
    }

    pub(crate) fn is_ok(self) -> bool {
        matches!(self, Status::Pass)
    }
}

/// One rule from `FINISH.md`, with whatever it found.
#[derive(Debug, Clone)]
pub(crate) struct Check {
    /// The `FINISH.md` identifier, e.g. `G0.2`.
    pub id: &'static str,
    /// What the rule is, in one line.
    pub title: &'static str,
    pub status: Status,
    /// Findings, one per line. Empty when the check passed.
    pub findings: Vec<String>,
}

impl Check {
    pub(crate) fn pass(id: &'static str, title: &'static str) -> Self {
        Self {
            id,
            title,
            status: Status::Pass,
            findings: Vec::new(),
        }
    }

    pub(crate) fn fail(id: &'static str, title: &'static str, findings: Vec<String>) -> Self {
        Self {
            id,
            title,
            status: Status::Fail,
            findings,
        }
    }

    pub(crate) fn skip(id: &'static str, title: &'static str, why: impl Into<String>) -> Self {
        Self {
            id,
            title,
            status: Status::Skip,
            findings: vec![why.into()],
        }
    }

    /// A check that passes, with a note worth recording.
    pub(crate) fn note(mut self, note: impl Into<String>) -> Self {
        self.findings.push(note.into());
        self
    }
}

/// The result of running a set of checks.
#[derive(Debug, Default)]
pub(crate) struct Report {
    checks: Vec<Check>,
}

impl Report {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record `check` only if `wanted` names it. An empty `wanted` means all.
    pub(crate) fn push_if(&mut self, wanted: &[String], check: Check) {
        if wanted.is_empty() || wanted.iter().any(|w| w.eq_ignore_ascii_case(check.id)) {
            self.checks.push(check);
        }
    }

    #[must_use]
    pub(crate) fn checks(&self) -> &[Check] {
        &self.checks
    }

    /// `true` when nothing failed. Skips do not fail a run; the gauntlet lists them.
    #[must_use]
    pub(crate) fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.status.is_ok())
    }

    /// Render for a terminal, and a count of what did not pass.
    #[must_use]
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        for c in &self.checks {
            let _ = writeln!(out, "{:<5} {:<6} {}", c.id, c.status.label(), c.title);
            for f in &c.findings {
                for line in f.lines() {
                    let _ = writeln!(out, "        {line}");
                }
            }
        }
        let failed = self
            .checks
            .iter()
            .filter(|c| c.status == Status::Fail)
            .count();
        let skipped = self
            .checks
            .iter()
            .filter(|c| c.status == Status::Skip)
            .count();
        let _ = writeln!(
            out,
            "\n{} checked, {} failed, {} skipped",
            self.checks.len(),
            failed,
            skipped
        );
        out
    }
}
