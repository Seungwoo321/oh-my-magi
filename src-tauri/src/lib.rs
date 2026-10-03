mod commands;
mod core_dispatch;
mod desktop;
mod native_source_picker;
mod pdf_capture;
mod preferences;
mod profiles;
mod records;
mod release;
mod role_files;
pub mod run_projection;
mod runtime_budget;
mod source_observations;
mod store_selection;
mod strict_json;

pub fn run() {
    desktop::run();
}

#[cfg(feature = "native-live-probe")]
mod native_live_tests;

#[cfg(feature = "native-live-probe")]
pub fn run_native_live_probe() {
    native_live_tests::run();
}
