//! Windows Explorer integration.
//!
//! A stub that changes nothing yet. XP-06 fills it in with the stage 1 plan
//! from `docs/research/windows-support.md` section 3: a per-user classic verb
//! under `HKCU\Software\Classes\*\shell\MorphRing` plus a Send To shortcut,
//! both written without admin rights and both reported as
//! [`Change`](crate::integration::Change)s.

use std::path::Path;

use crate::integration::{Change, IntegrationError};

/// Adds the Explorer menu entry starting `exe`. Changes nothing yet.
pub fn install(exe: &Path, home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    // Nothing to do, and doing nothing is the one thing this stub is great at.
    let _ = (exe, home, dry_run);
    Ok(Vec::new())
}

/// Removes the Explorer menu entry. Changes nothing yet.
pub fn uninstall(home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    let _ = (home, dry_run);
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_changes_nothing() {
        let (exe, home) = (Path::new("/opt/morphring-app"), Path::new("/home/u"));
        assert!(install(exe, home, false).unwrap().is_empty());
        assert!(uninstall(home, false).unwrap().is_empty());
    }
}
