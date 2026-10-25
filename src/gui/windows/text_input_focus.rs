//! Which text input owns the keyboard this frame.
//!
//! Item viewer shortcuts (Enter opens the selection, Delete, Backspace, arrow
//! keys, type-to-filter, ...) must never act on a keystroke that was typed
//! into a text input such as the address bar, the rename box or a tag name.
//!
//! Checking `has_focus()` while the item viewer runs is not enough: a
//! single-line `TextEdit` gives up its focus in the same frame that it
//! handles Enter, and egui clears the focus for Escape before any widget
//! runs. In both frames the item viewer would see the key with no text input
//! focused, so Enter in the address bar also opened the selected item.
//!
//! [`end_frame`] records the text input that has focus when a frame ends.
//! [`begin_frame`] keeps that record for the next frame together with the
//! text input focused when it starts. A text input in either record, or one
//! that has focus right now, owns the keyboard for the whole frame
//! ([`text_input_owns_keyboard`]).

use egui::{Context, Id};

/// Focused text inputs, recorded at the end of the previous frame and at the
/// start of this one.
#[derive(Clone, Copy, Debug, Default)]
struct Snapshot {
    previous_frame_end: Option<Id>,
    frame_start: Option<Id>,
}

fn snapshot_id() -> Id {
    Id::new("eden_text_input_focus_snapshot")
}

fn last_frame_end_id() -> Id {
    Id::new("eden_text_input_focus_last_frame_end")
}

/// The focused widget, if it is a text input.
fn focused_text_input(ctx: &Context) -> Option<Id> {
    let id = ctx.memory(|mem| mem.focused())?;
    egui::text_edit::TextEditState::load(ctx, id).map(|_| id)
}

/// Call once per frame before any widget is drawn.
pub fn begin_frame(ctx: &Context) {
    let previous_frame_end = ctx.data(|data| data.get_temp::<Option<Id>>(last_frame_end_id()));
    let snapshot = Snapshot {
        previous_frame_end: previous_frame_end.flatten(),
        frame_start: focused_text_input(ctx),
    };
    ctx.data_mut(|data| data.insert_temp(snapshot_id(), snapshot));
}

/// Call once per frame after every widget has been drawn.
pub fn end_frame(ctx: &Context) {
    let focused = focused_text_input(ctx);
    ctx.data_mut(|data| data.insert_temp(last_frame_end_id(), focused));
}

/// Whether a text input other than `except` owns the keyboard this frame.
/// Pass the caller's own text input (e.g. its filter box) as `except` when the
/// caller handles that input's keys itself.
pub fn text_input_owns_keyboard(ctx: &Context, except: Option<Id>) -> bool {
    let snapshot = ctx
        .data(|data| data.get_temp::<Snapshot>(snapshot_id()))
        .unwrap_or_default();
    [
        snapshot.previous_frame_end,
        snapshot.frame_start,
        focused_text_input(ctx),
    ]
    .into_iter()
    .flatten()
    .any(|id| Some(id) != except)
}
