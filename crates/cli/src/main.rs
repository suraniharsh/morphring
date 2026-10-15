//! The `morphring` binary: logging, Ctrl+C, then hands the arguments to
//! `morphring::run`.

use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

use morphring_core::CancelToken;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

/// Exit code of the second Ctrl+C, the shell's usual 128 + SIGINT.
const EXIT_INTERRUPTED: i32 = 130;

/// Our crates, which `--verbose` turns up to debug.
const OUR_CRATES: [&str; 3] = ["morphring", "morphring_core", "morphring_engines"];

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    init_logging(verbose_requested(&args));

    let cancel = CancelToken::new();
    let presses = Presses::new(cancel.clone());
    if let Err(e) = ctrlc::set_handler(move || {
        if presses.press() == Press::Quit {
            std::process::exit(EXIT_INTERRUPTED);
        }
    }) {
        // Not fatal: Ctrl+C then just kills us the old-fashioned way.
        tracing::warn!("cannot install the Ctrl+C handler: {e}");
    }

    // Unlocked handles on purpose: worker threads log to stderr through
    // tracing, and holding the stderr lock here would leave them waiting on a
    // lock we never let go of.
    let code = morphring::run(args, &mut std::io::stdout(), &mut std::io::stderr(), cancel);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

/// Logs go to stderr at `warn` by default. `RUST_LOG` picks any level, and
/// `--verbose` raises MorphRing's own crates to `debug` (D5).
fn init_logging(verbose: bool) {
    let mut filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .from_env_lossy();
    if verbose {
        for krate in OUR_CRATES {
            if let Ok(directive) = format!("{krate}=debug").parse() {
                filter = filter.add_directive(directive);
            }
        }
    }
    // try_init: if something already set a subscriber, theirs wins and we move on.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// True when `--verbose` appears before a `--` terminator. We need it before
/// clap runs, since the subscriber has to exist before anything logs.
fn verbose_requested(args: &[OsString]) -> bool {
    args.iter()
        .take_while(|a| a.as_os_str() != "--")
        .any(|a| a.as_os_str() == "--verbose")
}

/// What one Ctrl+C should do.
#[derive(Debug, PartialEq, Eq)]
enum Press {
    /// First press: cancel politely, let the job clean up after itself.
    Cancel,
    /// Second press: the user has run out of patience, and so have we.
    Quit,
}

/// Counts Ctrl+C presses: the first fires the cancel token, every later one quits.
struct Presses {
    count: AtomicUsize,
    cancel: CancelToken,
}

impl Presses {
    fn new(cancel: CancelToken) -> Self {
        Self {
            count: AtomicUsize::new(0),
            cancel,
        }
    }

    fn press(&self) -> Press {
        if self.count.fetch_add(1, Ordering::SeqCst) == 0 {
            self.cancel.cancel();
            Press::Cancel
        } else {
            Press::Quit
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn verbose_is_found_anywhere_before_dash_dash() {
        assert!(verbose_requested(&os(&["--verbose", "doctor"])));
        assert!(verbose_requested(&os(&[
            "convert",
            "a.png",
            "--to",
            "jpg",
            "--verbose"
        ])));
        assert!(!verbose_requested(&os(&["convert", "--", "--verbose"])));
        assert!(!verbose_requested(&os(&["-v"])));
        assert!(!verbose_requested(&os(&[])));
    }

    #[test]
    fn first_ctrl_c_cancels_and_the_second_quits() {
        let cancel = CancelToken::new();
        let presses = Presses::new(cancel.clone());
        assert!(!cancel.is_cancelled());
        assert_eq!(presses.press(), Press::Cancel);
        assert!(cancel.is_cancelled());
        assert_eq!(presses.press(), Press::Quit);
        assert_eq!(presses.press(), Press::Quit);
    }
}
