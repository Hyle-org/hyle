use fixtures::ctx::E2ECtx;
use tracing::info;

use hyle::model::ProofData;

mod fixtures;

use anyhow::Result;

mod e2e_amm {
    use amm::AmmAction;
    use fixtures::{
        contracts::{AmmContract, HyllarContract},
        proofs::HyrunProofGen,
    };
    use hydentity::AccountInfo;
    use hyle_contract_sdk::{
        erc20::{ERC20Action, ERC20},
        identity_provider::{IdentityAction, IdentityVerification},
        BlobIndex, ContractAction, ContractName,
    };
    use hyrun::{CliCommand, HydentityArgs};

    use super::*;

    async fn assert_account_allowance(
        ctx: &E2ECtx,
        contract_name: &str,
        owner: &str,
        spender: &str,
        expected_allowance: u128,
    ) -> Result<()> {
        let contract_hyllar = ctx.get_contract(contract_name).await?;
        let state: hyllar::HyllarToken = contract_hyllar.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());

        assert_eq!(
            state
                .allowance(owner, spender)
                .expect("bob identity not found"),
            expected_allowance
        );
        Ok(())
    }

    async fn assert_multiple_balances(
        ctx: &E2ECtx,
        contract_name: &str,
        balances: &[(&str, u128)],
    ) -> Result<()> {
        let contract_hyllar = ctx.get_contract(contract_name).await?;
        let state: hyllar::HyllarToken = contract_hyllar.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());

        for (account, expected) in balances {
            assert_eq!(
                state.balance_of(account).expect("Account not found"),
                *expected,
                "Incorrect balance for {}",
                account
            );
        }
        Ok(())
    }

    async fn scenario_amm(ctx: E2ECtx) -> Result<()> {
        // Here is the flow that we are going to test:
        // Register bob in hydentity

        // Send 25 hyllar from faucet to bob
        // Send 50 hyllar2 from faucet to bob

        // Register new amm contract "amm2"

        // Bob approves 100 hyllar to amm2
        // Bob approves 100 hyllar2 to amm2

        // Bob registers a new pair in amm
        //    By sending 20 hyllar to amm
        //    By sending 50 hyllar2 to amm

        // Bob swaps 5 hyllar for 10 hyllar2
        //    By sending 5 hyllar to amm
        //    By sending 10 hyllar2 to bob (from amm)

        let proof_generator = HyrunProofGen::setup_working_directory();

        let contract_hyllar = ctx.get_contract("hyllar").await?;
        let state =
            hyllar::HyllarTokenContract::init(contract_hyllar.state.try_into()?, "caller".into());
        let hyllar_initial_total_amount: u128 = state
            .balance_of("faucet.hydentity")
            .expect("faucet identity not found");

        ///////////////////// hyllar2 contract registration /////////////////
        info!("➡️  Registring hyllar2 contract");
        const HYLLAR2_CONTRACT_NAME: &str = "hyllar2";
        ctx.register_contract::<HyllarContract>(HYLLAR2_CONTRACT_NAME)
            .await?;
        /////////////////////////////////////////////////////////////////////

        let contract_hyllar2 = ctx.get_contract("hyllar2").await?;
        let state =
            hyllar::HyllarTokenContract::init(contract_hyllar2.state.try_into()?, "caller".into());
        let hyllar2_initial_total_amount: u128 = state
            .balance_of("faucet.hydentity")
            .expect("faucet identity not found");

        ///////////////////// bob identity registration /////////////////////
        info!("➡️  Sending blob to register bob identity");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![IdentityAction::RegisterIdentity {
                    account: "bob.hydentity".to_string(),
                }
                .as_blob(ContractName("hydentity".to_owned()))],
            )
            .await?;

        proof_generator
            .generate_proof(
                &ctx,
                CliCommand::Hydentity {
                    command: HydentityArgs::Register {
                        account: "bob.hydentity".to_string(),
                    },
                },
                "bob.hydentity",
                "password",
                None,
            )
            .await;

        let proof = proof_generator.read_proof(0);

        info!("➡️  Sending proof for register");
        ctx.send_proof_single(
            "hydentity".into(),
            ProofData::Bytes(proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Waiting for height 2");
        ctx.wait_height(2).await?;

        let contract_hydentity = ctx.get_contract("hydentity").await?;
        let state: hydentity::Hydentity = contract_hydentity.state.try_into()?;

        // faucet_start_nonce = 0 in single-mode, N in multi-node(N) mode
        let faucet_start_nonce = serde_json::from_str::<AccountInfo>(
            state
                .get_identity_info("faucet.hydentity")
                .expect("faucet identity not found")
                .as_str(),
        )
        .expect("Failed to parse faucet identity info")
        .nonce;

        /////////////////////////////////////////////////////////////////////

        ///////////////// sending hyllar from faucet to bob /////////////////
        info!("➡️  Sending blob to transfer 25 hyllar from faucet to bob");
        let blob_tx_hash = ctx
            .send_blob(
                "faucet.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "faucet.hydentity".to_string(),
                        nonce: faucet_start_nonce,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    ERC20Action::Transfer {
                        recipient: "bob.hydentity".to_string(),
                        amount: 25,
                    }
                    .as_blob(ContractName("hyllar".to_owned()), None, None),
                ],
            )
            .await?;

        proof_generator
            .generate_proof(
                &ctx,
                CliCommand::Hyllar {
                    command: hyrun::HyllarArgs::Transfer {
                        recipient: "bob.hydentity".to_string(),
                        amount: 25,
                    },
                    hyllar_contract_name: "hyllar".to_string(),
                },
                "faucet.hydentity",
                "password",
                Some(faucet_start_nonce),
            )
            .await;

        let hydentity_proof = proof_generator.read_proof(0);
        let bob_transfer_proof = proof_generator.read_proof(1);

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof_single(
            "hyllar".into(),
            ProofData::Bytes(bob_transfer_proof),
            blob_tx_hash,
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        let contract_hyllar = ctx.get_contract("hyllar").await?;
        let state: hyllar::HyllarToken = contract_hyllar.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());
        assert_eq!(
            state
                .balance_of("bob.hydentity")
                .expect("bob identity not found"),
            25
        );
        assert_multiple_balances(
            &ctx,
            "hyllar",
            &[
                ("bob.hydentity", 25),
                ("faucet.hydentity", hyllar_initial_total_amount - 25),
            ],
        )
        .await?;
        /////////////////////////////////////////////////////////////////////

        ///////////////// sending hyllar2 from faucet to bob /////////////////
        info!("➡️  Sending blob to transfer 50 hyllar2 from faucet to bob");
        let blob_tx_hash = ctx
            .send_blob(
                "faucet.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "faucet.hydentity".to_string(),
                        nonce: faucet_start_nonce + 1,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    ERC20Action::Transfer {
                        recipient: "bob.hydentity".to_string(),
                        amount: 50,
                    }
                    .as_blob(ContractName("hyllar2".to_owned()), None, None),
                ],
            )
            .await?;

        proof_generator
            .generate_proof(
                &ctx,
                CliCommand::Hyllar {
                    command: hyrun::HyllarArgs::Transfer {
                        recipient: "bob.hydentity".to_string(),
                        amount: 50,
                    },
                    hyllar_contract_name: "hyllar2".to_string(),
                },
                "faucet.hydentity",
                "password",
                Some(faucet_start_nonce + 1),
            )
            .await;

        let hydentity_proof = proof_generator.read_proof(0);
        let bob_transfer_proof = proof_generator.read_proof(1);

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof_single(
            "hyllar2".into(),
            ProofData::Bytes(bob_transfer_proof),
            blob_tx_hash,
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        assert_multiple_balances(
            &ctx,
            "hyllar2",
            &[
                ("bob.hydentity", 50),
                ("faucet.hydentity", hyllar2_initial_total_amount - 50),
            ],
        )
        .await?;
        /////////////////////////////////////////////////////////////////////

        ///////////////////// amm contract registration /////////////////////
        info!("➡️  Registring amm contract");
        const AMM_CONTRACT_NAME: &str = "amm2";
        ctx.register_contract::<AmmContract>(AMM_CONTRACT_NAME)
            .await?;
        /////////////////////////////////////////////////////////////////////

        //////////////////// Bob approves AMM on hyllar /////////////////////
        info!("➡️  Sending blob to approve amm on hyllar");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "bob.hydentity".to_string(),
                        nonce: 0,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    ERC20Action::Approve {
                        spender: AMM_CONTRACT_NAME.to_string(),
                        amount: 100,
                    }
                    .as_blob(ContractName("hyllar".to_owned()), None, None),
                ],
            )
            .await?;

        proof_generator
            .generate_proof(
                &ctx,
                CliCommand::Hyllar {
                    command: hyrun::HyllarArgs::Approve {
                        spender: "amm2".to_string(),
                        amount: 100,
                    },
                    hyllar_contract_name: "hyllar".to_string(),
                },
                "bob.hydentity",
                "password",
                Some(0),
            )
            .await;

        let hydentity_proof = proof_generator.read_proof(0);
        let bob_approve_hyllar_proof = proof_generator.read_proof(1);

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for approve hyllar");
        ctx.send_proof_single(
            "hyllar".into(),
            ProofData::Bytes(bob_approve_hyllar_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        assert_account_allowance(&ctx, "hyllar", "bob.hydentity", "amm2", 100).await?;
        /////////////////////////////////////////////////////////////////////

        //////////////////// Bob approves AMM on hyllar2 /////////////////////
        info!("➡️  Sending blob to approve amm on hyllar2");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "bob.hydentity".to_string(),
                        nonce: 1,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    ERC20Action::Approve {
                        spender: AMM_CONTRACT_NAME.to_string(),
                        amount: 100,
                    }
                    .as_blob(ContractName("hyllar2".to_owned()), None, None),
                ],
            )
            .await?;

        proof_generator
            .generate_proof(
                &ctx,
                CliCommand::Hyllar {
                    command: hyrun::HyllarArgs::Approve {
                        spender: "amm2".to_string(),
                        amount: 100,
                    },
                    hyllar_contract_name: "hyllar2".to_string(),
                },
                "bob.hydentity",
                "password",
                Some(1),
            )
            .await;

        let hydentity_proof = proof_generator.read_proof(0);
        let bob_approve_hyllar2_proof = proof_generator.read_proof(1);

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for approve hyllar2");
        ctx.send_proof_single(
            "hyllar2".into(),
            ProofData::Bytes(bob_approve_hyllar2_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        assert_account_allowance(&ctx, "hyllar2", "bob.hydentity", "amm2", 100).await?;
        /////////////////////////////////////////////////////////////////////

        /////////////// Creating new pair hyllar/hyllar2 on amm ///////////////
        info!("➡️  Sending blob to transfer 50 hyllar2 from faucet to bob");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "bob.hydentity".to_string(),
                        nonce: 2,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    AmmAction::NewPair {
                        pair: ("hyllar".to_string(), "hyllar2".to_string()),
                        amounts: (20, 50),
                    }
                    .as_blob(
                        ContractName(AMM_CONTRACT_NAME.to_owned()),
                        None,
                        Some(vec![BlobIndex(2), BlobIndex(3)]),
                    ),
                    ERC20Action::TransferFrom {
                        sender: "bob.hydentity".to_string(),
                        recipient: AMM_CONTRACT_NAME.to_string(),
                        amount: 20,
                    }
                    .as_blob(
                        ContractName("hyllar".to_owned()),
                        Some(BlobIndex(1)),
                        None,
                    ),
                    ERC20Action::TransferFrom {
                        sender: "bob.hydentity".to_string(),
                        recipient: AMM_CONTRACT_NAME.to_string(),
                        amount: 50,
                    }
                    .as_blob(
                        ContractName("hyllar2".to_owned()),
                        Some(BlobIndex(1)),
                        None,
                    ),
                ],
            )
            .await?;

        proof_generator
            .generate_proof(
                &ctx,
                CliCommand::Amm {
                    command: hyrun::AmmArgs::NewPair {
                        token_a: "hyllar".to_owned(),
                        token_b: "hyllar2".to_owned(),
                        amount_a: 20,
                        amount_b: 50,
                    },
                    amm_contract_name: "amm2".to_string(),
                },
                "bob.hydentity",
                "password",
                Some(2),
            )
            .await;

        let hydentity_proof = proof_generator.read_proof(0);
        let bob_new_pair_proof = proof_generator.read_proof(1);
        let bob_transfer_hyllar_proof = proof_generator.read_proof(2);
        let bob_transfer_hyllar2_proof = proof_generator.read_proof(3);

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for new pair");
        ctx.send_proof_single(
            AMM_CONTRACT_NAME.into(),
            ProofData::Bytes(bob_new_pair_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof_single(
            "hyllar".into(),
            ProofData::Bytes(bob_transfer_hyllar_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for hyllar2");
        ctx.send_proof_single(
            "hyllar2".into(),
            ProofData::Bytes(bob_transfer_hyllar2_proof),
            blob_tx_hash,
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        assert_multiple_balances(
            &ctx,
            "hyllar",
            &[
                ("bob.hydentity", 5),
                ("amm2", 20),
                ("faucet.hydentity", hyllar_initial_total_amount - 25),
            ],
        )
        .await?;

        assert_multiple_balances(
            &ctx,
            "hyllar2",
            &[
                ("bob.hydentity", 0),
                ("amm2", 50),
                ("faucet.hydentity", hyllar2_initial_total_amount - 50),
            ],
        )
        .await?;
        //////////////////////////////////////////////////////////////////////

        /////////////////////// Bob actually swaps //////////////////////////
        info!("➡️  Sending blob for bob to swap 5 hyllar for 10 hyllar2");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "bob.hydentity".to_string(),
                        nonce: 3,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    AmmAction::Swap {
                        pair: ("hyllar".to_string(), "hyllar2".to_string()),
                        amounts: (5, 10),
                    }
                    .as_blob(
                        ContractName(AMM_CONTRACT_NAME.to_owned()),
                        None,
                        Some(vec![BlobIndex(2), BlobIndex(3)]),
                    ),
                    ERC20Action::TransferFrom {
                        sender: "bob.hydentity".to_string(),
                        recipient: AMM_CONTRACT_NAME.to_string(),
                        amount: 5,
                    }
                    .as_blob(
                        ContractName("hyllar".to_owned()),
                        Some(BlobIndex(1)),
                        None,
                    ),
                    ERC20Action::Transfer {
                        recipient: "bob.hydentity".to_string(),
                        amount: 10,
                    }
                    .as_blob(
                        ContractName("hyllar2".to_owned()),
                        Some(BlobIndex(1)),
                        None,
                    ),
                ],
            )
            .await?;

        proof_generator
            .generate_proof(
                &ctx,
                CliCommand::Amm {
                    command: hyrun::AmmArgs::Swap {
                        token_a: "hyllar".to_owned(),
                        token_b: "hyllar2".to_owned(),
                        amount_a: 5,
                        amount_b: 10,
                    },
                    amm_contract_name: "amm2".to_string(),
                },
                "bob.hydentity",
                "password",
                Some(3),
            )
            .await;

        let hydentity_proof = proof_generator.read_proof(0);
        let bob_swap_proof = proof_generator.read_proof(1);
        let bob_transfer_proof = proof_generator.read_proof(2);
        let amm_transfer_from_proof = proof_generator.read_proof(3);

        let recursive_proof = proof_generator
            .generate_recursive_proof(
                &[
                    hyle_contracts::HYDENTITY_ID,
                    hyle_contracts::AMM_ID,
                    hyle_contracts::HYLLAR_ID,
                    hyle_contracts::HYLLAR_ID,
                ],
                &[
                    &hydentity_proof,
                    &bob_swap_proof,
                    &bob_transfer_proof,
                    &amm_transfer_from_proof,
                ],
            )
            .await;

        info!("➡️  Sending recursive proof for hydentity, amm, hyllar and hyllar2");
        ctx.send_proof(
            "risc0-recursion".into(),
            ProofData::Bytes(recursive_proof),
            vec![
                blob_tx_hash.clone(),
                blob_tx_hash.clone(),
                blob_tx_hash.clone(),
                blob_tx_hash.clone(),
            ],
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        assert_multiple_balances(
            &ctx,
            "hyllar",
            &[
                ("bob.hydentity", 0),
                ("amm2", 25),
                ("faucet.hydentity", hyllar_initial_total_amount - 25),
            ],
        )
        .await?;

        assert_multiple_balances(
            &ctx,
            "hyllar2",
            &[
                ("bob.hydentity", 10),
                ("amm2", 40),
                ("faucet.hydentity", hyllar2_initial_total_amount - 50),
            ],
        )
        .await?;
        /////////////////////////////////////////////////////////////////////
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn amm_single_node() -> Result<()> {
        let ctx = E2ECtx::new_single(300).await?;
        scenario_amm(ctx).await
    }

    #[test_log::test(tokio::test)]
    async fn amm_multi_nodes() -> Result<()> {
        let ctx = E2ECtx::new_multi(2, 300).await?;

        scenario_amm(ctx).await
    }
}
