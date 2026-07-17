//! GGA position-reporting disclosure, inline in the NTRIP column. This is
//! the One Surface fold of two former floating windows - the "Details"
//! dialog (GGA mode/source, manual position) and the "Location" picker
//! (offline city/ZIP search) - onto the front page, where a caster that
//! demands NMEA GGA can be configured without hunting through a menu.
//!
//! The fold also removed a redundancy: the old picker kept the manual
//! position as validated text fields feeding an `Action` the caller applied,
//! *and* the Details dialog edited the same profile floats through
//! `DragValue`s. With one surface there is one editor - range-clamped
//! `DragValue`s bound straight to the active profile - and the city/ZIP
//! search and "use receiver position" simply write those same two floats.
//! No text intermediate, no `Action` plumbing, no parse/validate step: the
//! widgets cannot produce an out-of-range coordinate.
//!
//! Everything works offline; a missing or corrupt embedded geocoder only
//! disables search (said once in the event log at boot), never the manual
//! editor.

use geodb::GeoHit;

use crate::settings::{GgaMode, GgaSource};
use crate::ui::{App, theme};

/// Most hits shown for one query. Eight rows read at a glance and match the
/// type-ahead budget the plan set for the picker.
const RESULT_LIMIT: usize = 8;

/// Search state for the city/ZIP box, owned by `App` (`gga_view`) so a query
/// survives collapsing and reopening the section. The manual position itself
/// lives on the profile, not here - these are search scratch only.
#[derive(Default)]
pub struct ViewState {
    query: String,
    /// The query `hits` was resolved for; resolving only on change keeps the
    /// per-frame cost a string compare instead of a database scan.
    resolved_for: String,
    hits: Vec<GeoHit>,
    selected: usize,
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    theme::card(ui.style()).show(ui, |ui| {
        let open = app.settings.window.gga_open;
        ui.horizontal(|ui| {
            if ui
                .selectable_label(
                    open,
                    if open {
                        "v GGA position reporting"
                    } else {
                        "> GGA position reporting"
                    },
                )
                .on_hover_text(
                    "Whether and how this client sends its position (NMEA GGA) \
                     up to the caster",
                )
                .clicked()
            {
                app.settings.window.gga_open = !open;
            }
            // Collapsed or open, the current configuration reads on the right:
            // the whole point of the fold is that GGA state is never hidden.
            let p = app.settings.active();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(summary(
                        p.gga_mode,
                        p.gga_source,
                        p.manual_lat,
                        p.manual_lon,
                    ))
                    .small()
                    .weak(),
                );
            });
        });

        if open {
            ui.add_space(2.0);
            body(app, ui);
        }
    });
}

fn body(app: &mut App, ui: &mut egui::Ui) {
    let receiver = app.state.gnss.lat_deg.zip(app.state.gnss.lon_deg);
    // Disjoint field borrows: the profile edits, the search view mutates, the
    // geocoder is read-only. Held simultaneously because they name different
    // fields of App - never routed through a `&mut self` method.
    let db = app.geodb.as_ref();
    let view = &mut app.gga_view;
    let p = app.settings.active_mut();

    egui::Grid::new("gga-grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Send GGA");
            egui::ComboBox::from_id_salt("gga-mode")
                .width(140.0)
                .selected_text(match p.gga_mode {
                    GgaMode::Off => "off",
                    GgaMode::WhenRequired => "when required",
                    GgaMode::Always => "always",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut p.gga_mode, GgaMode::Off, "off");
                    ui.selectable_value(&mut p.gga_mode, GgaMode::WhenRequired, "when required");
                    ui.selectable_value(&mut p.gga_mode, GgaMode::Always, "always");
                });
            ui.end_row();

            ui.label("Position source");
            ui.horizontal(|ui| {
                ui.radio_value(&mut p.gga_source, GgaSource::Receiver, "receiver")
                    .on_hover_text("Pass the receiver's last GGA through verbatim");
                ui.radio_value(&mut p.gga_source, GgaSource::Manual, "manual")
                    .on_hover_text("Fabricate a GGA at the position below");
            });
            ui.end_row();
        });

    // The "when required" caveat only earns its ink while that mode is
    // selected: it explains exactly that option (and its dependence on a
    // downloaded sourcetable), so it is noise next to "off" or "always".
    if p.gga_mode == GgaMode::WhenRequired {
        ui.label(
            egui::RichText::new(
                "when required = only for streams whose sourcetable entry sets \
                 the NMEA flag. With no sourcetable downloaded this sends \
                 nothing - use Get Sourcetable first.",
            )
            .small()
            .weak(),
        );
    }

    // The manual editor and its fillers only make sense for a manual point;
    // in receiver mode the profile floats are dormant, so we show nothing.
    if p.gga_source == GgaSource::Manual {
        ui.add_space(4.0);
        ui.separator();
        manual_editor(ui, p, view, db, receiver);
    }
}

/// Manual-position editor: two range-clamped coordinate fields plus the
/// offline city/ZIP search and the receiver-copy button that fill them. Every
/// path writes `p.manual_lat`/`p.manual_lon` directly.
fn manual_editor(
    ui: &mut egui::Ui,
    p: &mut crate::settings::Profile,
    view: &mut ViewState,
    db: Option<&geodb::GeoDb>,
    receiver: Option<(f64, f64)>,
) {
    egui::Grid::new("gga-manual-grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Latitude");
            ui.add(
                egui::DragValue::new(&mut p.manual_lat)
                    .speed(0.0001)
                    .range(-90.0..=90.0)
                    .fixed_decimals(6),
            );
            ui.end_row();
            ui.label("Longitude");
            ui.add(
                egui::DragValue::new(&mut p.manual_lon)
                    .speed(0.0001)
                    .range(-180.0..=180.0)
                    .fixed_decimals(6),
            );
            ui.end_row();
        });

    ui.add_space(2.0);

    // --- offline city / ZIP search -------------------------------------
    // Arrow keys steer the result selection while the search box is focused;
    // they must be consumed BEFORE the TextEdit sees them, so focus is read
    // from the previous frame via a stable widget id.
    let search_id = egui::Id::new("gga-search");
    let search_focused = ui.ctx().memory(|m| m.has_focus(search_id));
    if search_focused && !view.hits.is_empty() {
        let (up, down) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            )
        });
        if up {
            view.selected = step_selection(view.selected, view.hits.len(), false);
        }
        if down {
            view.selected = step_selection(view.selected, view.hits.len(), true);
        }
    }

    let search = ui.add(
        egui::TextEdit::singleline(&mut view.query)
            .id(search_id)
            .hint_text("Nairobi  |  Portland, OR  |  97201")
            .desired_width(f32::INFINITY),
    );
    if view.query != view.resolved_for {
        view.resolved_for = view.query.clone();
        // resolve() returns hits already ordered by population descending.
        view.hits = match db {
            Some(db) => db.resolve(&view.query, RESULT_LIMIT),
            None => Vec::new(),
        };
        view.selected = 0;
    }
    // Enter surrenders focus in egui; that is the apply gesture for the
    // selected (top by default) hit.
    let mut picked: Option<(f64, f64)> = None;
    if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        picked = hit_coords(&view.hits, view.selected);
    }

    if db.is_none() {
        ui.label(
            egui::RichText::new(
                "Offline location database unavailable (see the event log); \
                 the coordinate fields above still work.",
            )
            .small()
            .weak(),
        );
    } else if view.hits.is_empty() && !view.query.trim().is_empty() {
        ui.label(egui::RichText::new("No matches.").small().weak());
    }

    for i in 0..view.hits.len() {
        let label = row_label(&view.hits[i]);
        if ui.selectable_label(view.selected == i, label).clicked() {
            view.selected = i;
            picked = hit_coords(&view.hits, i);
        }
    }

    ui.add_space(4.0);
    if ui
        .add_enabled(
            receiver.is_some(),
            egui::Button::new("Use receiver position"),
        )
        .on_disabled_hover_text("No position received from the receiver yet")
        .clicked()
        && let Some((la, lo)) = receiver
    {
        picked = Some((la, lo));
    }

    if let Some((lat, lon)) = picked {
        p.manual_lat = lat;
        p.manual_lon = lon;
    }
}

/// One result row: display label plus coordinates at the database's own
/// 1e-4 deg resolution.
fn row_label(hit: &GeoHit) -> String {
    format!("{}  ({:.4}, {:.4})", hit.display, hit.lat, hit.lon)
}

/// Coordinates of the currently selected hit. A stale out-of-range selection
/// (the list shrank since the arrows last moved) clamps to the last row
/// rather than dropping the gesture; no hits means nothing to apply.
fn hit_coords(hits: &[GeoHit], selected: usize) -> Option<(f64, f64)> {
    let hit = hits.get(selected.min(hits.len().checked_sub(1)?))?;
    Some((hit.lat, hit.lon))
}

/// Arrow-key selection step, clamped at both ends (no wrap - matching how
/// every native list widget behaves).
fn step_selection(selected: usize, len: usize, down: bool) -> usize {
    if len == 0 {
        0
    } else if down {
        (selected + 1).min(len - 1)
    } else {
        selected.saturating_sub(1)
    }
}

/// One-line collapsed summary of the profile's GGA configuration, so the
/// disclosure communicates its state without being opened. "off" says it all
/// on its own - source and position are moot when nothing is sent.
fn summary(mode: GgaMode, source: GgaSource, lat: f64, lon: f64) -> String {
    let mode_txt = match mode {
        GgaMode::Off => return "off".to_string(),
        GgaMode::WhenRequired => "when required",
        GgaMode::Always => "always",
    };
    let src = match source {
        GgaSource::Receiver => "receiver".to_string(),
        GgaSource::Manual => format!("manual {lat:.5}, {lon:.5}"),
    };
    format!("{mode_txt}, {src}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stubbed resolve result standing in for `GeoDb::resolve` output.
    fn stub_hits() -> Vec<GeoHit> {
        vec![
            GeoHit {
                display: "Portland, OR, US".to_string(),
                lat: 45.5202,
                lon: -122.6742,
                population: 652_503,
            },
            GeoHit {
                display: "Portland, ME, US".to_string(),
                lat: 43.6591,
                lon: -70.2568,
                population: 66_881,
            },
        ]
    }

    #[test]
    fn selection_maps_to_coordinates() {
        let hits = stub_hits();
        assert_eq!(hit_coords(&hits, 0), Some((45.5202, -122.6742)));
        assert_eq!(hit_coords(&hits, 1), Some((43.6591, -70.2568)));
        // A stale selection beyond the list clamps to the last row.
        assert_eq!(hit_coords(&hits, 7), hit_coords(&hits, 1));
        // No hits, nothing to apply - Enter on an unmatched query is a no-op.
        assert_eq!(hit_coords(&[], 0), None);
        assert_eq!(hit_coords(&[], 5), None);
    }

    #[test]
    fn selection_stepping_clamps_at_both_ends() {
        assert_eq!(step_selection(0, 3, true), 1);
        assert_eq!(step_selection(1, 3, true), 2);
        assert_eq!(step_selection(2, 3, true), 2, "clamps at the bottom");
        assert_eq!(step_selection(2, 3, false), 1);
        assert_eq!(step_selection(0, 3, false), 0, "clamps at the top");
        assert_eq!(step_selection(0, 0, true), 0, "empty list is safe");
        assert_eq!(step_selection(5, 0, false), 0);
    }

    #[test]
    fn row_label_shows_display_and_coordinates() {
        let hits = stub_hits();
        assert_eq!(
            row_label(&hits[0]),
            "Portland, OR, US  (45.5202, -122.6742)"
        );
        let nairobi = GeoHit {
            display: "Nairobi, KE".to_string(),
            lat: -1.2833,
            lon: 36.8167,
            population: 2_750_547,
        };
        assert_eq!(row_label(&nairobi), "Nairobi, KE  (-1.2833, 36.8167)");
    }

    /// The collapsed summary reflects mode, source, and - for a manual point -
    /// the coordinates, so the section's state reads without opening it.
    #[test]
    fn summary_reflects_configuration() {
        assert_eq!(
            summary(GgaMode::Off, GgaSource::Manual, 45.0, -122.0),
            "off"
        );
        assert_eq!(
            summary(GgaMode::WhenRequired, GgaSource::Receiver, 0.0, 0.0),
            "when required, receiver"
        );
        assert_eq!(
            summary(GgaMode::Always, GgaSource::Manual, 45.52021, -122.67419),
            "always, manual 45.52021, -122.67419"
        );
    }

    /// The exact query path the search drives, against the real embedded
    /// database: the plan's M3 acceptance queries resolve, respect the limit
    /// and the population ordering, and every hit yields coordinates the
    /// range-clamped fields accept unchanged (so a hit round-trips).
    #[test]
    fn embedded_db_answers_search_queries() {
        let db = geodb::GeoDb::embedded().expect("embedded db validates");
        let portland = db.resolve("portland, or", RESULT_LIMIT);
        assert!(!portland.is_empty());
        assert_eq!(portland[0].display, "Portland, OR, US");
        let nairobi = db.resolve("nairobi", RESULT_LIMIT);
        assert!(!nairobi.is_empty());
        assert!(
            nairobi
                .windows(2)
                .all(|w| w[0].population >= w[1].population),
            "rows must arrive ordered by population descending"
        );
        let zip = db.resolve("97201", RESULT_LIMIT);
        assert_eq!(zip.len(), 1);
        assert_eq!(zip[0].display, "ZIP 97201");
        for hits in [&portland, &nairobi, &zip] {
            assert!(hits.len() <= RESULT_LIMIT);
            for h in hits.iter() {
                let (lat, lon) = hit_coords(std::slice::from_ref(h), 0).expect("hit has coords");
                assert!((-90.0..=90.0).contains(&lat), "lat in range: {lat}");
                assert!((-180.0..=180.0).contains(&lon), "lon in range: {lon}");
            }
        }
    }
}
