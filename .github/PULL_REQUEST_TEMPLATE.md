## What does this change?

<!-- One or two sentences. Link the issue if there is one: Fixes #123 -->

## How was it tested?

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] App changes: `cargo clippy --all-targets -- -D warnings` and `cargo test` in `app/src-tauri`
- [ ] UI changes: `node --test app/ui/*.test.mjs`
- [ ] Tried it for real (describe what you dragged where):

## Platforms

<!-- Platform code must stay cfg-gated so Linux, Windows and macOS all compile. -->

- [ ] Linux
- [ ] Windows
- [ ] macOS

## Screenshots (UI changes)

## Checklist

- [ ] New UI uses the existing palette tokens in `app/ui/*.css`
- [ ] New third-party code, crates or binaries are listed in `THIRD-PARTY-NOTICES.md`
- [ ] User-visible changes are noted in `CHANGELOG.md`
