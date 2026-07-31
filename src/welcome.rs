use tracing::info;

/// Prints the startup banner.
///
/// Takes the mode rather than reading `ISM_MODE` itself, so the banner cannot disagree with the
/// configuration that was actually loaded — it used to keep its own copy of the default.
pub fn welcome(run_mode: &str) {
    let version = env!("CARGO_PKG_VERSION");

    let title = [
        r"  ___ ____  __  __  ",
        r" |_ _/ ___||  \/  | ",
        r"  | |\___ \| |\/| | ",
        r"  | | ___) | |  | | ",
        r" |__||____/|_|  |_| ",
    ];
    for line in title {
        println!("{}", line);
    }
    println!();
    println!("Version: {} | Run-Mode: {}", version, run_mode);
    println!();
    info!("Starting up ISM in {run_mode} mode.");
}
