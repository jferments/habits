#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    if let Err(e) = habits::run_browser_bridge() {
        habits::write_fatal_log(&format!("browser bridge: {e:#}"));
        std::process::exit(1);
    }
}
