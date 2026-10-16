//! Turns the `--to`/`--preset` arguments of `convert` and `watch` into a job,
//! ported from `ResolvePreset` in Filee's `src/Filee.Cli/Cli.cs`
//! (docs/reverse-engineering/07-cli-tests-docs.md section 6).
//!
//! Empty on purpose: P2-41 moves in with `job_from_args`. Until then this is
//! the quietest file in the crate.
