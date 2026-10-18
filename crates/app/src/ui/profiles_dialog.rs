//! Profiles Manager window: a thin render layer over the CRUD core in
//! `crate::profiles`, shaped like every profiles manager a user has already
//! met - a list of named entries (name, the caster it points at, an ACTIVE
//! badge), a column of plainly clickable verbs beside it, and ONE primary
//! action, "Use profile", that makes the selected entry active.
//!
//! Interaction contract (docs/user-guide.md "Dialogs" mirrors it):
//! - click selects; "Use profile", Enter, or double-click activates - the
//!   button is the discoverable path, the other two are its accelerators;
//! - Up/Down move the selection, Alt+Up/Down move the profile, F2 renames,
//!   Delete asks to delete; right-click a row for the same verbs;
//! - rename is inline and needs an explicit commit: Enter or OK saves the
//!   name, Esc or Cancel abandons it, and any other action (another row,
//!   another verb) abandons it too. Losing focus alone never commits;
//! - a typed name another profile already has is refused in place, live,
//!   never silently suffixed (`profiles::check_name`);
//! - no profile is ever created under a made-up name: New..., Duplicate...
//!   and the profile strip's "Save as profile..." all open the same name
//!   prompt ([`NamePrompt`]), pre-filled with a suggestion drawn from where
//!   the profile points (`profiles::suggest_name`) that the user accepts or
//!   overwrites;
//! - each row says when it differs from settings.toml ("unsaved edits",
//!   "not saved yet"), so Save changes and Save as profile are legible;
//! - delete asks in a modal and says what will become active;
//! - Import... and Export... hand off to `profile_transfer`'s dialogs, which
//!   report back through the notice line;
//! - every disabled verb states why directly beneath itself, and a live
//!   connection is announced at the top of the window.
//!
//! Save model: a profile edit is an ordinary settings edit. It applies at
//! once in memory and lights the shared Save control (`App::save_control`)
//! exactly like a front-page field; nothing here writes settings.toml
//! behind the user's back.
//!
//! Connection safety: while an NTRIP worker is live, the running job was
//! built from the ACTIVE profile, so switching and deleting are refused -
//! both in the verbs' guards and again where commands apply. Renames stay
//! allowed: the worker holds a clone of the job and identifies nothing by
//! profile name.

use egui::RichText;

use crate::profiles;
use crate::ui::profile_transfer::Transfer;
use crate::ui::{App, dialog, theme};

/// Width of the verb column; the list takes the rest.
const VERB_WIDTH: f32 = 150.0;

/// Transient window state. Lives on `App`; `Default` is the reset applied
/// whenever the window closes, so every opening starts on the active
/// profile with nothing half-done.
#[derive(Default)]
pub struct ViewState {
    /// Selected row. None only on the first frame of an opening; `body`
    /// seeds it from the active profile, so the verbs always have a target.
    selected: Option<usize>,
    rename: Option<Rename>,
    /// Row awaiting the delete confirmation modal.
    confirm_delete: Option<usize>,
    /// One line of feedback about the last action ("Now using ...").
    notice: Option<String>,
    /// The footer's Close button was pressed this frame.
    close: bool,
    /// Scroll the selected row into view on the next frame (keyboard moves
    /// and reorders can take it off-screen).
    scroll_pending: bool,
}

impl ViewState {
    /// Report an action finished outside the window (import, export) the
    /// way its own verbs report: a selection and the notice line.
    pub(super) fn report(&mut self, select: Option<usize>, notice: String) {
        if select.is_some() {
            self.selected = select;
            self.scroll_pending = true;
        }
        self.notice = Some(notice);
    }
}

/// An inline rename in progress. The edit lives in `buf`, so an abandoned
/// rename never touches the settings.
struct Rename {
    index: usize,
    buf: String,
    /// One-shot: focus the field on the frame it appears.
    focus_pending: bool,
}

/// Per-frame facts every list row needs, computed once in `body`.
#[derive(Clone, Copy)]
struct RowFrame {
    sel: usize,
    busy: bool,
    owns_keyboard: bool,
}

/// Everything a frame's widgets and keys can ask for, applied in one place
/// after rendering so guards are checked exactly once, at the moment of
/// the action.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Cmd {
    Select(usize),
    Activate(usize),
    New,
    Duplicate(usize),
    Import,
    Export(usize),
    BeginRename(usize),
    CommitRename,
    CancelRename,
    AskDelete(usize),
    Move { from: usize, to: usize },
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.show_profiles {
        return;
    }
    // The view state moves out for the frame so the dialog closure can
    // borrow `app` and the view independently; `ViewState: Default` makes
    // the take free.
    let mut view = std::mem::take(&mut app.profiles_view);
    // Edits here apply at once (the shared Save control writes them), so
    // nothing is pending and a click outside may close it like About.
    let dismissed = dialog::show(
        ctx,
        "profiles",
        "Profiles",
        dialog::Dismiss::Anywhere,
        |ui, _frame| {
            // A fixed bed: the panels inside lay out against it, and the list
            // scrolls - the dialog never resizes itself frame to frame.
            let size = egui::vec2(620.0, 400.0);
            ui.allocate_ui(size, |ui| {
                ui.set_min_size(size);
                body(app, &mut view, ui);
            });
        },
    );
    delete_modal(app, &mut view, ctx);
    let open = !(view.close || dismissed);
    app.profiles_view = if open { view } else { ViewState::default() };
    app.show_profiles = open;
}

fn body(app: &mut App, view: &mut ViewState, ui: &mut egui::Ui) {
    // The window state outlives list edits made elsewhere (the front-page
    // picker, a reload); clamp every stored index before use so a stale
    // frame can never panic.
    let n = app.settings.profiles.len();
    let sel = view
        .selected
        .filter(|&i| i < n)
        .unwrap_or_else(|| app.settings.active_index());
    view.selected = Some(sel);
    if view.rename.as_ref().is_some_and(|r| r.index >= n) {
        view.rename = None;
    }
    if view.confirm_delete.is_some_and(|i| i >= n) {
        view.confirm_delete = None;
    }

    let busy = app.ntrip_busy();
    let mut cmds = Vec::new();
    // Focus is sampled at the top of App::ui, before any widget ran: a
    // main-window field that just gave up focus on Enter still counts.
    let owns_keyboard =
        app.focus_at_frame_start.is_none() && app.name_prompt.is_none() && app.transfer.is_none();
    let row_frame = RowFrame {
        sel,
        busy,
        owns_keyboard,
    };
    if owns_keyboard {
        keyboard(app, view, ui, sel, busy, &mut cmds);
    }

    // Panels, not hand-computed heights: the window's current size drives
    // the layout (footer and verb column fixed, the list absorbs resizes).
    // Sizing the list from `available_height` instead fed the content size
    // back into the window, which then grew every frame.
    let bare = egui::Frame::new();
    if busy {
        egui::Panel::top("profiles-busy")
            .frame(bare)
            .show_separator_line(false)
            .show(ui, |ui| {
                theme::warning_callout().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(format!(
                            "Connected with \"{}\". Disconnect to switch or delete profiles - adding, duplicating and renaming still work.",
                            app.settings.active_profile
                        ))
                        .color(theme::ink_primary()),
                    );
                });
                ui.add_space(4.0);
            });
    }
    egui::Panel::bottom("profiles-footer")
        .frame(bare)
        .show_separator_line(false)
        .show(ui, |ui| {
            ui.add_space(2.0);
            match &view.notice {
                Some(text) => ui.label(RichText::new(text).color(theme::ink_secondary())),
                None => ui.label(""),
            };
            ui.separator();
            ui.label(
                RichText::new(
                    "Enter or double-click uses a profile; right-click for more. Save changes writes edits.",
                )
                .small()
                .weak(),
            );
            dialog::footer(ui, |ui| {
                if ui.button("Close").clicked() {
                    view.close = true;
                }
                app.save_control(ui);
            });
        });
    egui::Panel::right("profiles-verbs")
        .frame(egui::Frame::new().inner_margin(egui::Margin {
            left: 8,
            ..Default::default()
        }))
        .resizable(false)
        .show_separator_line(false)
        .exact_size(VERB_WIDTH)
        .show(ui, |ui| verbs(app, view, ui, sel, busy, &mut cmds));
    egui::CentralPanel::default().frame(bare).show(ui, |ui| {
        theme::list_bed().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for i in 0..n {
                        row(app, view, ui, i, row_frame, &mut cmds);
                    }
                });
        });
    });

    for cmd in cmds {
        // Indices were captured before any command ran; once one reshapes
        // the list, the rest of this frame's commands could name the wrong
        // profile, so they are dropped (the user acts again on fresh rows).
        let reshapes = matches!(cmd, Cmd::New | Cmd::Duplicate(_) | Cmd::Move { .. });
        apply(app, view, cmd);
        if reshapes {
            break;
        }
    }
}

/// One list row: badge + name over the caster it points at, or the inline
/// rename editor for the row being renamed.
fn row(
    app: &App,
    view: &mut ViewState,
    ui: &mut egui::Ui,
    i: usize,
    frame: RowFrame,
    cmds: &mut Vec<Cmd>,
) {
    let RowFrame {
        sel,
        busy,
        owns_keyboard,
    } = frame;
    let p = &app.settings.profiles[i];
    let is_active = p.name == app.settings.active_profile;
    let selected = i == sel;

    // Background goes UNDER the content, but its size is known only after
    // the content lays out: reserve the paint slot now, fill it after.
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 10,
            right: 6,
            top: 4,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if is_active {
                    theme::badge(ui, "ACTIVE", theme::success(), theme::success_tint());
                }
                if let Some(r) = view.rename.as_mut().filter(|r| r.index == i) {
                    rename_editor(&app.settings, r, owns_keyboard, ui, cmds);
                } else {
                    ui.label(RichText::new(&p.name).strong());
                    if let Some(tag) = app.profile_save_state(&p.name).tag() {
                        ui.label(RichText::new(tag).small().color(theme::warning()));
                    }
                }
            });
            if let Some(r) = view.rename.as_ref().filter(|r| r.index == i) {
                let (text, color) = match profiles::check_name(&app.settings, Some(i), &r.buf) {
                    Ok(_) => (
                        "Enter or OK saves the name - Esc or Cancel keeps the old one.".to_string(),
                        theme::ink_secondary(),
                    ),
                    Err(problem) => (problem.message(&r.buf), theme::danger()),
                };
                ui.label(RichText::new(text).small().color(color));
            }
            ui.label(
                RichText::new(p.summary())
                    .monospace()
                    .color(theme::ink_secondary()),
            );
        });
    let rect = inner.response.rect;

    if view.rename.as_ref().is_some_and(|r| r.index == i) {
        // The editor's own widgets own the pointer; the row is not a target.
        ui.painter()
            .set(bg, theme::list_row_background(rect, true, false));
        return;
    }
    let resp = ui.interact(rect, ui.id().with(("profile-row", i)), egui::Sense::click());
    ui.painter().set(
        bg,
        theme::list_row_background(rect, selected, resp.hovered()),
    );
    if selected && std::mem::take(&mut view.scroll_pending) {
        resp.scroll_to_me(None);
    }
    // Right-click selects too, so the menu's verbs act on the row that is
    // visibly selected.
    if resp.clicked() || resp.secondary_clicked() {
        cmds.push(Cmd::Select(i));
    }
    if resp.double_clicked() {
        cmds.push(Cmd::Activate(i));
    }
    let n = app.settings.profiles.len();
    resp.context_menu(|ui| {
        let can_use = activate_block_reason(busy, is_active).is_none();
        if ui
            .add_enabled(can_use, egui::Button::new("Use profile"))
            .clicked()
        {
            cmds.push(Cmd::Activate(i));
        }
        if ui.button("Rename").clicked() {
            cmds.push(Cmd::BeginRename(i));
        }
        if ui.button("Duplicate...").clicked() {
            cmds.push(Cmd::Duplicate(i));
        }
        if ui.button("Export...").clicked() {
            cmds.push(Cmd::Export(i));
        }
        ui.separator();
        if ui
            .add_enabled(i > 0, egui::Button::new("Move up"))
            .clicked()
        {
            cmds.push(Cmd::Move { from: i, to: i - 1 });
        }
        if ui
            .add_enabled(i + 1 < n, egui::Button::new("Move down"))
            .clicked()
        {
            cmds.push(Cmd::Move { from: i, to: i + 1 });
        }
        ui.separator();
        let can_delete = delete_block_reason(n, busy).is_none();
        if ui
            .add_enabled(can_delete, egui::Button::new("Delete..."))
            .clicked()
        {
            cmds.push(Cmd::AskDelete(i));
        }
    });
}

/// The inline name field with its explicit OK / Cancel. OK is disabled
/// while the name is blank or taken; the reason is shown under the field.
fn rename_editor(
    settings: &crate::settings::Settings,
    r: &mut Rename,
    owns_keyboard: bool,
    ui: &mut egui::Ui,
    cmds: &mut Vec<Cmd>,
) {
    let valid = profiles::check_name(settings, Some(r.index), &r.buf).is_ok();
    let field_w = (ui.available_width() - 120.0).max(80.0);
    let resp = ui.add(egui::TextEdit::singleline(&mut r.buf).desired_width(field_w));
    if r.focus_pending {
        r.focus_pending = false;
        resp.request_focus();
    }
    // egui drops focus on Enter; that combination - and only that - is the
    // keyboard commit. A plain focus loss (clicking elsewhere) does nothing.
    if resp.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)) {
        if valid {
            cmds.push(Cmd::CommitRename);
        } else {
            r.focus_pending = true;
        }
    }
    // Esc cancels when it was meant for this edit: the field had focus
    // (egui drops it on Esc), or the window owns the keyboard. An Esc in a
    // main-window field must not throw this edit away.
    if (resp.lost_focus() || owns_keyboard)
        && ui.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        cmds.push(Cmd::CancelRename);
    }
    if ui.add_enabled(valid, theme::accent_button("OK")).clicked() {
        cmds.push(Cmd::CommitRename);
    }
    if ui.button("Cancel").clicked() {
        cmds.push(Cmd::CancelRename);
    }
}

/// The verb column. Every verb is a real button at full column width; a
/// blocked verb is disabled with its reason printed directly beneath it.
fn verbs(
    app: &App,
    view: &ViewState,
    ui: &mut egui::Ui,
    sel: usize,
    busy: bool,
    cmds: &mut Vec<Cmd>,
) {
    let n = app.settings.profiles.len();
    let selected_is_active = app.settings.profiles[sel].name == app.settings.active_profile;
    if verb(
        ui,
        theme::accent_button("Use profile"),
        activate_block_reason(busy, selected_is_active),
    ) {
        cmds.push(Cmd::Activate(sel));
    }
    ui.add_space(6.0);
    if verb(ui, egui::Button::new("New..."), None) {
        cmds.push(Cmd::New);
    }
    if verb(ui, egui::Button::new("Duplicate..."), None) {
        cmds.push(Cmd::Duplicate(sel));
    }
    let renaming_this = view.rename.as_ref().is_some_and(|r| r.index == sel);
    if verb(
        ui,
        egui::Button::new("Rename"),
        renaming_this.then_some("Editing the name in the list"),
    ) {
        cmds.push(Cmd::BeginRename(sel));
    }
    ui.add_space(6.0);
    if verb(ui, egui::Button::new("Import..."), None) {
        cmds.push(Cmd::Import);
    }
    if verb(ui, egui::Button::new("Export..."), None) {
        cmds.push(Cmd::Export(sel));
    }
    ui.add_space(6.0);
    if verb(
        ui,
        egui::Button::new("Delete..."),
        delete_block_reason(n, busy),
    ) {
        cmds.push(Cmd::AskDelete(sel));
    }
}

fn verb(ui: &mut egui::Ui, button: egui::Button<'_>, block: Option<&str>) -> bool {
    let clicked = ui
        .add_enabled(
            block.is_none(),
            button.min_size(egui::vec2(ui.available_width(), 0.0)),
        )
        .clicked();
    if let Some(why) = block {
        ui.label(theme::caption(why));
    }
    clicked
}

/// List keyboard. The caller only runs this while the window owns the
/// keyboard (last press inside it, nothing focused at frame start); here,
/// additionally, never while a rename or confirmation is in flight.
fn keyboard(
    app: &App,
    view: &ViewState,
    ui: &egui::Ui,
    sel: usize,
    busy: bool,
    cmds: &mut Vec<Cmd>,
) {
    if view.rename.is_some() || view.confirm_delete.is_some() {
        return;
    }
    let n = app.settings.profiles.len();
    use egui::{Key, Modifiers};
    ui.input_mut(|inp| {
        if inp.consume_key(Modifiers::ALT, Key::ArrowUp) && sel > 0 {
            cmds.push(Cmd::Move {
                from: sel,
                to: sel - 1,
            });
        } else if inp.consume_key(Modifiers::ALT, Key::ArrowDown) && sel + 1 < n {
            cmds.push(Cmd::Move {
                from: sel,
                to: sel + 1,
            });
        } else if inp.consume_key(Modifiers::NONE, Key::ArrowUp) {
            cmds.push(Cmd::Select(step_selection(sel, n, -1)));
        } else if inp.consume_key(Modifiers::NONE, Key::ArrowDown) {
            cmds.push(Cmd::Select(step_selection(sel, n, 1)));
        } else if inp.consume_key(Modifiers::NONE, Key::Enter) {
            cmds.push(Cmd::Activate(sel));
        } else if inp.consume_key(Modifiers::NONE, Key::F2) {
            cmds.push(Cmd::BeginRename(sel));
        } else if inp.consume_key(Modifiers::NONE, Key::Delete)
            && delete_block_reason(n, busy).is_none()
        {
            cmds.push(Cmd::AskDelete(sel));
        }
    });
}

/// Delete confirmation in a real modal (the platform idiom: the rest of the
/// app is inert until the user answers). Esc or a click outside cancels.
fn delete_modal(app: &mut App, view: &mut ViewState, ctx: &egui::Context) {
    let Some(i) = view.confirm_delete else {
        return;
    };
    let n = app.settings.profiles.len();
    if i >= n {
        view.confirm_delete = None;
        return;
    }
    let busy = app.ntrip_busy();
    let p = &app.settings.profiles[i];
    let name = p.name.clone();
    // Says what is lost in the user's terms; a profile with no caster has
    // nothing worth itemizing.
    let losing = if p.host.trim().is_empty() {
        "This removes the profile and its settings.".to_string()
    } else {
        format!(
            "This removes the profile: its caster ({}), credentials and GGA settings.",
            p.summary()
        )
    };
    let is_active = name == app.settings.active_profile;
    // Mirrors profiles::remove: deleting the active profile hands "active"
    // to the first remaining one.
    let successor = app
        .settings
        .profiles
        .iter()
        .find(|q| q.name != name)
        .map(|q| q.name.clone())
        .unwrap_or_default();
    let block = delete_block_reason(n, busy);

    let mut confirmed = false;
    let mut cancelled = false;
    let title = format!("Delete \"{name}\"?");
    let dismissed = dialog::show(
        ctx,
        "profile-delete",
        &title,
        dialog::Dismiss::Anywhere,
        |ui, _frame| {
            ui.set_width(380.0);
            ui.label(losing);
            if is_active {
                ui.label(format!(
                    "It is the active profile - \"{successor}\" becomes active."
                ));
            }
            if let Some(why) = block {
                // Re-checked here: the user may have connected while the modal
                // sat open, and the guard must hold at the moment of the click.
                ui.label(RichText::new(why).color(theme::warning()));
            }
            ui.add_space(8.0);
            dialog::footer(ui, |ui| {
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
                if ui
                    .add_enabled(block.is_none(), theme::danger_button("Delete"))
                    .clicked()
                {
                    confirmed = true;
                }
            });
        },
    );
    if cancelled || dismissed {
        view.confirm_delete = None;
        return;
    }
    if !confirmed || app.ntrip_busy() {
        return;
    }
    view.confirm_delete = None;
    let active_before = app.settings.active_profile.clone();
    if profiles::remove(&mut app.settings, i) {
        let new_len = app.settings.profiles.len();
        view.selected = selection_after_remove(view.selected, i, new_len);
        app.hub.event(format!("Profile \"{name}\" deleted"));
        if app.settings.active_profile != active_before {
            app.on_profile_switched();
            view.notice = Some(format!(
                "Deleted \"{name}\" - now using \"{}\".",
                app.settings.active_profile
            ));
        } else {
            view.notice = Some(format!("Deleted \"{name}\"."));
        }
    }
}

fn apply(app: &mut App, view: &mut ViewState, cmd: Cmd) {
    // Any action other than finishing the rename itself abandons it: the
    // edit is explicit-commit, so walking away is a cancel, never a save.
    if !matches!(cmd, Cmd::CommitRename | Cmd::CancelRename) {
        view.rename = None;
    }
    match cmd {
        Cmd::Select(i) => {
            view.selected = Some(i);
            view.scroll_pending = true;
            view.notice = None;
        }
        Cmd::Activate(i) => {
            view.selected = Some(i);
            let is_active = app.settings.profiles[i].name == app.settings.active_profile;
            if let Some(why) = activate_block_reason(app.ntrip_busy(), is_active) {
                view.notice = Some(why.to_string());
            } else if profiles::activate(&mut app.settings, i) {
                let name = app.settings.profiles[i].name.clone();
                app.hub.event(format!("Profile \"{name}\" activated"));
                app.on_profile_switched();
                view.notice = Some(format!("Now using \"{name}\"."));
            }
        }
        Cmd::New => {
            view.notice = None;
            app.name_prompt = Some(NamePrompt::new_profile());
        }
        Cmd::Duplicate(i) => {
            view.selected = Some(i);
            view.notice = None;
            app.name_prompt = Some(NamePrompt::duplicate(app, i));
        }
        Cmd::Import => {
            view.notice = None;
            app.transfer = Some(Transfer::import(app, None));
        }
        Cmd::Export(i) => {
            view.selected = Some(i);
            view.notice = None;
            app.transfer = Some(Transfer::export(&app.settings.profiles[i].name));
        }
        Cmd::BeginRename(i) => {
            view.selected = Some(i);
            view.notice = None;
            begin_rename(app, view, i);
        }
        Cmd::CommitRename => {
            let Some(r) = view.rename.take() else { return };
            match profiles::check_name(&app.settings, Some(r.index), &r.buf) {
                Ok(name) => {
                    let old = app.settings.profiles[r.index].name.clone();
                    if old != name && profiles::rename(&mut app.settings, r.index, &name) {
                        view.notice = Some(format!("Renamed \"{old}\" to \"{name}\"."));
                    }
                }
                // Defensive: OK/Enter are gated on validity, but a stale
                // frame must not drop the user's text.
                Err(_) => {
                    view.rename = Some(Rename {
                        focus_pending: true,
                        ..r
                    })
                }
            }
        }
        Cmd::CancelRename => view.rename = None,
        Cmd::AskDelete(i) => {
            view.selected = Some(i);
            if delete_block_reason(app.settings.profiles.len(), app.ntrip_busy()).is_none() {
                view.confirm_delete = Some(i);
            }
        }
        Cmd::Move { from, to } => {
            if profiles::move_profile(&mut app.settings, from, to) {
                view.selected = Some(to);
                view.scroll_pending = true;
            }
        }
    }
}

fn begin_rename(app: &App, view: &mut ViewState, i: usize) {
    view.rename = Some(Rename {
        index: i,
        buf: app.settings.profiles[i].name.clone(),
        focus_pending: true,
    });
    view.confirm_delete = None;
}

/// What the name prompt is naming.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Purpose {
    /// The main window's current settings become a new, active profile.
    SaveAs,
    /// An empty profile, filled in afterwards on the main window.
    New,
    /// A copy of profile `usize`.
    Duplicate(usize),
}

/// The one modal every new profile is named in, before it exists. Lives on
/// `App` (the profile strip opens it too); `show_name_prompt` renders it.
pub struct NamePrompt {
    purpose: Purpose,
    buf: String,
}

impl NamePrompt {
    /// "Save as profile...": suggest a name from what the settings point at,
    /// using the sourcetable's description of the stream when one is loaded.
    pub fn save_as(app: &App) -> Self {
        let p = app.settings.active();
        let ident = app
            .state
            .str_record(p.host.trim(), p.port, p.mountpoint.trim())
            .map(|r| r.identifier.as_str());
        let suggestion = profiles::suggest_name(p, ident);
        NamePrompt {
            purpose: Purpose::SaveAs,
            buf: profiles::free_name(&app.settings, &suggestion),
        }
    }

    fn new_profile() -> Self {
        NamePrompt {
            purpose: Purpose::New,
            buf: String::new(),
        }
    }

    fn duplicate(app: &App, i: usize) -> Self {
        let src = &app.settings.profiles[i].name;
        NamePrompt {
            purpose: Purpose::Duplicate(i),
            buf: profiles::free_name(&app.settings, &format!("{src} copy")),
        }
    }
}

/// Render the name prompt, if one is open, and carry out its action.
pub fn show_name_prompt(app: &mut App, ctx: &egui::Context) {
    let Some(mut prompt) = app.name_prompt.take() else {
        return;
    };
    if let Purpose::Duplicate(i) = prompt.purpose
        && i >= app.settings.profiles.len()
    {
        return; // the source vanished under the prompt
    }
    let busy = app.ntrip_busy();
    let active = app.settings.active().clone();
    let (title, action) = match prompt.purpose {
        Purpose::SaveAs => ("Save as profile".to_string(), "Save profile"),
        Purpose::New => ("New profile".to_string(), "Add profile"),
        Purpose::Duplicate(i) => (
            format!("Duplicate \"{}\"", app.settings.profiles[i].name),
            "Duplicate",
        ),
    };
    // Bytes alone are not corrections: an RT27 port streams happily and
    // decodes to nothing, so the reassurance needs real RTCM3 frames.
    let streaming = matches!(app.state.ntrip.status, crate::bus::NtripStatus::Streaming)
        && !app.state.ntrip.rtcm.is_empty();
    let source_state = app.profile_save_state(&active.name);

    let mut confirmed = false;
    let mut cancelled = false;
    // Explicit: a typed name must not vanish on a stray click outside.
    let dismissed = dialog::show(
        ctx,
        "profile-name",
        &title,
        dialog::Dismiss::Explicit,
        |ui, frame| {
            ui.set_width(420.0);
            match prompt.purpose {
                Purpose::SaveAs => {
                    ui.label("Saves the settings on the main window as a new profile and makes it active.");
                    ui.label(
                        RichText::new(active.summary())
                            .monospace()
                            .color(theme::ink_secondary()),
                    );
                    if streaming {
                        ui.label(
                            RichText::new("These settings are streaming corrections right now.")
                                .color(theme::success()),
                        );
                    }
                    match source_state {
                    ProfileSaveState::Edited => ui.label(format!(
                        "\"{}\" keeps its last saved settings; your edits go into the new profile.",
                        active.name
                    )),
                    ProfileSaveState::NotSaved | ProfileSaveState::Saved => ui.label(format!(
                        "\"{}\" stays as it is.",
                        active.name
                    )),
                };
                }
                Purpose::New => {
                    ui.label(if busy {
                    "Adds an empty profile. You are connected, so it is added without switching to it."
                } else {
                    "Adds an empty profile and makes it active - fill in the caster on the main window, Connect to test it, then Save changes."
                });
                }
                Purpose::Duplicate(_) => {
                    ui.label("Copies the caster, credentials and GGA settings under a new name.");
                }
            }
            ui.add_space(8.0);
            ui.label("Name");
            let out = egui::TextEdit::singleline(&mut prompt.buf)
                .hint_text("e.g. Office base, VRS north")
                .desired_width(f32::INFINITY)
                .show(ui);
            // Focus on the SECOND frame: a focus requested while the
            // opening click is still being processed is drawn but receives
            // no typing.
            if frame == 2 {
                out.response.request_focus();
                // Select the suggestion: typing replaces it, Enter accepts it.
                let mut state = out.state;
                let end = egui::text::CCursor::new(prompt.buf.chars().count());
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        end,
                    )));
                state.store(ui.ctx(), out.response.id);
            }
            let check = profiles::check_name(&app.settings, None, &prompt.buf);
            match &check {
                Ok(_) if matches!(prompt.purpose, Purpose::SaveAs) => ui.label(theme::caption(
                    "Also writes settings.toml, like Save changes.",
                )),
                Ok(_) => ui.label(""),
                Err(problem) => ui.label(
                    RichText::new(problem.message(&prompt.buf))
                        .small()
                        .color(theme::danger()),
                ),
            };
            if out.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if check.is_ok() {
                    confirmed = true;
                } else {
                    out.response.request_focus();
                }
            }
            ui.add_space(8.0);
            dialog::footer(ui, |ui| {
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
                if ui
                    .add_enabled(check.is_ok(), theme::accent_button(action))
                    .clicked()
                {
                    confirmed = true;
                }
            });
        },
    );
    if cancelled || (!confirmed && dismissed) {
        return;
    }
    if !confirmed {
        app.name_prompt = Some(prompt);
        return;
    }
    let name = prompt.buf.trim().to_string();
    match prompt.purpose {
        Purpose::SaveAs => {
            if app.save_current_as(&name).is_err() {
                app.name_prompt = Some(prompt);
                return;
            }
            if app.show_profiles {
                app.profiles_view.notice =
                    Some(format!("Saved the current settings as \"{name}\"."));
            }
        }
        Purpose::New => match profiles::add_named(&mut app.settings, &name) {
            Ok(i) => {
                app.profiles_view.selected = Some(i);
                app.profiles_view.scroll_pending = true;
                if !busy && profiles::activate(&mut app.settings, i) {
                    app.on_profile_switched();
                    app.hub
                        .event(format!("Profile \"{name}\" added and activated"));
                    app.profiles_view.notice = Some(format!(
                        "Added \"{name}\" - fill in its caster on the main window."
                    ));
                } else {
                    app.profiles_view.notice = Some(format!("Added \"{name}\"."));
                }
            }
            Err(_) => app.name_prompt = Some(prompt),
        },
        Purpose::Duplicate(i) => match profiles::duplicate_named(&mut app.settings, i, &name) {
            Ok(j) => {
                app.profiles_view.selected = Some(j);
                app.profiles_view.scroll_pending = true;
                app.profiles_view.notice = Some(format!("Added \"{name}\"."));
            }
            Err(_) => app.name_prompt = Some(prompt),
        },
    }
}

/// How a profile stands against settings.toml, for the list's tags and the
/// Save As explanation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSaveState {
    Saved,
    /// Exists in the file, but with different values in memory.
    Edited,
    /// Not in the file at all.
    NotSaved,
}

impl ProfileSaveState {
    fn tag(self) -> Option<&'static str> {
        match self {
            ProfileSaveState::Saved => None,
            ProfileSaveState::Edited => Some("unsaved edits"),
            ProfileSaveState::NotSaved => Some("not saved yet"),
        }
    }
}

/// Why Delete is refused right now, or None when allowed. Pure for tests:
/// this predicate IS the cannot-delete-last and no-delete-while-connected
/// contract the window enforces. (There is always a selection now - the
/// window seeds it from the active profile - so "select a profile first"
/// is no longer a state the user can reach.)
fn delete_block_reason(profile_count: usize, busy: bool) -> Option<&'static str> {
    if profile_count <= 1 {
        return Some("The last profile can't be deleted");
    }
    if busy {
        return Some("Disconnect to delete profiles");
    }
    None
}

/// Why "Use profile" is refused right now, or None when allowed.
fn activate_block_reason(busy: bool, selected_is_active: bool) -> Option<&'static str> {
    if selected_is_active {
        return Some("Already the active profile");
    }
    if busy {
        return Some("Disconnect to switch profiles");
    }
    None
}

/// Arrow-key step through a list of `n` rows, clamped at both ends (no
/// wrap-around: a list that jumps from the bottom to the top surprises).
fn step_selection(sel: usize, n: usize, delta: isize) -> usize {
    if n == 0 {
        return 0;
    }
    sel.saturating_add_signed(delta).min(n - 1)
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
    fn delete_guard_covers_last_profile_and_connection() {
        assert_eq!(
            delete_block_reason(1, false),
            Some("The last profile can't be deleted")
        );
        assert_eq!(
            delete_block_reason(3, true),
            Some("Disconnect to delete profiles")
        );
        assert_eq!(delete_block_reason(2, false), None);
        // Last-profile beats busy: the reason shown is the permanent one.
        assert_eq!(
            delete_block_reason(1, true),
            Some("The last profile can't be deleted")
        );
    }

    #[test]
    fn activate_guard_covers_identity_and_connection() {
        assert_eq!(
            activate_block_reason(false, true),
            Some("Already the active profile")
        );
        assert_eq!(
            activate_block_reason(true, false),
            Some("Disconnect to switch profiles")
        );
        // Already-active beats busy: re-using the live profile is not a
        // switch, so "disconnect" would be a false instruction.
        assert_eq!(
            activate_block_reason(true, true),
            Some("Already the active profile")
        );
        assert_eq!(activate_block_reason(false, false), None);
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

    #[test]
    fn arrow_keys_clamp_without_wrapping() {
        assert_eq!(step_selection(0, 3, 1), 1);
        assert_eq!(step_selection(2, 3, 1), 2, "bottom stays at bottom");
        assert_eq!(step_selection(0, 3, -1), 0, "top stays at top");
        assert_eq!(step_selection(2, 3, -1), 1);
        assert_eq!(step_selection(0, 0, 1), 0, "total on an empty list");
    }
}
