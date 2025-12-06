use std::env;
use tracing::info;

pub fn welcome() {

    let version = env!("CARGO_PKG_VERSION");
    let run_mode = env::var("ISM_MODE").unwrap_or_else(|_| "development".into());

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