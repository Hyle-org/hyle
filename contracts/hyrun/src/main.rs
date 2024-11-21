use hydentity::Hydentity;
use hyllar::HyllarToken;
use sdk::{erc20::ERC20Action, identity_provider::IdentityAction, BlobData, ContractInput};
use serde::Deserialize;

use clap::{Parser, Subcommand};

mod contract;

#[derive(Debug, Deserialize)]
pub struct ContractName(pub String);

impl From<String> for ContractName {
    fn from(s: String) -> Self {
        ContractName(s)
    }
}

impl From<&str> for ContractName {
    fn from(s: &str) -> Self {
        ContractName(s.into())
    }
}

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

enum ContractFunctionEnum {
    Hydentity(IdentityAction),
    Hyllar(ERC20Action),
}

impl bincode::Encode for ContractFunctionEnum {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        match self {
            ContractFunctionEnum::Hydentity(f) => f.encode(encoder),
            ContractFunctionEnum::Hyllar(f) => f.encode(encoder),
        }
    }
}

impl From<IdentityAction> for ContractFunctionEnum {
    fn from(val: IdentityAction) -> Self {
        ContractFunctionEnum::Hydentity(val)
    }
}
impl From<ERC20Action> for ContractFunctionEnum {
    fn from(val: ERC20Action) -> Self {
        ContractFunctionEnum::Hyllar(val)
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

    #[arg(long, default_value = "user")]
    pub user: String,

    #[arg(long, default_value = "password")]
    pub password: String,

    #[arg(long, default_value = "localhost")]
    pub host: String,

    #[arg(long, default_value = "4321")]
    pub port: u32,
}

fn main() {
    let cli = Cli::parse();

    // TODO - get identity from user input
    let identity = sdk::Identity(cli.user.clone());

    match cli.command.clone() {
        CliCommand::State { contract } => match contract.as_str() {
            "hydentity" => {
                let state = contract::fetch_current_state::<Hydentity>(&cli, &contract)
                    .expect("failed to fetch state");
                println!("State: {:?}", state);
            }
            "hyllar" => {
                let state = contract::fetch_current_state::<HyllarToken>(&cli, &contract)
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
            contract::print_hyled_blob_tx(&identity, vec![("hydentity".into(), cf.clone().into())]);
            let blobs = vec![cf.into()];

            contract::run(
                &cli,
                "hydentity",
                |identities: Hydentity| -> ContractInput<Hydentity> {
                    ContractInput::<Hydentity> {
                        initial_state: identities,
                        identity: identity.clone(),
                        tx_hash: "".to_string(),
                        contract_name: "hydentity".into(),
                        private_blob: BlobData(cli.password.as_bytes().to_vec()),
                        blobs: blobs.clone(),
                        index: 0,
                    }
                },
            );
        }
        CliCommand::Hyllar { command } => {
            if let HyllarArgs::Init { initial_supply } = command {
                contract::init("hyllar", HyllarToken::new(initial_supply));
                return;
            }
            let cf: ERC20Action = command.into();
            let identity_cf: IdentityAction = IdentityAction::VerifyIdentity {
                account: identity.0.clone(),
                blobs_hash: vec!["".into()], // TODO: hash blob
            };
            contract::print_hyled_blob_tx(
                &identity,
                vec![
                    ("hydentity".into(), identity_cf.clone().into()),
                    ("hyllar".into(), cf.clone().into()),
                ],
            );

            let blobs = vec![identity_cf.into(), cf.into()];

            contract::run(
                &cli,
                "hyllar",
                |token: hyllar::HyllarToken| -> ContractInput<hyllar::HyllarToken> {
                    ContractInput::<HyllarToken> {
                        initial_state: token,
                        identity: identity.clone(),
                        tx_hash: "".to_string(),
                        contract_name: "hyllar".into(),
                        private_blob: BlobData(vec![]),
                        blobs: blobs.clone(),
                        index: 1,
                    }
                },
            );
            contract::run(
                &cli,
                "hydentity",
                |token: hydentity::Hydentity| -> ContractInput<hydentity::Hydentity> {
                    ContractInput::<Hydentity> {
                        initial_state: token,
                        identity: identity.clone(),
                        tx_hash: "".to_string(),
                        contract_name: "hydentity".into(),
                        private_blob: BlobData(cli.password.as_bytes().to_vec()),
                        blobs: blobs.clone(),
                        index: 0,
                    }
                },
            );
        }
    };
}
