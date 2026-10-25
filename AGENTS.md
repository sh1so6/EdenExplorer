# EdenExplorer agent notes

## Build
- After collecting the results of delegated agents (workers), finish with
  `cargo build --release` so `target/release/EdenExplorer.exe` is up to date.
  - If a running EdenExplorer locks the exe, you may close it: first tell the
    user that you are closing it, then close it gracefully (WM_CLOSE, e.g.
    `taskkill /PID <pid>` without `/F`) and wait for the process to exit
    (timeout). Never use `/F`; if it does not exit, report to the user.
  - Open tabs are saved in `session.json` and restored on the next start
    (builds that include session restore). After the release build, relaunch
    `target/release/EdenExplorer.exe` detached (e.g. PowerShell
    `Start-Process`) if it was running before, once the old process is gone.
  - Only one cargo build at a time; use `target/probe` (see
    `scripts/UI_PROBE.md`) for UI-probe builds so the release exe is untouched.
