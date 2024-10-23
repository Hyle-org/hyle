use sdk::{BlobData, ContractInput};
use serde::Deserialize;

use clap::{Parser, Subcommand};

mod contract;

#[derive(Debug, Deserialize)]
pub struct ContractName(pub String);

#[derive(Deserialize, Debug)]
pub struct Contract {
    pub name: ContractName,
    pub program_id: Vec<u8>,
    pub state: sdk::StateDigest,
    pub verifier: String,
}

#[derive(Subcommand, Clone)]
pub enum HyfiArgs {
    Init,
    State,
    Transfer {
        from: String,
        to: String,
        amount: u64,
    },
    Mint {
        to: String,
        amount: u64,
    },
}
impl From<HyfiArgs> for hyfi::model::ContractFunction {
    fn from(cmd: HyfiArgs) -> Self {
        match cmd {
            HyfiArgs::Transfer { from, to, amount } => Self::Transfer { from, to, amount },
            HyfiArgs::Mint { to, amount } => Self::Mint { to, amount },
            HyfiArgs::Init => panic!("Init is not a valid contract function"),
            HyfiArgs::State => panic!("State is not a valid contract function"),
        }
    }
}

#[derive(Subcommand, Clone)]
pub enum HydentityArgs {
    Init,
    Register { account: String, password: String },
    CheckPassword { account: String, password: String },
}
impl From<HydentityArgs> for hydentity::model::ContractFunction {
    fn from(cmd: HydentityArgs) -> Self {
        match cmd {
            HydentityArgs::Register { account, password } => Self::Register { account, password },
            HydentityArgs::CheckPassword { account, password } => {
                Self::CheckPassword { account, password }
            }
            HydentityArgs::Init => panic!("Init is not a valid contract function"),
        }
    }
}

#[derive(Subcommand, Clone)]
enum ContractChoice {
    Hyfi {
        #[command(subcommand)]
        command: HyfiArgs,
    },
    Hydentity {
        #[command(subcommand)]
        command: HydentityArgs,
    },
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: ContractChoice,

    #[clap(long, short)]
    init: bool,

    #[arg(long, default_value = "localhost")]
    pub host: String,

    #[arg(long, default_value = "4321")]
    pub port: u32,
}

fn main() {
    let cli = Cli::parse();

    match cli.command.clone() {
        ContractChoice::Hyfi { command } => {
            if matches!(command, HyfiArgs::Init) {
                contract::init("hyfi", hyfi::model::Balances::default());
                return;
            }
            if matches!(command, HyfiArgs::State) {
                let state = contract::fetch_current_state::<hyfi::model::Balances>(&cli, "hyfi");
                println!("Current state: {:?}", state);
                return;
            }
            let cf: hyfi::model::ContractFunction = command.into();
            contract::run(
                &cli,
                "hyfi",
                cf.clone(),
                |balances: hyfi::model::Balances| -> ContractInput<hyfi::model::Balances> {
                    // TODO: Allow user to add real tx_hash
                    let tx_hash = "".to_string();
                    // TODO: Allow user to add multiple values in payload
                    let blobs = vec![BlobData(
                        bincode::encode_to_vec(cf.clone(), bincode::config::standard())
                            .expect("failed to encode program inputs"),
                    )];

                    let index = 0;

                    ContractInput::<hyfi::model::Balances> {
                        initial_state: balances,
                        tx_hash,
                        blobs,
                        index,
                    }
                },
            );
        }
        ContractChoice::Hydentity { command } => {
            if matches!(command, HydentityArgs::Init) {
                contract::init("hydentity", hydentity::model::Identities::default());
                return;
            }
            let cf: hydentity::model::ContractFunction = command.into();
            contract::run(
                &cli,
                "hydentity",
                cf.clone(),
                |identities: hydentity::model::Identities| -> ContractInput::<hydentity::model::Identities> {
                    // TODO: Allow user to add real tx_hash
                    let tx_hash = "".to_string();
                    // TODO: Allow user to add multiple values in payload
                    let blobs = vec![BlobData(
                        bincode::encode_to_vec(cf.clone(), bincode::config::standard())
                            .expect("failed to encode program inputs"),
                    )];

                    let index = 0;

                    ContractInput::<hydentity::model::Identities> {
                        initial_state: identities,
                        tx_hash,
                        blobs,
                        index,
                    }
                },
            );
        }
    };
}
