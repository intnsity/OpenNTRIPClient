//! "Receiver (Serial)" group: port selection, framing, NovAtel auto-config,
//! connect/disconnect, status.

use crate::bus::SerialStatus;
use crate::settings::{NovatelFormat, ParityCfg};
use crate::ui::{App, theme};

const BAUDS: [u32; 7] = [2_400, 4_800, 9_600, 19_200, 38_400, 57_600, 115_200];

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    theme::card(ui.style()).show(ui, |ui| {
        ui.set_min_height(150.0);
        ui.strong("Receiver (Serial)");
        ui.add_space(2.0);

        let connected = app.serial.is_some();
        // Settings apply at open time; freezing them while connected makes
        // that visible instead of silently ignoring edits.
        ui.add_enabled_ui(!connected, |ui| {
            egui::Grid::new("serial-grid")
                .num_columns(2)
                .spacing([6.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Port");
                    ui.horizontal(|ui| {
                        port_combo(app, ui);
                        if ui
                            .button("Refresh")
                            .on_hover_text("Rescan serial ports")
                            .clicked()
                        {
                            app.refresh_ports();
                        }
                    });
                    ui.end_row();

                    ui.label("Baud");
                    egui::ComboBox::from_id_salt("serial-baud")
                        .width(90.0)
                        .selected_text(app.settings.serial.baud.to_string())
                        .show_ui(ui, |ui| {
                            for b in BAUDS {
                                ui.selectable_value(
                                    &mut app.settings.serial.baud,
                                    b,
                                    b.to_string(),
                                );
                            }
                        });
                    ui.end_row();

                    ui.label("Framing");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("serial-databits")
                            .width(48.0)
                            .selected_text(app.settings.serial.data_bits.to_string())
                            .show_ui(ui, |ui| {
                                for b in [7u8, 8] {
                                    ui.selectable_value(
                                        &mut app.settings.serial.data_bits,
                                        b,
                                        b.to_string(),
                                    );
                                }
                            });
                        egui::ComboBox::from_id_salt("serial-parity")
                            .width(64.0)
                            .selected_text(app.settings.serial.parity.as_str())
                            .show_ui(ui, |ui| {
                                for p in ParityCfg::ALL {
                                    ui.selectable_value(
                                        &mut app.settings.serial.parity,
                                        *p,
                                        p.as_str(),
                                    );
                                }
                            });
                        egui::ComboBox::from_id_salt("serial-stopbits")
                            .width(48.0)
                            .selected_text(app.settings.serial.stop_bits.to_string())
                            .show_ui(ui, |ui| {
                                for b in [1u8, 2] {
                                    ui.selectable_value(
                                        &mut app.settings.serial.stop_bits,
                                        b,
                                        b.to_string(),
                                    );
                                }
                            });
                    });
                    ui.end_row();
                });

            ui.checkbox(
                &mut app.settings.serial.novatel_autoconfig,
                "NovAtel auto-config",
            );
            ui.add_enabled_ui(app.settings.serial.novatel_autoconfig, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Format");
                    egui::ComboBox::from_id_salt("novatel-format")
                        .width(90.0)
                        .selected_text(app.settings.serial.novatel_format.as_str())
                        .show_ui(ui, |ui| {
                            for f in NovatelFormat::ALL {
                                ui.selectable_value(
                                    &mut app.settings.serial.novatel_format,
                                    *f,
                                    f.as_str(),
                                );
                            }
                        });
                    ui.label("Rate");
                    egui::ComboBox::from_id_salt("novatel-rate")
                        .width(64.0)
                        .selected_text(format!("{} Hz", app.settings.serial.novatel_rate_hz))
                        .show_ui(ui, |ui| {
                            for r in [1u8, 5, 10] {
                                ui.selectable_value(
                                    &mut app.settings.serial.novatel_rate_hz,
                                    r,
                                    format!("{r} Hz"),
                                );
                            }
                        });
                });
            });
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let label = if connected { "Disconnect" } else { "Connect" };
            let can_click = connected || !app.settings.serial.port.trim().is_empty();
            if ui
                .add_enabled(can_click, theme::accent_button(label))
                .clicked()
            {
                app.toggle_serial();
            }
            status_line(app, ui);
        });
    });
}

fn port_combo(app: &mut App, ui: &mut egui::Ui) {
    let current = app.settings.serial.port.clone();
    // The saved port stays selectable even when unplugged - reconnecting the
    // receiver must not require re-picking it.
    let mut items: Vec<(String, String)> = app.ports.clone();
    if !current.is_empty() && !items.iter().any(|(name, _)| *name == current) {
        items.push((current.clone(), "not present".to_string()));
    }
    let selected_label = match items.iter().find(|(name, _)| *name == current) {
        Some((name, label)) if !label.is_empty() => format!("{name} - {label}"),
        _ => current.clone(),
    };
    egui::ComboBox::from_id_salt("serial-port")
        .width(150.0)
        .selected_text(selected_label)
        .show_ui(ui, |ui| {
            if items.is_empty() {
                ui.label("no ports found");
            }
            for (name, label) in &items {
                let text = if label.is_empty() {
                    name.clone()
                } else {
                    format!("{name} - {label}")
                };
                ui.selectable_value(&mut app.settings.serial.port, name.clone(), text);
            }
        });
}

fn status_line(app: &App, ui: &mut egui::Ui) {
    let (text, color) = match &app.state.serial.status {
        SerialStatus::Connected { port, detail } => {
            (format!("Connected: {port} ({detail})"), theme::SUCCESS)
        }
        SerialStatus::Disconnected { reason } if reason.is_empty() => {
            ("Not connected".to_string(), theme::INK_SECONDARY)
        }
        SerialStatus::Disconnected { reason } => (reason.clone(), theme::DANGER),
    };
    let mut line = text;
    if app.state.serial.overruns > 0 {
        line.push_str(&format!("  |  overruns: {}", app.state.serial.overruns));
    }
    ui.label(egui::RichText::new(line).color(color).small());
}
