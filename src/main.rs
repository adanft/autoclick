/// Starts the CLI application and reports any fatal error to stderr.
fn main() {
    autoclick::init_logging();

    if let Err(error) = autoclick::app::run() {
        eprintln!("fatal: {error:#}");
        std::process::exit(1);
    }
}
