# EdenExplorer agent notes

## Build
- After collecting the results of delegated agents (workers), finish with
  `cargo build --release` so `target/release/EdenExplorer.exe` is up to date.
  - If a running EdenExplorer locks the exe and the build fails, tell the user
    instead of killing the process.
  - Only one cargo build at a time; use `target/probe` (see
    `scripts/UI_PROBE.md`) for UI-probe builds so the release exe is untouched.
