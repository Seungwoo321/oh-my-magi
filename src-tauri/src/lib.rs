mod commands;
mod desktop;
mod pdf_capture;
mod preferences;
mod profiles;
mod records;
pub mod run_projection;

pub fn run() {
    desktop::run();
}
