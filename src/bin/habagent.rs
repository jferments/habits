#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    if let Err(err) = habits::run_agent() {
        habits::write_fatal_log(&format!("{err:#}"));
        // Make Task Scheduler report a real failure instead of exit code 0.
        std::process::exit(1);
    }
}
