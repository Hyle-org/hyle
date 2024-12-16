use clap::Parser;
use hyrun::{run_command, Cli, Context, ContractData};

fn main() {
    let cli = Cli::parse();
    let context = Context {
        cli,
        contract_data: ContractData::default(),
        hardcoded_initial_states: Default::default(),
    };
    run_command(&context);
}
