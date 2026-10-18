//! Bottom strip: collapsible elevation chart with its controls and stats,
//! plus the RX byte-rate readout on the same row.

use egui_plot::{Line, Plot, PlotPoints};

use crate::ui::{App, theme};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        // No summary: the stats caption on this same row already states the
        // collapsed graph, and repeating it would only crowd the controls.
        theme::disclosure(
            ui,
            &mut app.settings.window.graph_open,
            "Elevation graph (receiver)",
            "",
            theme::ink_secondary(),
        );
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
        // Monospace: four values ticking at receiver rate would otherwise
        // jitter the caption's width every update.
        ui.label(
            egui::RichText::new(elevation_stats(s.current, s.min, s.max, s.range()))
                .small()
                .monospace()
                .color(theme::ink_secondary()),
        );
    });

    if app.settings.window.graph_open {
        let points: PlotPoints = app.state.chart.series.points().to_vec().into();
        Plot::new("elevation-plot")
            // Scales with the window so the graph never swallows the tabs.
            .height((ui.ctx().content_rect().height() * 0.18).clamp(90.0, 150.0))
            .allow_scroll(false)
            .x_axis_label("seconds")
            .y_axis_label("m")
            .show(ui, |plot| {
                // The one data series in the app rides the interactive
                // cobalt over the recessed paper well.
                plot.line(Line::new("Elevation", points).color(theme::accent()));
            });
    }
    ui.add_space(2.0);
}

/// Cumulative RX total for the bottom strip. Below 1 MB it reads in kB so
/// the counter visibly ticks at real correction rates (a 0.5-1 kB/s stream
/// would sit on "0.01 MB" for minutes); above, the familiar MB reading.
pub fn fmt_total(bytes: u64) -> String {
    if bytes < 1_000_000 {
        format!("{:.1} kB", bytes as f64 / 1e3)
    } else {
        format!("{:.2} MB", bytes as f64 / 1e6)
    }
}

/// The graph's caption, in the same millimetre resolution as the
/// Elevation readout slots and the base height, so the three agree digit
/// for digit on the same receiver.
fn elevation_stats(
    cur: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    range: Option<f64>,
) -> String {
    let fmt = |v: Option<f64>| v.map_or_else(|| "-".to_string(), |x| format!("{x:.3}"));
    format!(
        "cur {}  min {}  max {}  range {} m",
        fmt(cur),
        fmt(min),
        fmt(max),
        fmt(range)
    )
}

#[cfg(test)]
mod tests {
    use super::{elevation_stats, fmt_total};

    /// The kB/MB switchover: per-second visible motion at stream rates,
    /// MB once totals are MB-sized.
    #[test]
    fn elevation_stats_read_to_the_millimetre() {
        assert_eq!(
            elevation_stats(Some(1615.876), Some(1615.8), Some(1616.0004), Some(0.2004)),
            "cur 1615.876  min 1615.800  max 1616.000  range 0.200 m"
        );
        assert_eq!(
            elevation_stats(None, None, None, None),
            "cur -  min -  max -  range - m"
        );
    }

    #[test]
    fn total_reads_in_kb_below_one_mb() {
        assert_eq!(fmt_total(0), "0.0 kB");
        assert_eq!(fmt_total(14_237), "14.2 kB");
        assert_eq!(fmt_total(999_949), "999.9 kB");
        assert_eq!(fmt_total(1_000_000), "1.00 MB");
        assert_eq!(fmt_total(123_456_789), "123.46 MB");
    }
}
