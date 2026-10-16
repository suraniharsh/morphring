//! macOS Finder integration.
//!
//! A stub that changes nothing yet. XP-11 fills it in with a Finder Quick
//! Action, `~/Library/Services/Convert with MorphRing.workflow`, that hands
//! the selected files to the app (`docs/research/macos-support.md` section 6),
//! reported as [`Change`](crate::integration::Change)s.

use std::path::Path;

use crate::integration::{Change, IntegrationError};

/// Adds the Finder menu entry starting `exe`. Changes nothing yet.
pub fn install(exe: &Path, home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    // Finder will get its turn. For now it gets a polite no-op.
    let _ = (exe, home, dry_run);
    Ok(Vec::new())
}

/// Removes the Finder menu entry. Changes nothing yet.
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
