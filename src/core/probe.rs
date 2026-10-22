//! Dev-only UI probe (`ui-probe` cargo feature).
//!
//! When the binary is built with `--features ui-probe` *and* the environment
//! variable `EDEN_PROBE_SCRIPT` points at a script file, the app runs in probe
//! mode:
//!
//! * all app data (settings, theme, tags, favorites, layout) lives in an
//!   isolated directory (`EDEN_PROBE_DATA_DIR`, default `%TEMP%\eden-probe-data`);
//! * the single-instance mutex / path forwarding is skipped and the window gets a
//!   different title, so it never interferes with a normally running instance;
//! * the window is created far off-screen, without activation and without a
//!   taskbar button;
//! * every OS input event is dropped and replaced by synthetic egui events read
//!   from the script, so the real mouse/keyboard are never touched or needed;
//! * `shot PATH` saves an egui viewport screenshot as PNG.
//!
//! See `scripts/UI_PROBE.md` for the script format.

use eframe::egui;
use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const SCRIPT_ENV: &str = "EDEN_PROBE_SCRIPT";
const DATA_DIR_ENV: &str = "EDEN_PROBE_DATA_DIR";

/// Window title used in probe mode. Must differ from the normal title so that
/// path forwarding (`FindWindowW(None, "EdenExplorer")`) never targets the probe.
pub const WINDOW_TITLE: &str = "EdenExplorer Probe";

/// Far outside any realistic monitor layout, but not the -32000 "minimized" sentinel.
const OFFSCREEN_POS: egui::Pos2 = egui::pos2(-20000.0, -20000.0);
const DEFAULT_SIZE: egui::Vec2 = egui::vec2(1117.0, 717.0);

/// Frames waited after every input command so hover/press states settle.
const DEFAULT_WAIT_FRAMES: u32 = 2;
/// Frames to let the app start up before the first command runs.
const WARMUP_FRAMES: u32 = 5;
/// A screenshot is only taken once this many frames *and* this much time have
/// passed since the last injected input (egui hover animations take ~83 ms).
const SHOT_SETTLE_FRAMES: u64 = 3;
const SHOT_SETTLE_TIME: Duration = Duration::from_millis(250);
/// Give up on a screenshot that never arrives.
const SHOT_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Activation / configuration

struct Config {
    script_path: PathBuf,
    data_dir: PathBuf,
    size: egui::Vec2,
    lines: Vec<Line>,
    read_error: Option<String>,
}

fn config() -> Option<&'static Config> {
    static CONFIG: OnceLock<Option<Config>> = OnceLock::new();
    CONFIG.get_or_init(load_config).as_ref()
}

fn load_config() -> Option<Config> {
    let script = std::env::var_os(SCRIPT_ENV).filter(|value| !value.is_empty())?;
    let script_path = std::path::absolute(PathBuf::from(script)).ok()?;

    let data_dir = std::env::var_os(DATA_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("eden-probe-data"));
    let data_dir = std::path::absolute(&data_dir).unwrap_or(data_dir);

    // A missing/unreadable script still activates probe mode (isolation must
    // hold); the error is reported in the log and the app quits right away.
    let (lines, read_error) = match std::fs::read_to_string(&script_path) {
        Ok(text) => (parse_script(&text), None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };

    let mut size = DEFAULT_SIZE;
    for line in &lines {
        match &line.command {
            Ok(Command::Size(value)) => size = *value,
            _ => break,
        }
    }

    Some(Config {
        script_path,
        data_dir,
        size,
        lines,
        read_error,
    })
}

fn log_path(script_path: &Path) -> PathBuf {
    script_path.with_extension("log")
}

/// `true` when built with `ui-probe` and `EDEN_PROBE_SCRIPT` is set.
pub fn is_active() -> bool {
    config().is_some()
}

/// Isolated data directory replacing `%LOCALAPPDATA%\ExplorerEden` in probe mode.
pub fn data_dir_override() -> Option<PathBuf> {
    config().map(|config| config.data_dir.clone())
}

/// Makes the main window non-intrusive: off-screen, never activated, no
/// taskbar button, distinct title, reproducible size.
pub fn adjust_native_options(mut options: eframe::NativeOptions) -> eframe::NativeOptions {
    let Some(config) = config() else {
        return options;
    };
    options.viewport = options
        .viewport
        .with_title(WINDOW_TITLE)
        .with_inner_size(config.size)
        .with_position(OFFSCREEN_POS)
        .with_active(false)
        .with_taskbar(false)
        .with_clamp_size_to_monitor_size(false);
    options
}

// ---------------------------------------------------------------------------
// Script parsing

struct Line {
    number: usize,
    text: String,
    command: Result<Command, String>,
}

#[derive(Clone, Debug)]
enum Command {
    Size(egui::Vec2),
    WaitFrames(u32),
    WaitMs(u64),
    Move(egui::Pos2),
    Down(egui::PointerButton),
    Up(egui::PointerButton),
    Click(egui::Pos2, egui::PointerButton),
    Drag {
        from: egui::Pos2,
        to: egui::Pos2,
        steps: u32,
    },
    Wheel(f32),
    Key(egui::Key, egui::Modifiers),
    Text(String),
    Shot(PathBuf),
    Quit,
}

fn parse_script(text: &str) -> Vec<Line> {
    text.lines()
        .enumerate()
        .filter_map(|(index, raw)| {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            Some(Line {
                number: index + 1,
                text: trimmed.to_string(),
                command: parse_command(trimmed),
            })
        })
        .collect()
}

fn parse_command(line: &str) -> Result<Command, String> {
    let mut parts = line.split_whitespace();
    let name = parts.next().unwrap_or_default().to_ascii_lowercase();
    let args: Vec<&str> = parts.collect();

    let num = |index: usize| -> Result<f32, String> {
        let value = args
            .get(index)
            .ok_or_else(|| format!("missing argument #{}", index + 1))?;
        value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("invalid number '{value}'"))
    };
    let count = |index: usize| -> Result<u64, String> {
        let value = args
            .get(index)
            .ok_or_else(|| format!("missing argument #{}", index + 1))?;
        value
            .parse::<u64>()
            .map_err(|_| format!("invalid integer '{value}'"))
    };
    let button = |index: usize| -> Result<egui::PointerButton, String> {
        match args.get(index).map(|s| s.to_ascii_lowercase()).as_deref() {
            None | Some("left") => Ok(egui::PointerButton::Primary),
            Some("right") => Ok(egui::PointerButton::Secondary),
            Some("middle") => Ok(egui::PointerButton::Middle),
            Some(other) => Err(format!("unknown button '{other}'")),
        }
    };

    match name.as_str() {
        "size" => Ok(Command::Size(egui::vec2(num(0)?, num(1)?))),
        "wait_frames" => Ok(Command::WaitFrames(count(0)?.min(u32::MAX as u64) as u32)),
        "wait_ms" => Ok(Command::WaitMs(count(0)?)),
        "move" => Ok(Command::Move(egui::pos2(num(0)?, num(1)?))),
        "down" => Ok(Command::Down(button(0)?)),
        "up" => Ok(Command::Up(button(0)?)),
        "click" => Ok(Command::Click(egui::pos2(num(0)?, num(1)?), button(2)?)),
        "drag" => {
            let steps = if args.len() > 4 { count(4)? } else { 10 };
            Ok(Command::Drag {
                from: egui::pos2(num(0)?, num(1)?),
                to: egui::pos2(num(2)?, num(3)?),
                steps: steps.clamp(1, 1000) as u32,
            })
        }
        "wheel" => Ok(Command::Wheel(num(0)?)),
        "key" => {
            let key_name = args.first().ok_or("missing key name")?;
            let key = egui::Key::from_name(key_name)
                .ok_or_else(|| format!("unknown key '{key_name}'"))?;
            let mut modifiers = egui::Modifiers::NONE;
            for flag in &args[1..] {
                match flag.to_ascii_lowercase().as_str() {
                    "ctrl" => {
                        modifiers.ctrl = true;
                        modifiers.command = true;
                    }
                    "shift" => modifiers.shift = true,
                    "alt" => modifiers.alt = true,
                    other => return Err(format!("unknown modifier '{other}'")),
                }
            }
            Ok(Command::Key(key, modifiers))
        }
        "text" => {
            let rest = line[line.find(char::is_whitespace).ok_or("missing text")?..].trim_start();
            Ok(Command::Text(rest.to_string()))
        }
        "shot" => {
            let path = line[line.find(char::is_whitespace).ok_or("missing path")?..].trim();
            Ok(Command::Shot(PathBuf::from(
                path.replace('/', std::path::MAIN_SEPARATOR_STR),
            )))
        }
        "quit" => Ok(Command::Quit),
        other => Err(format!("unknown command '{other}'")),
    }
}

// ---------------------------------------------------------------------------
// Runtime

enum Step {
    WaitFrames(u32),
    WaitMs(u64),
    WaitUntil(Instant),
    Input(Vec<egui::Event>, egui::Modifiers),
    Shot(PathBuf),
    AwaitShot { path: PathBuf, since: Instant },
    Quit,
}

struct Runner {
    config: &'static Config,
    next_line: usize,
    queue: VecDeque<Step>,
    log: Option<File>,
    pointer: egui::Pos2,
    frame: u64,
    started: Instant,
    last_input_frame: u64,
    last_input_time: Instant,
    finished: bool,
}

impl Runner {
    fn new(config: &'static Config) -> Self {
        let log = File::create(log_path(&config.script_path)).ok();
        let now = Instant::now();
        let mut runner = Self {
            config,
            next_line: 0,
            queue: VecDeque::from([Step::WaitFrames(WARMUP_FRAMES)]),
            log,
            pointer: egui::Pos2::ZERO,
            frame: 0,
            started: now,
            last_input_frame: 0,
            last_input_time: now,
            finished: false,
        };
        runner.log(&format!("probe script: {}", config.script_path.display()));
        runner.log(&format!("data dir: {}", config.data_dir.display()));
        runner.log(&format!(
            "window: \"{WINDOW_TITLE}\" {}x{} pt at ({}, {})",
            config.size.x, config.size.y, OFFSCREEN_POS.x, OFFSCREEN_POS.y
        ));
        if let Some(error) = &config.read_error {
            runner.log(&format!("ERROR cannot read script: {error}"));
        }
        runner
    }

    fn log(&mut self, message: &str) {
        let line = format!(
            "[frame {:>5} {:>8.3}s] {message}",
            self.frame,
            self.started.elapsed().as_secs_f64()
        );
        if let Some(file) = self.log.as_mut() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }

    fn resolve(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.config
                .script_path
                .parent()
                .unwrap_or(Path::new("."))
                .join(path)
        }
    }

    fn mark_input(&mut self) {
        self.last_input_frame = self.frame;
        self.last_input_time = Instant::now();
    }

    fn press(&self, button: egui::PointerButton, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: self.pointer,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn input(events: Vec<egui::Event>) -> Step {
        Step::Input(events, egui::Modifiers::NONE)
    }

    /// Expands the next script line into per-frame steps. Returns `false` at
    /// the end of the script.
    fn load_next_line(&mut self) -> bool {
        let Some(line) = self.config.lines.get(self.next_line) else {
            return false;
        };
        let first_command = self.config.lines[..self.next_line]
            .iter()
            .all(|l| matches!(l.command, Ok(Command::Size(_))));
        self.next_line += 1;

        let command = match &line.command {
            Ok(command) => command.clone(),
            Err(error) => {
                let message = format!("ERROR line {}: {error}: {}", line.number, line.text);
                self.log(&message);
                return true;
            }
        };
        let message = format!("line {}: {}", line.number, line.text);
        self.log(&message);

        let settle = Step::WaitFrames(DEFAULT_WAIT_FRAMES);
        match command {
            Command::Size(_) => {
                if !first_command {
                    self.log("ERROR `size` is only honoured before any other command");
                }
            }
            Command::WaitFrames(n) => self.queue.push_back(Step::WaitFrames(n)),
            Command::WaitMs(ms) => self.queue.push_back(Step::WaitMs(ms)),
            Command::Move(pos) => {
                self.pointer = pos;
                self.queue
                    .extend([Self::input(vec![egui::Event::PointerMoved(pos)]), settle]);
            }
            Command::Down(button) => {
                let event = self.press(button, true);
                self.queue.extend([Self::input(vec![event]), settle]);
            }
            Command::Up(button) => {
                let event = self.press(button, false);
                self.queue.extend([Self::input(vec![event]), settle]);
            }
            Command::Click(pos, button) => {
                self.pointer = pos;
                let down = self.press(button, true);
                let up = self.press(button, false);
                self.queue.extend([
                    Self::input(vec![egui::Event::PointerMoved(pos)]),
                    Step::WaitFrames(1),
                    Self::input(vec![down]),
                    Step::WaitFrames(1),
                    Self::input(vec![up]),
                    settle,
                ]);
            }
            Command::Drag { from, to, steps } => {
                self.pointer = from;
                let down = self.press(egui::PointerButton::Primary, true);
                self.queue.extend([
                    Self::input(vec![egui::Event::PointerMoved(from)]),
                    Step::WaitFrames(1),
                    Self::input(vec![down]),
                    Step::WaitFrames(1),
                ]);
                for i in 1..=steps {
                    let pos = from.lerp(to, i as f32 / steps as f32);
                    self.queue
                        .push_back(Self::input(vec![egui::Event::PointerMoved(pos)]));
                }
                self.pointer = to;
                let up = self.press(egui::PointerButton::Primary, false);
                self.queue
                    .extend([Step::WaitFrames(1), Self::input(vec![up]), settle]);
            }
            Command::Wheel(dy) => {
                self.queue.extend([
                    Self::input(vec![egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, dy),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::NONE,
                    }]),
                    settle,
                ]);
            }
            Command::Key(key, modifiers) => {
                let event = |pressed| egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers,
                };
                self.queue.extend([
                    Step::Input(vec![event(true)], modifiers),
                    Step::Input(vec![event(false)], modifiers),
                    settle,
                ]);
            }
            Command::Text(text) => {
                self.queue
                    .extend([Self::input(vec![egui::Event::Text(text)]), settle]);
            }
            Command::Shot(path) => {
                let path = self.resolve(&path);
                self.queue.push_back(Step::Shot(path));
            }
            Command::Quit => self.queue.push_back(Step::Quit),
        }
        true
    }

    fn handle_screenshot(&mut self, image: &egui::ColorImage, ppp: f32) {
        let Some(Step::AwaitShot { path, .. }) = self.queue.front() else {
            return;
        };
        let path = path.clone();
        self.queue.pop_front();

        let [width, height] = image.size;
        let mut rgba = Vec::with_capacity(width * height * 4);
        for pixel in &image.pixels {
            let [r, g, b, _] = pixel.to_srgba_unmultiplied();
            // The composited window is opaque; drop the swapchain alpha so the
            // PNG looks like what is on screen.
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let result = image::RgbaImage::from_raw(width as u32, height as u32, rgba)
            .ok_or_else(|| "invalid image buffer".to_string())
            .and_then(|img| img.save(&path).map_err(|e| e.to_string()));
        let message = match result {
            Ok(()) => format!(
                "saved {} ({width}x{height} px, pixels_per_point {ppp})",
                path.display()
            ),
            Err(error) => format!("ERROR saving {}: {error}", path.display()),
        };
        self.log(&message);
    }

    fn tick(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.frame += 1;
        loop {
            let Some(step) = self.queue.front_mut() else {
                if !self.load_next_line() {
                    self.log("end of script");
                    self.queue.push_back(Step::Quit);
                }
                continue;
            };
            match step {
                Step::WaitFrames(n) => {
                    if *n == 0 {
                        self.queue.pop_front();
                        continue;
                    }
                    *n -= 1;
                    return;
                }
                Step::WaitMs(ms) => {
                    *step = Step::WaitUntil(Instant::now() + Duration::from_millis(*ms));
                    continue;
                }
                Step::WaitUntil(deadline) => {
                    if Instant::now() >= *deadline {
                        self.queue.pop_front();
                        continue;
                    }
                    return;
                }
                Step::Input(..) => {
                    let Some(Step::Input(events, modifiers)) = self.queue.pop_front() else {
                        unreachable!();
                    };
                    raw_input.modifiers = modifiers;
                    raw_input.events.extend(events);
                    self.mark_input();
                    return;
                }
                Step::Shot(path) => {
                    let settled = self.frame >= self.last_input_frame + SHOT_SETTLE_FRAMES
                        && self.last_input_time.elapsed() >= SHOT_SETTLE_TIME;
                    if !settled {
                        return;
                    }
                    let path = path.clone();
                    ctx.send_viewport_cmd_to(
                        egui::ViewportId::ROOT,
                        egui::ViewportCommand::Screenshot(egui::UserData::default()),
                    );
                    *step = Step::AwaitShot {
                        path,
                        since: Instant::now(),
                    };
                    return;
                }
                Step::AwaitShot { path, since } => {
                    if since.elapsed() < SHOT_TIMEOUT {
                        return;
                    }
                    let message = format!("ERROR screenshot timed out: {}", path.display());
                    self.queue.pop_front();
                    self.log(&message);
                    continue;
                }
                Step::Quit => {
                    self.log("quit");
                    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                    self.finished = true;
                    return;
                }
            }
        }
    }
}

/// Called from `eframe::App::raw_input_hook`: drops all real OS input and
/// injects the scripted events for this frame.
pub fn raw_input_hook(ctx: &egui::Context, raw_input: &mut egui::RawInput) {
    let Some(config) = config() else {
        return;
    };

    // Keep only screenshot results; everything else is real user input that
    // must not reach the probe (mouse passing by, keys, focus, file drops).
    let screenshots: Vec<_> = raw_input
        .events
        .drain(..)
        .filter_map(|event| match event {
            egui::Event::Screenshot { image, .. } => Some(image),
            _ => None,
        })
        .collect();
    raw_input.hovered_files.clear();
    raw_input.dropped_files.clear();
    raw_input.modifiers = egui::Modifiers::NONE;
    // Behave as a focused window so keyboard shortcuts work without the OS focus.
    raw_input.focused = true;

    if raw_input.viewport_id != egui::ViewportId::ROOT {
        return;
    }

    static RUNNER: Mutex<Option<Runner>> = Mutex::new(None);
    let Ok(mut guard) = RUNNER.lock() else {
        return;
    };
    let runner = guard.get_or_insert_with(|| Runner::new(config));
    if runner.finished {
        return;
    }

    let ppp = ctx.pixels_per_point();
    for image in &screenshots {
        runner.handle_screenshot(image, ppp);
    }

    runner.tick(ctx, raw_input);
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands_and_skips_comments() {
        let lines = parse_script(
            "# comment\n\nsize 800 600\nclick 10 20 right\ndrag 1 2 3 4\nwheel -120\n",
        );
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].number, 3);
        assert!(matches!(lines[0].command, Ok(Command::Size(v)) if v == egui::vec2(800.0, 600.0)));
        assert!(matches!(
            lines[1].command,
            Ok(Command::Click(p, egui::PointerButton::Secondary)) if p == egui::pos2(10.0, 20.0)
        ));
        assert!(matches!(
            lines[2].command,
            Ok(Command::Drag { steps: 10, .. })
        ));
        assert!(matches!(lines[3].command, Ok(Command::Wheel(dy)) if dy == -120.0));
    }

    #[test]
    fn parses_key_modifiers_text_and_shot() {
        match parse_command("key T ctrl shift") {
            Ok(Command::Key(egui::Key::T, modifiers)) => {
                assert!(modifiers.ctrl && modifiers.command && modifiers.shift && !modifiers.alt);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            parse_command("text  hello world"),
            Ok(Command::Text(text)) if text == "hello world"
        ));
        assert!(matches!(
            parse_command("shot out/a b.png"),
            Ok(Command::Shot(path)) if path == Path::new("out").join("a b.png")
        ));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_command("bogus").is_err());
        assert!(parse_command("move 1").is_err());
        assert!(parse_command("move x 1").is_err());
        assert!(parse_command("down sideways").is_err());
        assert!(parse_command("key NotAKey").is_err());
        assert!(parse_command("key T hyper").is_err());
    }
}
