//! Path checks for archive entries: no absolute paths, no `..`, no links that
//! point outside the destination folder (zip slip and its cousins). Filled in
//! by the archive tasks; empty in the skeleton.
