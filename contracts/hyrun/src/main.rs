use core::panic;

use amm::AmmState;
use hydentity::Hydentity;
use hyllar::HyllarToken;
use sdk::{
    erc20::ERC20Action, identity_provider::IdentityAction, BlobData, BlobIndex, ContractInput,
    ContractName, TxHash,
};
use serde::Deserialize;

use clap::{Parser, Subcommand};

mod contract;

#[derive(Deserialize, Debug)]
pub struct Contract {
    pub name: ContractName,
    pub program_id: Vec<u8>,
    pub state: sdk::StateDigest,
    pub verifier: String,
}

#[derive(Subcommand, Clone)]
pub enum HydentityArgs {
    Init,
    Register { account: String },
}
impl From<HydentityArgs> for sdk::identity_provider::IdentityAction {
    fn from(cmd: HydentityArgs) -> Self {
        match cmd {
            HydentityArgs::Register { account } => Self::RegisterIdentity { account },
            HydentityArgs::Init => panic!("Init is not a valid contract function"),
        }
    }
}
#[derive(Subcommand, Clone)]
pub enum HyllarArgs {
    Init { initial_supply: u128 },
    Transfer { recipient: String, amount: u128 },
}
impl From<HyllarArgs> for ERC20Action {
    fn from(cmd: HyllarArgs) -> Self {
        match cmd {
            HyllarArgs::Transfer { recipient, amount } => Self::Transfer { recipient, amount },
            HyllarArgs::Init { .. } => panic!("Init is not a valid contract function"),
        }
    }
}

#[derive(Subcommand, Clone)]
enum CliCommand {
    State {
        contract: String,
    },
    Hydentity {
        #[command(subcommand)]
        command: HydentityArgs,
    },
    Hyllar {
        #[command(subcommand)]
        command: HyllarArgs,
    },
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,

    #[clap(long, short)]
    init: bool,

    #[arg(long, short)]
    pub user: Option<String>,

    #[arg(long, short)]
    pub password: Option<String>,

    #[arg(long, short)]
    pub nonce: Option<u32>,

    #[arg(long, default_value = "localhost")]
    pub host: String,

    #[arg(long, default_value = "4321")]
    pub port: u32,
}

fn main() {
    let cli = Cli::parse();

    match cli.command.clone() {
        CliCommand::State { contract } => match contract.as_str() {
            "hydentity" => {
                let state = contract::fetch_current_state::<Hydentity>(&cli, &contract)
                    .expect("failed to fetch state");
                println!("State: {:?}", state);
            }
            "hyllar" | "hyllar2" => {
                let state = contract::fetch_current_state::<HyllarToken>(&cli, &contract)
                    .expect("failed to fetch state");
                println!("State: {:?}", state);
            }
            "amm" => {
                let state = contract::fetch_current_state::<AmmState>(&cli, &contract)
                    .expect("failed to fetch state");
                println!("State: {:?}", state);
            }
            _ => panic!("Unknown contract"),
        },
        CliCommand::Hydentity { command } => {
            if matches!(command, HydentityArgs::Init) {
                contract::init("hydentity", Hydentity::new());
                return;
            }
            let cf: IdentityAction = command.into();
            let identity = sdk::Identity(
                cli.user
                    .clone()
                    .unwrap_or_else(|| panic!("Missing user argument")),
            );
            let password = cli
                .password
                .clone()
                .unwrap_or_else(|| panic!("Missing password argument"))
                .as_bytes()
                .to_vec();
            let blobs = vec![cf.as_blob(ContractName("hydentity".to_owned()))];
            contract::print_hyled_blob_tx(&identity, &blobs);

            contract::run(
                &cli,
                "hydentity",
                |identities: Hydentity| -> ContractInput<Hydentity> {
                    ContractInput::<Hydentity> {
                        initial_state: identities,
                        identity: identity.clone(),
                        tx_hash: TxHash("".to_owned()),
                        private_blob: BlobData(password.clone()),
                        blobs: blobs.clone(),
                        index: BlobIndex(0),
                    }
                },
            );
        }
        CliCommand::Hyllar { command } => {
            if let HyllarArgs::Init { initial_supply } = command {
                contract::init(
                    "hyllar",
                    HyllarToken::new(initial_supply, "faucet.hydentity".to_string()),
                );
                return;
            }
            let cf: ERC20Action = command.into();
            let identity = cli
                .user
                .clone()
                .unwrap_or_else(|| panic!("Missing user argument"));
            let nonce = cli
                .nonce
                .unwrap_or_else(|| panic!("Missing nonce argument"));
            let password = cli
                .password
                .clone()
                .unwrap_or_else(|| panic!("Missing password argument"))
                .as_bytes()
                .to_vec();
            let identity_cf: IdentityAction = IdentityAction::VerifyIdentity {
                account: identity.clone(),
                nonce,
            };

            let blobs = vec![
                identity_cf.as_blob(ContractName("hydentity".to_owned())),
                cf.as_blob(ContractName("hyllar".to_owned()), None, None),
            ];
            contract::print_hyled_blob_tx(&identity.clone().into(), &blobs);

            contract::run(
                &cli,
                "hydentity",
                |token: hydentity::Hydentity| -> ContractInput<hydentity::Hydentity> {
                    ContractInput::<Hydentity> {
                        initial_state: token,
                        identity: identity.clone().into(),
                        tx_hash: TxHash("".to_owned()),
                        private_blob: BlobData(password.clone()),
                        blobs: blobs.clone(),
                        index: BlobIndex(0),
                    }
                },
            );
            contract::run(
                &cli,
                "hyllar",
                |token: hyllar::HyllarToken| -> ContractInput<hyllar::HyllarToken> {
                    ContractInput::<HyllarToken> {
                        initial_state: token,
                        identity: identity.clone().into(),
                        tx_hash: TxHash("".to_owned()),
                        private_blob: BlobData(vec![]),
                        blobs: blobs.clone(),
                        index: BlobIndex(1),
                    }
                },
            );
        }
    };
}
