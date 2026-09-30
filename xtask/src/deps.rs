//! The dependency table, for the audit trail in `docs/DEPENDENCIES.md`.

use crate::workspace::{Workspace, dependency_report};

pub(crate) fn cmd() -> Result<(), String> {
    let ws = Workspace::load()?;
    print!("{}", dependency_report(&ws));
    Ok(())
}
