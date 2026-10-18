//! The receiver card (setup step 2, optional): port selection, the "Port
//! settings" disclosure (baud, framing, NovAtel auto-config), connect/
//! disconnect, status.

use crate::bus::SerialStatus;
use crate::settings::{NovatelFormat, ParityCfg, SerialCfg};
use crate::ui::{App, theme};

const BAUDS: [u32; 7] = [2_400, 4_800, 9_600, 19_200, 38_400, 57_600, 115_200];

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    theme::card(ui.style()).show(ui, |ui| {
        // Cards fill their column so the two steps read as one row.
        ui.set_width(ui.available_width());
        theme::section_heading(ui, "2", "Receiver", true);
        ui.label(theme::caption(
            "Forwards corrections to a GNSS receiver on a COM port. \
                 Not needed to test a caster.",
        ));
        ui.add_space(2.0);

        let connected = app.serial.is_some();
        // Settings apply at open time; freezing them while connected makes
        // that visible instead of silently ignoring edits.
        ui.add_enabled_ui(!connected, |ui| {
            ui.horizontal(|ui| {
                ui.label("Port");
                port_combo(app, ui);
                if ui
                    .button("Refresh")
                    .on_hover_text("Rescan serial ports")
                    .clicked()
                {
                    app.refresh_ports();
                }
            });
        });

        // The toggle stays live while connected: the values are frozen, but
        // reading them is still allowed.
        let summary = port_settings_summary(&app.settings.serial);
        theme::disclosure(
            ui,
            &mut app.settings.window.serial_open,
            "Port settings",
            &summary,
            theme::ink_secondary(),
        );
        if app.settings.window.serial_open {
            ui.add_enabled_ui(!connected, |ui| port_settings(&mut app.settings.serial, ui));
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            // Secondary weight on purpose: the caster card owns the one
            // solid primary, since a receiver is optional.
            let label = if connected {
                "Disconnect receiver"
            } else {
                "Connect receiver"
            };
            let can_click = connected || !app.settings.serial.port.trim().is_empty();
            if ui
                .add_enabled(can_click, egui::Button::new(label))
                .clicked()
            {
                app.toggle_serial();
            }
            status_line(app, ui);
        });
    });
}

/// Baud, framing and NovAtel auto-config: set once per receiver, so they
/// live behind the disclosure.
fn port_settings(s: &mut SerialCfg, ui: &mut egui::Ui) {
    egui::Grid::new("serial-grid")
        .num_columns(2)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            ui.label("Baud");
            egui::ComboBox::from_id_salt("serial-baud")
                .width(90.0)
                .selected_text(s.baud.to_string())
                .show_ui(ui, |ui| {
                    for b in BAUDS {
                        ui.selectable_value(&mut s.baud, b, b.to_string());
                    }
                });
            ui.end_row();

            ui.label("Framing");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("serial-databits")
                    .width(48.0)
                    .selected_text(s.data_bits.to_string())
                    .show_ui(ui, |ui| {
                        for b in [7u8, 8] {
                            ui.selectable_value(&mut s.data_bits, b, b.to_string());
                        }
                    });
                egui::ComboBox::from_id_salt("serial-parity")
                    .width(64.0)
                    .selected_text(s.parity.as_str())
                    .show_ui(ui, |ui| {
                        for p in ParityCfg::ALL {
                            ui.selectable_value(&mut s.parity, *p, p.as_str());
                        }
                    });
                egui::ComboBox::from_id_salt("serial-stopbits")
                    .width(48.0)
                    .selected_text(s.stop_bits.to_string())
                    .show_ui(ui, |ui| {
                        for b in [1u8, 2] {
                            ui.selectable_value(&mut s.stop_bits, b, b.to_string());
                        }
                    });
            });
            ui.end_row();
        });

    ui.checkbox(&mut s.novatel_autoconfig, "NovAtel auto-config");
    ui.add_enabled_ui(s.novatel_autoconfig, |ui| {
        ui.horizontal(|ui| {
            ui.label("Format");
            egui::ComboBox::from_id_salt("novatel-format")
                .width(90.0)
                .selected_text(s.novatel_format.as_str())
                .show_ui(ui, |ui| {
                    for f in NovatelFormat::ALL {
                        ui.selectable_value(&mut s.novatel_format, *f, f.as_str());
                    }
                });
            ui.label("Rate");
            egui::ComboBox::from_id_salt("novatel-rate")
                .width(64.0)
                .selected_text(format!("{} Hz", s.novatel_rate_hz))
                .show_ui(ui, |ui| {
                    for r in [1u8, 5, 10] {
                        ui.selectable_value(&mut s.novatel_rate_hz, r, format!("{r} Hz"));
                    }
                });
        });
    });
}

/// The collapsed disclosure's one-line statement, e.g. "115200 8-N-1,
/// NovAtel off". Framing uses the conventional data-parity-stop notation.
fn port_settings_summary(s: &SerialCfg) -> String {
    let parity = match s.parity {
        ParityCfg::None => 'N',
        ParityCfg::Even => 'E',
        ParityCfg::Odd => 'O',
    };
    let novatel = if s.novatel_autoconfig {
        format!(
            "NovAtel {} {} Hz",
            s.novatel_format.as_str(),
            s.novatel_rate_hz
        )
    } else {
        "NovAtel off".to_string()
    };
    format!(
        "{} {}-{parity}-{}, {novatel}",
        s.baud, s.data_bits, s.stop_bits
    )
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
            (format!("Connected: {port} ({detail})"), theme::success())
        }
        SerialStatus::Disconnected { reason } if reason.is_empty() => {
            ("Not connected".to_string(), theme::ink_secondary())
        }
        SerialStatus::Disconnected { reason } => (reason.clone(), theme::danger()),
    };
    let mut line = text;
    if app.state.serial.overruns > 0 {
        line.push_str(&format!("  |  overruns: {}", app.state.serial.overruns));
    }
    ui.label(egui::RichText::new(line).color(color).small());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_settings_summary_states_the_configuration() {
        let mut s = SerialCfg::default();
        assert_eq!(port_settings_summary(&s), "115200 8-N-1, NovAtel off");
        s.baud = 9_600;
        s.data_bits = 7;
        s.parity = ParityCfg::Even;
        s.stop_bits = 2;
        s.novatel_autoconfig = true;
        s.novatel_rate_hz = 10;
        assert_eq!(
            port_settings_summary(&s),
            "9600 7-E-2, NovAtel rtcmv3 10 Hz"
        );
    }
}
