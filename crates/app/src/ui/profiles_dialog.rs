//! Profiles Manager window: a thin render layer over the CRUD core in
//! `crate::profiles`. Every successful mutation persists immediately through
//! the normal settings save (`App::save_settings`), so profile edits survive
//! a crash without waiting for the exit save.
//!
//! Connection safety: while an NTRIP worker is live, the running job was
//! built from the ACTIVE profile, so switching and deleting are disabled -
//! both the toolbar and the double-click path check `App::ntrip_busy`.
//! Renames stay allowed: the worker holds a clone of the job and identifies
//! nothing by profile name.

use egui::RichText;

use crate::profiles;
use crate::ui::App;

/// Transient window state (selection, inline rename, delete confirmation).
/// Lives on `App` behind the frozen stub contract; `Default` is the reset.
#[derive(Default)]
pub struct ViewState {
    selected: Option<usize>,
    /// Index being inline-renamed; the edit happens in `rename_buf` so a
    /// cancelled rename never touches the settings.
    renaming: Option<usize>,
    rename_buf: String,
    /// One-shot: focus the rename box on the frame it appears.
    rename_focus_pending: bool,
    /// Index awaiting the destructive-action confirmation strip.
    confirm_delete: Option<usize>,
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.show_profiles {
        return;
    }
    let mut open = app.show_profiles;
    // The view state moves out for the frame so the window closure can
    // borrow `app` and the view independently; `ViewState: Default` makes
    // the take free.
    let mut view = std::mem::take(&mut app.profiles_view);
    egui::Window::new("Profiles")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(300.0)
        .show(ctx, |ui| body(app, &mut view, ui));
    app.profiles_view = view;
    app.show_profiles = open;
}

fn body(app: &mut App, view: &mut ViewState, ui: &mut egui::Ui) {
    // The window state outlives list edits made elsewhere; clamp every
    // stored index before use so a stale frame can never panic.
    let n = app.settings.profiles.len();
    if view.selected.is_some_and(|i| i >= n) {
        view.selected = None;
    }
    if view.renaming.is_some_and(|i| i >= n) {
        view.renaming = None;
        view.rename_buf.clear();
    }
    if view.confirm_delete.is_some_and(|i| i >= n) {
        view.confirm_delete = None;
    }

    let busy = app.ntrip_busy();
    if busy {
        ui.label(
            RichText::new("Connected - disconnect to activate or delete profiles.")
                .small()
                .color(ui.visuals().warn_fg_color),
        );
        ui.add_space(2.0);
    }

    egui::ScrollArea::vertical()
        .max_height(260.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            // Justified layout: rows fill the width but keep left-aligned
            // text, so the whole line is a click target.
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                for i in 0..n {
                    row(app, view, ui, i, busy);
                }
            });
        });

    ui.separator();
    toolbar(app, view, ui, busy);
    confirm_strip(app, view, ui, busy);
    ui.add_space(2.0);
    ui.label(
        RichText::new("Double-click a profile to make it active. Changes save immediately.")
            .small()
            .weak(),
    );
}

fn row(app: &mut App, view: &mut ViewState, ui: &mut egui::Ui, i: usize, busy: bool) {
    if view.renaming == Some(i) {
        let resp =
            ui.add(egui::TextEdit::singleline(&mut view.rename_buf).desired_width(f32::INFINITY));
        if view.rename_focus_pending {
            view.rename_focus_pending = false;
            resp.request_focus();
        }
        if ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
            view.renaming = None;
            view.rename_buf.clear();
        } else if resp.lost_focus() {
            // Enter and click-away both commit, file-manager style; the
            // CRUD core rejects blank names and suffixes collisions.
            end_rename(app, view, true);
        }
        return;
    }

    let name = app.settings.profiles[i].name.clone();
    let is_active = name == app.settings.active_profile;
    let label = if is_active {
        RichText::new(format!("{name}  (active)")).strong()
    } else {
        RichText::new(name)
    };
    let resp = ui.selectable_label(view.selected == Some(i), label);
    if resp.clicked() {
        view.selected = Some(i);
    }
    if resp.double_clicked() && !busy {
        view.selected = Some(i);
        activate(app, i);
    }
}

fn toolbar(app: &mut App, view: &mut ViewState, ui: &mut egui::Ui, busy: bool) {
    ui.horizontal(|ui| {
        if ui.button("New").clicked() {
            end_rename(app, view, true);
            let i = profiles::add(&mut app.settings);
            view.selected = Some(i);
            view.confirm_delete = None;
            app.save_settings();
            // A fresh profile almost always wants a real name next.
            begin_rename(view, i, &app.settings.profiles[i].name);
        }

        let sel = view.selected;
        if ui
            .add_enabled(sel.is_some(), egui::Button::new("Duplicate"))
            .on_disabled_hover_text("Select a profile first")
            .clicked()
            && let Some(i) = sel
        {
            end_rename(app, view, true);
            if let Some(j) = profiles::duplicate(&mut app.settings, i) {
                view.selected = Some(j);
                view.confirm_delete = None;
                app.save_settings();
            }
        }

        if ui
            .add_enabled(sel.is_some(), egui::Button::new("Rename"))
            .on_disabled_hover_text("Select a profile first")
            .clicked()
            && let Some(i) = sel
        {
            end_rename(app, view, true);
            begin_rename(view, i, &app.settings.profiles[i].name.clone());
        }

        let block = delete_block_reason(app.settings.profiles.len(), busy, sel.is_some());
        if ui
            .add_enabled(block.is_none(), egui::Button::new("Delete"))
            .on_disabled_hover_text(block.unwrap_or_default())
            .clicked()
            && let Some(i) = sel
        {
            end_rename(app, view, true);
            view.confirm_delete = Some(i);
        }

        let selected_is_active =
            sel.is_some_and(|i| app.settings.profiles[i].name == app.settings.active_profile);
        let block = activate_block_reason(busy, sel.is_some(), selected_is_active);
        if ui
            .add_enabled(block.is_none(), egui::Button::new("Activate"))
            .on_disabled_hover_text(block.unwrap_or_default())
            .clicked()
            && let Some(i) = sel
        {
            end_rename(app, view, true);
            activate(app, i);
        }
    });
}

/// Destructive-action confirmation, rendered inline (a nested modal inside
/// an egui Window is more machinery than a two-button strip earns).
fn confirm_strip(app: &mut App, view: &mut ViewState, ui: &mut egui::Ui, busy: bool) {
    let Some(i) = view.confirm_delete else { return };
    let name = app.settings.profiles[i].name.clone();
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(format!("Delete profile \"{name}\"?"));
        // Re-checked here: the user may have connected while the strip sat
        // open, and the guard must hold at the moment of the click.
        if ui
            .add_enabled(!busy, egui::Button::new("Delete"))
            .on_disabled_hover_text("Disconnect before deleting profiles")
            .clicked()
        {
            view.confirm_delete = None;
            end_rename(app, view, true);
            let active_before = app.settings.active_profile.clone();
            if profiles::remove(&mut app.settings, i) {
                view.selected =
                    selection_after_remove(view.selected, i, app.settings.profiles.len());
                app.hub.event(format!("Profile \"{name}\" deleted"));
                if app.settings.active_profile != active_before {
                    app.on_profile_switched();
                }
                app.save_settings();
            }
        }
        if ui.button("Cancel").clicked() {
            view.confirm_delete = None;
        }
    });
}

fn begin_rename(view: &mut ViewState, i: usize, current: &str) {
    view.renaming = Some(i);
    view.rename_buf = current.to_string();
    view.rename_focus_pending = true;
    view.confirm_delete = None;
}

/// Leave rename mode; `commit` writes the buffer through the CRUD core
/// (which enforces non-blank and unique names). A committed rename that
/// changes nothing skips the save so the event log is not spammed.
fn end_rename(app: &mut App, view: &mut ViewState, commit: bool) {
    let Some(i) = view.renaming.take() else {
        return;
    };
    let buf = std::mem::take(&mut view.rename_buf);
    view.rename_focus_pending = false;
    if commit
        && app
            .settings
            .profiles
            .get(i)
            .is_some_and(|p| p.name != buf.trim())
        && profiles::rename(&mut app.settings, i, &buf)
    {
        app.save_settings();
    }
}

/// Switch the active profile via the CRUD core; on a real change, reset the
/// per-caster UI state and persist.
fn activate(app: &mut App, i: usize) {
    if app.ntrip_busy() {
        return;
    }
    if profiles::activate(&mut app.settings, i) {
        app.hub.event(format!(
            "Profile \"{}\" activated",
            app.settings.profiles[i].name
        ));
        app.on_profile_switched();
        app.save_settings();
    }
}

/// Why Delete is disabled right now, or None when allowed. Pure for tests:
/// this predicate IS the cannot-delete-last and no-delete-while-connected
/// contract the window enforces.
fn delete_block_reason(
    profile_count: usize,
    busy: bool,
    have_selection: bool,
) -> Option<&'static str> {
    if !have_selection {
        return Some("Select a profile first");
    }
    if profile_count <= 1 {
        return Some("The last profile cannot be deleted");
    }
    if busy {
        return Some("Disconnect before deleting profiles");
    }
    None
}

/// Why Activate is disabled right now, or None when allowed.
fn activate_block_reason(
    busy: bool,
    have_selection: bool,
    selected_is_active: bool,
) -> Option<&'static str> {
    if !have_selection {
        return Some("Select a profile first");
    }
    if selected_is_active {
        return Some("Already the active profile");
    }
    if busy {
        return Some("Disconnect before switching profiles");
    }
    None
}

/// Where the selection lands after deleting `removed` from a list now
/// `new_len` long: stay on the same list position (the item that slid up),
/// clamped to the end. `new_len` is never 0 here (the CRUD core refuses to
/// delete the last profile) but the function is total anyway.
fn selection_after_remove(
    selected: Option<usize>,
    removed: usize,
    new_len: usize,
) -> Option<usize> {
    let s = selected?;
    if new_len == 0 {
        return None;
    }
    let s = if s > removed { s - 1 } else { s };
    Some(s.min(new_len - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_guard_covers_selection_last_profile_and_connection() {
        assert!(
            delete_block_reason(3, false, false).is_some(),
            "no selection"
        );
        assert_eq!(
            delete_block_reason(1, false, true),
            Some("The last profile cannot be deleted")
        );
        assert_eq!(
            delete_block_reason(3, true, true),
            Some("Disconnect before deleting profiles")
        );
        assert_eq!(delete_block_reason(2, false, true), None);
        // Last-profile beats busy: the reason shown is the permanent one.
        assert_eq!(
            delete_block_reason(1, true, true),
            Some("The last profile cannot be deleted")
        );
    }

    #[test]
    fn activate_guard_covers_selection_identity_and_connection() {
        assert!(activate_block_reason(false, false, false).is_some());
        assert_eq!(
            activate_block_reason(false, true, true),
            Some("Already the active profile")
        );
        assert_eq!(
            activate_block_reason(true, true, false),
            Some("Disconnect before switching profiles")
        );
        assert_eq!(activate_block_reason(false, true, false), None);
    }

    #[test]
    fn selection_survives_removals_sensibly() {
        // Deleting the selected middle row: stay on the row that slid up.
        assert_eq!(selection_after_remove(Some(1), 1, 2), Some(1));
        // Deleting the selected last row: clamp to the new end.
        assert_eq!(selection_after_remove(Some(2), 2, 2), Some(1));
        // Deleting above the selection: selection follows its item down.
        assert_eq!(selection_after_remove(Some(2), 0, 2), Some(1));
        // Deleting below the selection: selection keeps its item.
        assert_eq!(selection_after_remove(Some(0), 2, 2), Some(0));
        // No selection stays no selection.
        assert_eq!(selection_after_remove(None, 0, 2), None);
        // Total even for the impossible empty list.
        assert_eq!(selection_after_remove(Some(0), 0, 0), None);
    }
}
