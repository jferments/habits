fn main() {
    if let Err(err) = habits::cli_main() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
