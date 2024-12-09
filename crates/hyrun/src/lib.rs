use core::panic;

use amm::{AmmAction, AmmState};
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
    Approve { spender: String, amount: u128 },
    Transfer { recipient: String, amount: u128 },
}
impl From<HyllarArgs> for ERC20Action {
    fn from(cmd: HyllarArgs) -> Self {
        match cmd {
            HyllarArgs::Transfer { recipient, amount } => Self::Transfer { recipient, amount },
            HyllarArgs::Approve { spender, amount } => Self::Approve { spender, amount },
            HyllarArgs::Init { .. } => panic!("Init is not a valid contract function"),
        }
    }
}
#[derive(Subcommand, Clone)]
pub enum AmmArgs {
    NewPair {
        token_a: String,
        token_b: String,
        amount_a: u128,
        amount_b: u128,
    },
    Swap {
        token_a: String,
        token_b: String,
        amount_a: u128,
        amount_b: u128,
    },
}

#[derive(Subcommand, Clone)]
pub enum CliCommand {
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
        hyllar_contract_name: String,
    },
    Amm {
        #[command(subcommand)]
        command: AmmArgs,
        amm_contract_name: String,
    },
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: CliCommand,

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

// Public because it's used in integration tests.
pub fn run_command(cli: Cli) {
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
            "amm" | "amm2" => {
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
        CliCommand::Hyllar {
            hyllar_contract_name,
            command,
        } => {
            if let HyllarArgs::Init { initial_supply } = command {
                contract::init(
                    &hyllar_contract_name,
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
                cf.as_blob(ContractName(hyllar_contract_name.clone()), None, None),
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
                &hyllar_contract_name,
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
        CliCommand::Amm {
            amm_contract_name,
            command,
        } => {
            match command {
                AmmArgs::NewPair {
                    token_a,
                    token_b,
                    amount_a,
                    amount_b,
                } => {
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
                        AmmAction::NewPair {
                            pair: (token_a.clone(), token_b.clone()),
                            amounts: (amount_a, amount_b),
                        }
                        .as_blob(
                            ContractName(amm_contract_name.to_owned()),
                            None,
                            Some(vec![BlobIndex(2), BlobIndex(3)]),
                        ),
                        ERC20Action::TransferFrom {
                            sender: identity.clone(),
                            recipient: amm_contract_name.to_string(),
                            amount: amount_a,
                        }
                        .as_blob(
                            ContractName(token_a.to_owned()),
                            Some(BlobIndex(1)),
                            None,
                        ),
                        ERC20Action::TransferFrom {
                            sender: identity.clone(),
                            recipient: amm_contract_name.to_string(),
                            amount: amount_b,
                        }
                        .as_blob(
                            ContractName(token_b.to_owned()),
                            Some(BlobIndex(1)),
                            None,
                        ),
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
                    // Run to add new pair to Amm
                    contract::run(
                        &cli,
                        &amm_contract_name,
                        |amm: amm::AmmState| -> ContractInput<amm::AmmState> {
                            ContractInput::<AmmState> {
                                initial_state: amm,
                                identity: identity.clone().into(),
                                tx_hash: TxHash("".to_owned()),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(1),
                            }
                        },
                    );
                    // Run for transferring token_a
                    contract::run(
                        &cli,
                        &token_a,
                        |token: hyllar::HyllarToken| -> ContractInput<hyllar::HyllarToken> {
                            ContractInput::<HyllarToken> {
                                initial_state: token,
                                identity: identity.clone().into(),
                                tx_hash: TxHash("".to_owned()),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(2),
                            }
                        },
                    );

                    // Run for transferring token_b
                    contract::run(
                        &cli,
                        &token_b,
                        |token: hyllar::HyllarToken| -> ContractInput<hyllar::HyllarToken> {
                            ContractInput::<HyllarToken> {
                                initial_state: token,
                                identity: identity.clone().into(),
                                tx_hash: TxHash("".to_owned()),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(3),
                            }
                        },
                    );
                }
                AmmArgs::Swap {
                    token_a,
                    token_b,
                    amount_a,
                    amount_b,
                } => {
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
                        AmmAction::Swap {
                            pair: (token_a.to_string(), token_b.to_string()),
                            amounts: (amount_a, amount_b),
                        }
                        .as_blob(
                            ContractName(amm_contract_name.to_owned()),
                            None,
                            Some(vec![BlobIndex(2), BlobIndex(3)]),
                        ),
                        ERC20Action::TransferFrom {
                            sender: identity.clone(),
                            recipient: amm_contract_name.to_string(),
                            amount: amount_a,
                        }
                        .as_blob(
                            ContractName(token_a.to_owned()),
                            Some(BlobIndex(1)),
                            None,
                        ),
                        ERC20Action::Transfer {
                            recipient: identity.clone(),
                            amount: amount_b,
                        }
                        .as_blob(
                            ContractName(token_b.to_owned()),
                            Some(BlobIndex(1)),
                            None,
                        ),
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
                    // Run for swapping token_a for token_b
                    contract::run(
                        &cli,
                        &amm_contract_name,
                        |amm: amm::AmmState| -> ContractInput<amm::AmmState> {
                            ContractInput::<AmmState> {
                                initial_state: amm,
                                identity: identity.clone().into(),
                                tx_hash: TxHash("".to_owned()),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(1),
                            }
                        },
                    );
                    // Run for transferring token_a
                    contract::run(
                        &cli,
                        &token_a,
                        |token: hyllar::HyllarToken| -> ContractInput<hyllar::HyllarToken> {
                            ContractInput::<HyllarToken> {
                                initial_state: token,
                                identity: identity.clone().into(),
                                tx_hash: TxHash("".to_owned()),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(2),
                            }
                        },
                    );

                    // Run for transferring token_b
                    contract::run(
                        &cli,
                        &token_b,
                        |token: hyllar::HyllarToken| -> ContractInput<hyllar::HyllarToken> {
                            ContractInput::<HyllarToken> {
                                initial_state: token,
                                identity: identity.clone().into(),
                                tx_hash: TxHash("".to_owned()),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(3),
                            }
                        },
                    );
                }
            }
        }
    };
}
