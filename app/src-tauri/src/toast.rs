//! The toast window for jobs that outlive the ring.
//!
//! A stub: P2-54 fills it in, driven only by the `job-*` events documented in
//! `commands.rs`.

/// Label of the toast window.
pub const TOAST_WINDOW: &str = "toast";

/// Removes the card of job `job_id` from the toast.
#[tauri::command]
pub fn toast_dismiss(job_id: u64) -> Result<(), String> {
    let _ = job_id;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dismiss_is_a_quiet_no_op_for_now() {
        assert_eq!(toast_dismiss(7), Ok(()));
    }
}
