//! Bottom strip: collapsible elevation chart with its controls and stats,
//! plus the RX byte-rate readout on the same row.

use egui_plot::{Line, Plot, PlotPoints};

use crate::ui::{App, theme};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let open = app.settings.window.graph_open;
        if ui
            .selectable_label(
                open,
                if open {
                    "v Elevation graph"
                } else {
                    "> Elevation graph"
                },
            )
            .clicked()
        {
            app.settings.window.graph_open = !open;
        }
        let recording = app.state.chart.recording;
        if ui
            .button(if recording { "Pause" } else { "Start" })
            .clicked()
        {
            app.state.chart.recording = !recording;
        }
        if ui.button("Reset").clicked() {
            app.state.chart.series.clear();
        }
        ui.add_space(8.0);
        let s = &app.state.chart.series;
        let fmt = |v: Option<f64>| v.map_or_else(|| "-".to_string(), |x| format!("{x:.1}"));
        ui.label(
            egui::RichText::new(format!(
                "cur {}  min {}  max {}  range {} m",
                fmt(s.current),
                fmt(s.min),
                fmt(s.max),
                fmt(s.range())
            ))
            .small()
            .color(theme::INK_SECONDARY),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "RX {:.1} kB/s   {}",
                    app.rate_kbps,
                    fmt_total(app.state.ntrip.total_bytes)
                ))
                .monospace(),
            );
        });
    });

    if app.settings.window.graph_open {
        let points: PlotPoints = app.state.chart.series.points().to_vec().into();
        Plot::new("elevation-plot")
            .height(150.0)
            .allow_scroll(false)
            .x_axis_label("seconds")
            .y_axis_label("m")
            .show(ui, |plot| {
                // The one data series in the app rides the interactive
                // cobalt over the recessed paper well.
                plot.line(Line::new("Elevation", points).color(theme::ACCENT));
            });
    }
    ui.add_space(2.0);
}

/// Cumulative RX total for the bottom strip. Below 1 MB it reads in kB so
/// the counter visibly ticks at real correction rates (a 0.5-1 kB/s stream
/// would sit on "0.01 MB" for minutes); above, the familiar MB reading.
fn fmt_total(bytes: u64) -> String {
    if bytes < 1_000_000 {
        format!("{:.1} kB", bytes as f64 / 1e3)
    } else {
        format!("{:.2} MB", bytes as f64 / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::fmt_total;

    /// The kB/MB switchover: per-second visible motion at stream rates,
    /// MB once totals are MB-sized.
    #[test]
    fn total_reads_in_kb_below_one_mb() {
        assert_eq!(fmt_total(0), "0.0 kB");
        assert_eq!(fmt_total(14_237), "14.2 kB");
        assert_eq!(fmt_total(999_949), "999.9 kB");
        assert_eq!(fmt_total(1_000_000), "1.00 MB");
        assert_eq!(fmt_total(123_456_789), "123.46 MB");
    }
}
