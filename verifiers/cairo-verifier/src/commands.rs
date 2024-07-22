use clap::{Args, Parser, Subcommand};

#[derive(Subcommand, Debug)]
pub enum ProverEntity {
    #[clap(about = "Generate a proof from a given trace of a cairo program execution")]
    Prove(ProveArgs),
    #[clap(about = "Verify a proof for a given compiled cairo program")]
    Verify(VerifyArgs)
}

#[derive(Args, Debug)]
pub struct ProveArgs {
    pub trace_bin_path: String,
    pub memory_bin_path: String,
    pub proof_path: String,
    pub output_path: String,
}

#[derive(Args, Debug)]
pub struct VerifyArgs {
    pub proof_path: String,
}
#[derive(Parser, Debug)]
pub struct ProverArgs {
    #[clap(subcommand)]
    pub entity: ProverEntity,
}