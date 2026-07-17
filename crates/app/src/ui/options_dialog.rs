//! Options dialog: display slots, audio alert, log toggles, update checks.

use crate::logging::LogCmd;
use crate::settings::{CheckUpdates, DisplayId};
use crate::ui::App;

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.show_options {
        return;
    }
    let mut open = app.show_options;
    egui::Window::new("Options")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("options-grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Center readout");
                    display_combo(ui, "opt-center", &mut app.settings.display.center);
                    ui.end_row();

                    ui.label("Right readout");
                    display_combo(ui, "opt-right", &mut app.settings.display.right);
                    ui.end_row();

                    ui.label("Audio alert (.wav)");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut app.settings.app.audio_alert_file)
                                .desired_width(180.0)
                                .hint_text("empty = silent"),
                        );
                        if ui.button("Test").clicked() {
                            match crate::audio::play_wav(&app.settings.app.audio_alert_file) {
                                Ok(()) => {}
                                Err(e) => app.hub.event(format!("Audio test failed: {e}")),
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Log files");
                    ui.vertical(|ui| {
                        if ui
                            .checkbox(
                                &mut app.settings.app.write_event_log,
                                "Write events to Logs\\YYYYMMDD.txt",
                            )
                            .changed()
                        {
                            let _ = app
                                .hub
                                .log_sender()
                                .send(LogCmd::SetEventLog(app.settings.app.write_event_log));
                        }
                        if ui
                            .checkbox(
                                &mut app.settings.app.write_nmea_log,
                                "Write NMEA to NMEA\\YYYYMMDD.txt",
                            )
                            .changed()
                        {
                            let _ = app
                                .hub
                                .log_sender()
                                .send(LogCmd::SetNmeaLog(app.settings.app.write_nmea_log));
                        }
                        // Unlike the two live log toggles above, capture is
                        // bound into the worker job at connect time
                        // (NtripJob.capture), so a change here follows the
                        // existing settings-apply-at-open-time pattern.
                        ui.checkbox(
                            &mut app.settings.app.capture_corrections,
                            "Capture corrections to Captures\\*.rtcm",
                        )
                        .on_hover_text(
                            "Raw correction stream for offline analysis; \
takes effect on the next connect",
                        );
                    });
                    ui.end_row();

                    ui.label("Auto-reconnect");
                    ui.checkbox(
                        &mut app.settings.app.auto_reconnect,
                        "Retry dropped streams",
                    );
                    ui.end_row();

                    ui.label("Check for updates");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("opt-updates")
                            .width(90.0)
                            .selected_text(app.settings.app.check_updates.as_str())
                            .show_ui(ui, |ui| {
                                for c in CheckUpdates::ALL {
                                    ui.selectable_value(
                                        &mut app.settings.app.check_updates,
                                        *c,
                                        c.as_str(),
                                    );
                                }
                            });
                        if ui
                            .button("Check now")
                            .on_hover_text("Opens the releases page in your browser")
                            .clicked()
                        {
                            app.check_updates_now();
                        }
                    });
                    ui.end_row();

                    ui.label("Last checked");
                    let last = if app.settings.state.last_update_check.is_empty() {
                        "never"
                    } else {
                        app.settings.state.last_update_check.as_str()
                    };
                    ui.label(last);
                    ui.end_row();
                });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Settings persist on [Save] and on exit.")
                    .small()
                    // Weak resolves to INK_SECONDARY via the theme's
                    // weak_text_color: readable hint text, AA on the paper.
                    .weak(),
            );
        });
    app.show_options = open;
}

fn display_combo(ui: &mut egui::Ui, id: &str, choice: &mut DisplayId) {
    egui::ComboBox::from_id_salt(id)
        .width(170.0)
        .selected_text(choice.label())
        .show_ui(ui, |ui| {
            for d in DisplayId::ALL {
                ui.selectable_value(choice, *d, d.label());
            }
        });
}
