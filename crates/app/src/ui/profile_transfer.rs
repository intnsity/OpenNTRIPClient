//! Export and Import dialogs for `.oncprofile` files, plus drag-and-drop.
//! A thin render layer over `crate::profile_file`: export writes a new file
//! into the Profiles folder and shows it in the file manager; import is a
//! reviewable plan whose result is an ordinary in-memory edit (it lights
//! Save changes and writes nothing by itself).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::RichText;

use crate::paths;
use crate::profile_file::{self, FoundFile, ImportRow};
use crate::settings::{Profile, Settings};
use crate::ui::{App, dialog, theme};

/// The open transfer dialog. Lives on `App`; `show` renders it.
pub enum Transfer {
    Export(Export),
    Import(Import),
}

pub struct Export {
    /// The profile the Profiles window had selected, by identity.
    name: String,
    all: bool,
    include_passwords: bool,
    error: Option<String>,
}

pub struct Import {
    /// Files on offer while none is chosen; searched once per opening.
    found: Vec<FoundFile>,
    chosen: Option<Chosen>,
    error: Option<String>,
}

struct Chosen {
    path: PathBuf,
    rows: Vec<ImportRow>,
}

impl Transfer {
    pub fn export(name: &str) -> Self {
        Transfer::Export(Export {
            name: name.to_string(),
            all: false,
            include_passwords: true,
            error: None,
        })
    }

    /// Import `path`, or - without one, or when it cannot be read - offer
    /// the profile files found in the usual places.
    pub fn import(app: &App, path: Option<PathBuf>) -> Self {
        let mut im = Import {
            found: Vec::new(),
            chosen: None,
            error: None,
        };
        if let Some(path) = path {
            im.load(path, &app.settings);
        }
        if im.chosen.is_none() {
            im.found = profile_file::discover(&app.base);
        }
        Transfer::Import(im)
    }
}

impl Import {
    fn load(&mut self, path: PathBuf, settings: &Settings) {
        match profile_file::read(&path) {
            Ok(profiles) => {
                self.chosen = Some(Chosen {
                    rows: profile_file::plan(profiles, settings),
                    path,
                });
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{}: {e}", file_name(&path))),
        }
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    app.transfer = match app.transfer.take() {
        Some(Transfer::Export(e)) => show_export(app, ctx, e),
        Some(Transfer::Import(im)) => show_import(app, ctx, im),
        None => None,
    };
}

fn show_export(app: &mut App, ctx: &egui::Context, mut e: Export) -> Option<Transfer> {
    let n = app.settings.profiles.len();
    let selected = app
        .settings
        .profiles
        .iter()
        .find(|p| p.name == e.name)
        .cloned();
    let (mut go, mut cancel) = (false, false);
    let dismissed = dialog::show(
        ctx,
        "profile-export",
        "Export profiles",
        dialog::Dismiss::Explicit,
        |ui, _frame| {
            ui.set_width(380.0);
            match &selected {
                Some(p) if n > 1 => {
                    ui.radio_value(&mut e.all, false, format!("\"{}\"", p.name));
                    ui.radio_value(&mut e.all, true, format!("All {n} profiles"));
                }
                Some(p) => {
                    ui.label(format!("\"{}\"", p.name));
                }
                None => {
                    e.all = true;
                    ui.label(format!("All {n} profiles"));
                }
            }
            ui.add_space(6.0);
            ui.checkbox(&mut e.include_passwords, "Include passwords");
            if e.include_passwords {
                ui.label(theme::caption(
                    "Stored as plain text - share only with people you trust.",
                ));
            }
            if let Some(err) = &e.error {
                ui.add_space(4.0);
                ui.label(RichText::new(err).color(theme::danger()));
            }
            ui.add_space(8.0);
            dialog::footer(ui, |ui| {
                cancel = ui.button("Cancel").clicked();
                go = ui.add(theme::accent_button("Export")).clicked();
            });
        },
    );
    if cancel || (dismissed && !go) {
        return None;
    }
    if !go {
        return Some(Transfer::Export(e));
    }
    let list = match selected {
        Some(p) if !e.all => vec![p],
        _ => app.settings.profiles.clone(),
    };
    match profile_file::export(&paths::profiles_dir(&app.base), &list, e.include_passwords) {
        Ok(path) => {
            app.hub.event(format!(
                "Exported {} to {}",
                count(list.len()),
                path.display()
            ));
            if let Err(err) = profile_file::reveal(&path) {
                app.hub
                    .event(format!("Could not open the Profiles folder: {err}"));
            }
            let shown = Path::new("Profiles").join(file_name(&path));
            app.profiles_view
                .report(None, format!("Exported to {}", shown.display()));
            None
        }
        Err(err) => {
            e.error = Some(err);
            Some(Transfer::Export(e))
        }
    }
}

fn show_import(app: &mut App, ctx: &egui::Context, mut im: Import) -> Option<Transfer> {
    let (mut go, mut cancel, mut pick) = (false, false, None);
    let dismissed = dialog::show(
        ctx,
        "profile-import",
        "Import profiles",
        dialog::Dismiss::Explicit,
        |ui, _frame| {
            ui.set_width(460.0);
            if let Some(err) = &im.error {
                ui.label(RichText::new(err).color(theme::danger()));
                ui.add_space(4.0);
            }
            let ready = match &mut im.chosen {
                None => {
                    pick = file_list(ui, &im.found, &app.base);
                    None
                }
                Some(c) => Some(review(ui, &app.settings, app.ntrip_busy(), c)),
            };
            ui.add_space(8.0);
            dialog::footer(ui, |ui| {
                cancel = ui.button("Cancel").clicked();
                if let Some((taken, valid)) = ready {
                    let label = if taken > 0 {
                        format!("Import {taken}")
                    } else {
                        "Import".to_string()
                    };
                    go = ui
                        .add_enabled(taken > 0 && valid, theme::accent_button(label))
                        .clicked();
                }
            });
        },
    );
    if cancel || (dismissed && !go) {
        return None;
    }
    if let Some(path) = pick {
        im.load(path, &app.settings);
    }
    if !go {
        return Some(Transfer::Import(im));
    }
    let Some(c) = &im.chosen else {
        return Some(Transfer::Import(im));
    };
    let active = app.settings.active_profile.clone();
    let replaces_active = c
        .rows
        .iter()
        .any(|r| r.take && r.replace && r.clashes_with_existing && r.profile.name == active);
    let busy = app.ntrip_busy();
    let imported = profile_file::apply_import(&mut app.settings, &c.rows, busy);
    let Some(&first) = imported.first() else {
        return Some(Transfer::Import(im));
    };
    if replaces_active {
        // Same name, new caster: per-caster UI state belongs to the old one.
        app.on_profile_switched();
    }
    let what = count(imported.len());
    app.hub
        .event(format!("Imported {what} from {}", c.path.display()));
    app.show_profiles = true;
    app.profiles_view.report(
        Some(first),
        format!("Imported {what} from {}", file_name(&c.path)),
    );
    None
}

/// The files on offer; returns the one clicked.
fn file_list(ui: &mut egui::Ui, found: &[FoundFile], base: &Path) -> Option<PathBuf> {
    let mut pick = None;
    if found.is_empty() {
        ui.label("No profile files found in:");
        for dir in profile_file::search_dirs(base) {
            ui.label(
                RichText::new(dir.display().to_string())
                    .monospace()
                    .color(theme::ink_secondary()),
            );
        }
    } else {
        ui.label("Choose a file:");
        theme::list_bed().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let now = SystemTime::now();
                    for f in found {
                        if file_row(ui, f, now) {
                            pick = Some(f.path.clone());
                        }
                    }
                });
        });
    }
    ui.add_space(4.0);
    ui.label(theme::caption(
        "Or drop a .oncprofile file onto the window.",
    ));
    pick
}

/// One clickable file: its name over "folder - age". True when clicked.
fn file_row(ui: &mut egui::Ui, f: &FoundFile, now: SystemTime) -> bool {
    let folder = f
        .path
        .parent()
        .and_then(Path::file_name)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let age = super::text::age_label(now.duration_since(f.modified).unwrap_or_default());
    // Background under the content, sized after it lays out.
    let bg = ui.painter().add(egui::Shape::Noop);
    let rect = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(file_name(&f.path)).strong());
            ui.label(theme::caption(format!("{folder} - {age}")));
        })
        .response
        .rect;
    let resp = ui
        .interact(rect, ui.id().with(&f.path), egui::Sense::click())
        .on_hover_text(f.path.display().to_string());
    ui.painter()
        .set(bg, theme::list_row_background(rect, false, resp.hovered()));
    resp.clicked()
}

/// The chosen file's rows. Returns (rows taken, every taken row valid).
fn review(ui: &mut egui::Ui, settings: &Settings, busy: bool, c: &mut Chosen) -> (usize, bool) {
    ui.label(RichText::new(format!("From {}", file_name(&c.path))).color(theme::ink_secondary()));
    // Messages reflect the frame's start (an edit shows next frame, drawn
    // at once); the Import gate below is checked after this frame's edits.
    let problems: Vec<_> = (0..c.rows.len())
        .map(|i| profile_file::row_problem(settings, &c.rows, i, busy))
        .collect();
    theme::list_bed().show(ui, |ui| {
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (row, problem) in c.rows.iter_mut().zip(&problems) {
                    ui.horizontal_top(|ui| {
                        ui.checkbox(&mut row.take, "");
                        ui.vertical(|ui| row_body(ui, row, *problem));
                    });
                    ui.add_space(4.0);
                }
            });
    });
    let taken = c.rows.iter().filter(|r| r.take).count();
    let valid =
        (0..c.rows.len()).all(|i| profile_file::row_problem(settings, &c.rows, i, busy).is_none());
    (taken, valid)
}

fn row_body(ui: &mut egui::Ui, row: &mut ImportRow, problem: Option<crate::profiles::NameProblem>) {
    let replacing = row.replace && row.clashes_with_existing;
    ui.add_enabled_ui(row.take, |ui| {
        if replacing {
            ui.label(RichText::new(&row.profile.name).strong());
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut row.name)
                    .hint_text("Name")
                    .desired_width(260.0),
            );
        }
        ui.label(
            RichText::new(summary(&row.profile))
                .monospace()
                .color(theme::ink_secondary()),
        );
        if row.clashes_with_existing {
            ui.checkbox(
                &mut row.replace,
                format!("Replace existing \"{}\"", row.profile.name),
            );
        }
    });
    if let Some(p) = problem {
        ui.label(
            RichText::new(p.message(row.target_name()))
                .small()
                .color(theme::danger()),
        );
    }
}

/// Where it points, plus the one thing a receiver of a password-less file
/// must still fill in.
fn summary(p: &Profile) -> String {
    let mut s = p.summary();
    if !p.username.is_empty() && p.password.is_empty() {
        s.push_str("  no password");
    }
    s
}

/// Drag and drop: a profile file dropped anywhere opens Import on it, and
/// a file held over the window says so. Runs every frame, before `show`.
pub fn handle_drops(app: &mut App, ctx: &egui::Context) {
    let (hovering, dropped) = ctx.input(|i| {
        let hovering = i
            .raw
            .hovered_files
            .iter()
            .any(|f| f.path.as_deref().is_none_or(importable));
        let dropped = i
            .raw
            .dropped_files
            .iter()
            .filter_map(|f| f.path.clone())
            .find(|p| importable(p));
        (hovering, dropped)
    });
    if hovering {
        drop_hint(ctx);
    }
    if let Some(path) = dropped {
        app.transfer = Some(Transfer::import(app, Some(path)));
    }
}

/// .oncprofile, or a settings.toml from another install.
fn importable(path: &Path) -> bool {
    profile_file::has_extension(path, profile_file::EXTENSION)
        || profile_file::has_extension(path, "toml")
}

/// A veil over the whole window with one line on it, above any dialog.
fn drop_hint(ctx: &egui::Context) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("drop-hint"),
    ));
    let rect = ctx.content_rect();
    painter.rect_filled(rect, 0.0, theme::paper_0().gamma_multiply(0.85));
    painter.rect_stroke(
        rect.shrink(10.0),
        8.0,
        egui::Stroke::new(2.0, theme::accent()),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Drop to import profiles",
        egui::FontId::proportional(20.0),
        theme::ink_primary(),
    );
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn count(n: usize) -> String {
    if n == 1 {
        "1 profile".to_string()
    } else {
        format!("{n} profiles")
    }
}
