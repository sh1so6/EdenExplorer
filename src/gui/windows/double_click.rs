//! Windows-style double-click detection.
//!
//! egui's own `Response::double_clicked` does not match what Windows users
//! expect, which made double-clicks in the item viewer fail:
//!
//! - It allows 0.3 s from release to release. Windows allows the
//!   double-click time (`GetDoubleClickTime`, 500 ms by default) from press to
//!   press, so many ordinary double-clicks were too slow for egui.
//! - egui counts a click within 0.6 s of the click before the previous one as
//!   a triple-click, and a triple-click is not a double-click. A click that
//!   selects an item followed by a quick double-click on it therefore never
//!   opened it. In Windows the press that completes a double-click ends the
//!   sequence, so a later press always starts a new one.
//!
//! [`begin_frame`] feeds every pointer press into a small state machine that
//! follows the Windows rules. Use [`is_double_click`] (or
//! [`primary_double_clicked`]) instead of egui's double-click checks.

use egui::{Context, Event, Id, PointerButton, Pos2, Response};
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXDOUBLECLK, SM_CYDOUBLECLK};

/// When two presses count as a double-click.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Limits {
    /// Longest time from the first press to the second, in seconds.
    max_delay: f64,
    /// Largest horizontal distance between the two presses, in points.
    max_dx: f32,
    /// Largest vertical distance between the two presses, in points.
    max_dy: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct State {
    /// The primary press that a second press can turn into a double-click.
    first_press: Option<(f64, Pos2)>,
    /// The latest primary press completed a double-click. Kept until the next
    /// press, so the release of that press sees it however late it comes.
    latest_press_is_double: bool,
}

impl State {
    fn register_press(&mut self, time: f64, pos: Pos2, button: PointerButton, limits: Limits) {
        if button != PointerButton::Primary {
            // Like Windows: another button between the two presses cancels
            // the double-click.
            self.first_press = None;
            self.latest_press_is_double = false;
            return;
        }

        let is_double = self.first_press.is_some_and(|(first_time, first_pos)| {
            time - first_time <= limits.max_delay
                && (pos.x - first_pos.x).abs() <= limits.max_dx
                && (pos.y - first_pos.y).abs() <= limits.max_dy
        });

        self.latest_press_is_double = is_double;
        // The press that completes a double-click ends the sequence: the next
        // press starts a new one instead of making a triple-click.
        self.first_press = if is_double { None } else { Some((time, pos)) };
    }
}

fn state_id() -> Id {
    Id::new("eden_windows_double_click")
}

/// The system double-click settings, in seconds and egui points. The
/// distances are never smaller than egui's click tolerance, so they are at
/// most as strict as egui's own double-click check was.
fn system_limits(ctx: &Context) -> Limits {
    let (time_ms, width_px, height_px) = unsafe {
        (
            GetDoubleClickTime(),
            GetSystemMetrics(SM_CXDOUBLECLK),
            GetSystemMetrics(SM_CYDOUBLECLK),
        )
    };
    let pixels_per_point = ctx.pixels_per_point().max(f32::EPSILON);
    let min_distance = ctx.options(|options| options.input_options.max_click_dist);
    // The system values span a rectangle centered on the first press.
    let half_extent = |px: i32| (px.max(0) as f32 * 0.5 / pixels_per_point).max(min_distance);

    Limits {
        max_delay: f64::from(time_ms) / 1000.0,
        max_dx: half_extent(width_px),
        max_dy: half_extent(height_px),
    }
}

/// Registers this frame's pointer presses. Call once per frame, before any
/// widget asks [`is_double_click`].
pub fn begin_frame(ctx: &Context) {
    let presses: Vec<(f64, Pos2, PointerButton)> = ctx.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    ..
                } => Some((input.time, *pos, *button)),
                _ => None,
            })
            .collect()
    });
    if presses.is_empty() {
        return;
    }

    let limits = system_limits(ctx);
    ctx.data_mut(|data| {
        let state = data.get_temp_mut_or_default::<State>(state_id());
        for (time, pos, button) in presses {
            state.register_press(time, pos, button, limits);
        }
    });
}

/// Whether the latest primary press completed a double-click.
fn latest_press_is_double(ctx: &Context) -> bool {
    ctx.data(|data| {
        data.get_temp::<State>(state_id())
            .is_some_and(|state| state.latest_press_is_double)
    })
}

/// Whether `response` was clicked this frame with the primary button and that
/// click completed a double-click.
pub fn is_double_click(response: &Response) -> bool {
    response.clicked_by(PointerButton::Primary) && latest_press_is_double(&response.ctx)
}

/// Whether a primary click anywhere this frame completed a double-click.
pub fn primary_double_clicked(ctx: &Context) -> bool {
    ctx.input(|input| input.pointer.button_clicked(PointerButton::Primary))
        && latest_press_is_double(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        max_delay: 0.5,
        max_dx: 6.0,
        max_dy: 6.0,
    };

    fn press(state: &mut State, time: f64, x: f32, y: f32) -> bool {
        state.register_press(time, Pos2::new(x, y), PointerButton::Primary, LIMITS);
        state.latest_press_is_double
    }

    #[test]
    fn second_press_within_time_and_distance_is_double() {
        let mut state = State::default();
        assert!(!press(&mut state, 1.0, 100.0, 100.0));
        assert!(press(&mut state, 1.45, 103.0, 98.0));
    }

    #[test]
    fn slow_or_distant_second_press_is_single() {
        let mut state = State::default();
        assert!(!press(&mut state, 1.0, 100.0, 100.0));
        assert!(!press(&mut state, 1.6, 100.0, 100.0));

        let mut state = State::default();
        assert!(!press(&mut state, 1.0, 100.0, 100.0));
        assert!(!press(&mut state, 1.1, 100.0, 120.0));
    }

    #[test]
    fn slow_press_starts_a_new_sequence() {
        let mut state = State::default();
        assert!(!press(&mut state, 1.0, 100.0, 100.0));
        assert!(!press(&mut state, 1.6, 100.0, 100.0));
        assert!(press(&mut state, 1.8, 100.0, 100.0));
    }

    #[test]
    fn select_click_then_quick_double_click_is_double() {
        // egui reports the third click as a triple-click here.
        let mut state = State::default();
        assert!(!press(&mut state, 1.0, 100.0, 100.0));
        assert!(!press(&mut state, 1.55, 100.0, 100.0));
        assert!(press(&mut state, 1.65, 100.0, 100.0));
    }

    #[test]
    fn press_after_a_double_click_starts_a_new_sequence() {
        let mut state = State::default();
        assert!(!press(&mut state, 1.0, 100.0, 100.0));
        assert!(press(&mut state, 1.1, 100.0, 100.0));
        assert!(!press(&mut state, 1.2, 100.0, 100.0));
        assert!(press(&mut state, 1.3, 100.0, 100.0));
    }

    #[test]
    fn presses_in_the_same_frame_pair_up() {
        let mut state = State::default();
        assert!(!press(&mut state, 1.0, 100.0, 100.0));
        assert!(press(&mut state, 1.0, 100.0, 100.0));
    }

    #[test]
    fn other_button_cancels_the_double_click() {
        let mut state = State::default();
        assert!(!press(&mut state, 1.0, 100.0, 100.0));
        state.register_press(
            1.1,
            Pos2::new(100.0, 100.0),
            PointerButton::Secondary,
            LIMITS,
        );
        assert!(!state.latest_press_is_double);
        assert!(!press(&mut state, 1.2, 100.0, 100.0));
    }
}
