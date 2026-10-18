//! One dialog pattern for everything the surface opens - Options, About,
//! Profiles, the profile name prompt, the delete confirmation: a modal card
//! sized to its content with a title row and an X.
//!
//! How a dialog may be dismissed depends on what it holds:
//! - `Dismiss::Anywhere` - information or immediate-apply tools (About,
//!   Profiles, the delete question): X, Esc, or a click outside.
//! - `Dismiss::Explicit` - pending edits the user could lose (Options, the
//!   name prompt): X or Esc, which the caller treats as Cancel, but never a
//!   stray click outside. These dialogs bring their own Cancel / confirm
//!   footer; a footer Close on a dialog that needs no answer would be noise.
//!
//! Being modal is what makes the rest simple: while a dialog is up the main
//! surface is inert, so no keystroke typed "elsewhere" can reach it.

use egui::{Context, Id, Ui};

use crate::ui::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dismiss {
    Anywhere,
    Explicit,
}

/// Render dialog `id` this frame; `body` gets the frame number of this
/// opening (1 on the first frame). Returns whether the user dismissed it -
/// the caller clears its own "open" flag on true (and, for
/// `Dismiss::Explicit`, treats it as Cancel).
pub fn show(
    ctx: &Context,
    id: &str,
    title: &str,
    dismiss: Dismiss,
    body: impl FnOnce(&mut Ui, u8),
) -> bool {
    let id = Id::new(("dialog", id));
    // The click that opened a dialog is still in this frame's input, and
    // egui would read it as a click outside the new modal; dismissal only
    // counts from the second frame on.
    // Counted per opening: a dialog not drawn on the previous frame starts
    // again at 1, however it was closed (X, Esc, or a caller's own button).
    let now = ctx.cumulative_frame_nr();
    let frames = ctx.data_mut(|d| {
        let (last, n) = d.get_temp_mut_or_default::<(u64, u8)>(id);
        // Same frame again (egui can run a second pass) keeps the count.
        *n = match now.checked_sub(*last) {
            Some(0) => *n,
            Some(1) => n.saturating_add(1),
            _ => 1,
        };
        *last = now;
        *n
    });
    if frames < 2 {
        ctx.request_repaint(); // an idle app would never reach frame 2
    }
    let mut close = false;
    let resp = egui::Modal::new(id).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading(title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if close_x(ui).clicked() {
                    close = true;
                }
            });
        });
        ui.add_space(2.0);
        body(ui, frames);
    });
    if frames >= 2 {
        close |= match dismiss {
            Dismiss::Anywhere => resp.should_close(),
            Dismiss::Explicit => {
                resp.is_top_modal
                    && !resp.any_popup_open
                    && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            }
        };
    }
    close
}

/// A dialog's button row: right to left, so the first button added takes
/// the far right (Cancel, by convention), the primary just left of it. One
/// row high on purpose: a bare right-to-left layout fills the height the
/// modal remembers from its previous content and leaves a gap above the
/// buttons when that content shrinks.
pub fn footer(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
}

/// The title row's X: a frameless square with a painted cross.
fn close_x(ui: &mut Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    let resp = resp.on_hover_text("Close (Esc)");
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            let w = ui.style().interact(&resp);
            ui.painter()
                .rect_filled(rect, w.corner_radius, w.weak_bg_fill);
        }
        let c = rect.center();
        let d = 4.5;
        let stroke = egui::Stroke::new(1.6, theme::ink_secondary());
        ui.painter()
            .line_segment([c + egui::vec2(-d, -d), c + egui::vec2(d, d)], stroke);
        ui.painter()
            .line_segment([c + egui::vec2(-d, d), c + egui::vec2(d, -d)], stroke);
    }
    resp
}
