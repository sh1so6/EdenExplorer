# UI probe (dev only)

A scripted, self-screenshotting EdenExplorer instance for verifying UI changes
without touching the real mouse, the foreground window, the user's settings or
a normally running EdenExplorer.

The probe lives inside the app (`src/core/probe.rs`) behind the `ui-probe`
cargo feature. Normal builds do not contain it.

## Build

Use a separate target dir so the normal `target/release` exe is neither
rebuilt nor locked:

```powershell
$env:CARGO_TARGET_DIR = "target/probe"
cargo build --release --features ui-probe
Remove-Item Env:CARGO_TARGET_DIR
```

## Run

```powershell
./scripts/ui-probe.ps1 -Script scripts/probe/sidebar_separator.txt -Seed scripts/probe/seed_vertical_tabs
```

| Parameter     | Default                                  | Meaning                                                                 |
| ------------- | ---------------------------------------- | ----------------------------------------------------------------------- |
| `-Script`     | (required)                               | Probe script to run.                                                    |
| `-DataDir`    | `%TEMP%\eden-probe-data`                 | Isolated data dir. Deleted and recreated on every run.                  |
| `-Seed`       | none                                     | Directory whose contents are copied into the data dir (e.g. `layout.json`). |
| `-Exe`        | `target\probe\release\EdenExplorer.exe`  | Probe binary.                                                           |
| `-TimeoutSec` | `60`                                     | After this, only the probe PID is killed.                               |

The helper prints the probe log plus evidence that the run was non-intrusive:
the real cursor position and foreground window (before, after and sampled
during the run), the probe window rectangle and whether
`%LOCALAPPDATA%\ExplorerEden` changed (it must stay `unchanged`).
Cursor or foreground changes made by *you* during the run show up there too.

Running the exe by hand works as well: set `EDEN_PROBE_SCRIPT=<script>` and
optionally `EDEN_PROBE_DATA_DIR=<dir>`. Without `EDEN_PROBE_SCRIPT` a
`ui-probe` build behaves exactly like a normal build.

## What probe mode changes

- Data files (`settings.bin`, `theme.bin`, `tags.bin`, `layout.json`,
  `favorites\`) live directly in the probe data dir.
- No single-instance mutex and no path forwarding. The window title is
  `EdenExplorer Probe`, so a normal instance's forwarding
  (`FindWindowW("EdenExplorer")`) never targets it.
- The window is created at (-20000, -20000), without activation, without a
  taskbar button and at the scripted size. The saved window size/mode is not
  applied. Off-screen windows still render normally and screenshots come from
  the renderer, so this does not affect them.
- Every OS input event (mouse, keyboard, focus, file drops) is dropped. Only
  scripted events reach egui, and the window always reports itself as focused.
- The app never starts actions that would use the real cursor or activate the
  window: native OLE drag-out, title-bar window drag, maximize/restore,
  minimize and showing the native file/folder context menu are disabled. A
  right-click on an item still builds that menu (for its timings, see
  `probe/context_native_timing.txt`) but never shows it, so its entries,
  Eden's own included, cannot be clicked.

## Script format

The script is line-based. `#` starts a comment line. Coordinates are egui
points (logical px) measured from the top-left of the window client area.
A screenshot at `pixels_per_point` 1 maps 1:1 to these points; the log prints
the factor.

| Command                        | Effect                                                                                         |
| ------------------------------ | ---------------------------------------------------------------------------------------------- |
| `size W H`                     | Initial inner size (default 1117x717). Must come before any other command.                    |
| `wait_frames N`                | Wait N frames.                                                                                 |
| `wait_ms N`                    | Wait N milliseconds.                                                                           |
| `move X Y`                     | Move the pointer.                                                                              |
| `down [left\|right\|middle]`   | Press a button at the current pointer position.                                                |
| `up [left\|right\|middle]`     | Release a button.                                                                              |
| `click X Y [button] [ctrl] [shift] [alt]` | Move, press, then release, each on its own frame, with the given modifiers held.  |
| `drag X1 Y1 X2 Y2 [steps]`     | Move, press, then `steps` interpolated moves (default 10, one per frame), then release (left button). |
| `wheel DY`                     | Scroll DY points. Positive scrolls up, negative scrolls down.                                  |
| `key NAME [ctrl] [shift] [alt]` | Press and release an `egui::Key` (`T`, `Tab`, `Escape`, `F2`, `ArrowDown`, …).                 |
| `text STRING`                  | Send the rest of the line as an `egui::Event::Text`.                                           |
| `shot PATH`                    | Save a PNG screenshot. A relative PATH is resolved from the script's directory.                |
| `quit`                         | Close the app. The app also closes at the end of the script.                                   |

Timing:

- 5 warm-up frames run before the first command. Use `wait_ms` to let async
  content (drives, folders, thumbnails) load.
- After every input command the probe waits 2 frames.
- `shot` waits until at least 3 frames and 250 ms have passed since the last
  input, so hover animations can settle. The next command only runs after the
  PNG is written. A screenshot that has not arrived after 5 s is logged as an
  error.

Every executed command, saved screenshot and error goes to
`<script name>.log` next to the script. Unknown or malformed commands are
logged as `ERROR` and skipped. Outputs under `scripts/probe/out/` and
`*.log` are git-ignored.

## Limitations

- Tooltips appear only after egui's hover delay. Add `wait_ms 600` before
  the `shot`.
- Native OLE drag and drop (drag-out to other apps, drops from Explorer)
  cannot be tested. Internal egui drags work.
- IME composition is not simulated. `text` sends committed text only.
- Modal OS UI started by a scripted click, such as `rfd` file dialogs, is real
  and appears on screen. Avoid those flows in scripts. Right-clicking an item
  still asks the shell for its menu entries, which loads the installed shell
  extensions as usual.
- The clipboard is the real system clipboard. Copy and cut commands in a script
  overwrite it.
- File operations act on real paths. Point scripts at scratch folders.
