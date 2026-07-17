//! Virtualized event-log pane - the Events bottom tab. `show_rows` renders
//! only the visible slice, so a full 10k-line ring stays at 60 fps;
//! stick-to-bottom follows new lines until the user scrolls up, exactly like
//! the original.

use crate::ui::{App, theme};

pub fn tab(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.strong("Event log");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Clear").clicked() {
                app.state.events.clear();
            }
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(app.state.events.join());
            }
        });
    });
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    theme::well_frame(ui.visuals()).show(ui, |ui| {
        // Both axes: lines render unwrapped (extend() keeps show_rows'
        // uniform row height), so without a horizontal scrollbar anything
        // past the pane edge - typically the remedy half of a diagnostic -
        // was silently unreachable except through the Copy button.
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show_rows(ui, row_height, app.state.events.len(), |ui, range| {
                for i in range {
                    if let Some(line) = app.state.events.get(i) {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(line).monospace().color(line_ink(line)),
                            )
                            .extend(),
                        );
                    }
                }
            });
    });
}

/// Log body reads in secondary ink; lines reporting trouble step up to
/// primary ink so a scan of the ring lands on them. Deliberately ink, not a
/// semantic color: the log is diagnostic prose, and a wall of crimson would
/// bury the status strip's real alarms. Events are plain strings by the
/// time they reach the ring, so this is a vocabulary check, not severity
/// metadata.
fn line_ink(line: &str) -> egui::Color32 {
    const TROUBLE: [&str; 8] = [
        "error",
        "fail",
        "could not",
        "denied",
        "unauthorized",
        "warning",
        "overrun",
        "crash",
    ];
    let lower = line.to_ascii_lowercase();
    if TROUBLE.iter().any(|w| lower.contains(w)) {
        theme::INK_PRIMARY
    } else {
        theme::INK_SECONDARY
    }
}

#[cfg(test)]
mod tests {
    use super::{line_ink, theme};

    #[test]
    fn trouble_lines_step_up_to_primary_ink() {
        for line in [
            "12:00:01 Connection FAILED: connection refused",
            "12:00:02 Could not save settings: access denied",
            "12:00:03 Serial overrun: 12 blocks dropped",
            "12:00:04 Authorization error (401 Unauthorized)",
        ] {
            assert_eq!(line_ink(line), theme::INK_PRIMARY, "{line}");
        }
    }

    #[test]
    fn routine_lines_stay_secondary() {
        for line in [
            "12:00:00 Open NTRIP Client v0.3.0 started",
            "12:00:05 Connected to caster",
            "12:00:06 Sourcetable received (114 streams)",
            "12:00:07 Settings saved",
        ] {
            assert_eq!(line_ink(line), theme::INK_SECONDARY, "{line}");
        }
    }
}
