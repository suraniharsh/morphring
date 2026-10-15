//! The `morphring` binary: hands the arguments to `morphring::run`.

use std::process::ExitCode;

use morphring_core::CancelToken;

fn main() -> ExitCode {
    // TODO(T15): logging and two-stage Ctrl+C. Until then, Ctrl+C just kills us,
    // which is rude but at least quick.
    let code = morphring::run(
        std::env::args_os().skip(1),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
        CancelToken::new(),
    );
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
