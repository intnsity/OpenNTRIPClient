//! eframe/egui user interface. `main_window::App` owns everything; the other
//! modules are pure render functions over it, split by screen region.

mod about;
mod bottom_tabs;
mod connlog_window;
mod dialog;
mod gga_section;
mod icons;
mod log_pane;
mod main_window;
mod ntrip_block;
mod options_dialog;
mod plot_panel;
mod profile_transfer;
mod profiles_dialog;
mod rtcm_inspector;
mod serial_block;
mod session_band;
mod sourcetable_browser;
mod status_strip;
mod stream_summary;
mod text;
mod theme;

pub use main_window::App;
