use clap::Parser;
use hyrun::{run_command, Cli};

fn main() {
    let cli = Cli::parse();
    run_command(cli);
}
