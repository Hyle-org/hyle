#![allow(clippy::unwrap_used, clippy::expect_used)]
use fixtures::ctx::E2ECtx;
use tracing::info;

use hyle::model::ProofData;

mod fixtures;

use anyhow::Result;

mod e2e_amm {
    use amm::{
        client::{new_pair, swap},
        AmmState,
    };
    use client_sdk::{
        contract_states,
        helpers::risc0::Risc0Prover,
        transaction_builder::{ProvableBlobTx, TxExecutorBuilder},
    };
    use fixtures::{
        contracts::{AmmContract, HyllarContract},
        ctx::E2EContract,
        proofs::generate_recursive_proof,
    };
    use hydentity::{
        client::{register_identity, verify_identity},
        Hydentity,
    };
    use hyle_contract_sdk::{erc20::ERC20, ContractName, Digestable, StateDigest};
    use hyle_contracts::{AMM_ELF, HYDENTITY_ELF, HYLLAR_ELF};
    use hyllar::{
        client::{approve, transfer},
        HyllarToken,
    };

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

    contract_states!(
        struct States {
            hydentity: Hydentity,
            hyllar: HyllarToken,
            hyllar2: HyllarToken,
            amm: AmmState,
        }
    );

    async fn scenario_amm(ctx: E2ECtx) -> Result<()> {
        // Here is the flow that we are going to test:
        // Register bob in hydentity

        // Send 25 hyllar from faucet to bob
        // Send 50 hyllar2 from faucet to bob

        // Register new amm contract "amm"

        // Bob approves 100 hyllar to amm2
        // Bob approves 100 hyllar2 to amm2

        // Bob registers a new pair in amm
        //    By sending 20 hyllar to amm
        //    By sending 50 hyllar2 to amm

        // Bob swaps 5 hyllar for 10 hyllar2
        //    By sending 5 hyllar to amm
        //    By sending 10 hyllar2 to bob (from amm)

        let mut executor = TxExecutorBuilder::new(States {
            hydentity: ctx.get_contract("hydentity").await?.state.try_into()?,
            hyllar: ctx.get_contract("hyllar").await?.state.try_into()?,
            hyllar2: HyllarContract::state_digest().try_into()?,
            amm: AmmState::default(),
        })
        .with_prover("hydentity".into(), Risc0Prover::new(HYDENTITY_ELF))
        .with_prover("hyllar".into(), Risc0Prover::new(HYLLAR_ELF))
        .with_prover("hyllar2".into(), Risc0Prover::new(HYLLAR_ELF))
        .with_prover("amm".into(), Risc0Prover::new(AMM_ELF))
        .build();

        let state = hyllar::HyllarTokenContract::init(executor.hyllar.clone(), "caller".into());
        let hyllar_initial_total_amount: u128 = state
            .balance_of("faucet.hydentity")
            .expect("faucet identity not found");

        let state = hyllar::HyllarTokenContract::init(executor.hyllar2.clone(), "caller".into());
        let hyllar2_initial_total_amount: u128 = state
            .balance_of("faucet.hydentity")
            .expect("faucet identity not found");

        /////////////////////////////////////////////////////////////////////

        ///////////////////// hyllar2 contract registration /////////////////
        info!("➡️  Registring hyllar2 contract");
        const HYLLAR2_CONTRACT_NAME: &str = "hyllar2";
        ctx.register_contract::<HyllarContract>(HYLLAR2_CONTRACT_NAME)
            .await?;
        /////////////////////////////////////////////////////////////////////

        ///////////////////// bob identity registration /////////////////////
        info!("➡️  Sending blob to register bob identity");

        let mut tx = ProvableBlobTx::new("bob.hydentity".into());
        register_identity(&mut tx, "hydentity".into(), "password".to_string())?;
        let blob_tx_hash = ctx.send_provable_blob_tx(&tx).await?;
        let tx = executor.process(tx)?;
        let proof = tx.iter_prove().next().unwrap().0.await?;

        info!("➡️  Sending proof for register");
        ctx.send_proof_single("hydentity".into(), proof, blob_tx_hash)
            .await?;

        info!("➡️  Waiting for height 2");
        ctx.wait_height(2).await?;
        /////////////////////////////////////////////////////////////////////

        ///////////////// sending hyllar from faucet to bob /////////////////
        info!("➡️  Sending blob to transfer 25 hyllar from faucet to bob");

        let mut tx = ProvableBlobTx::new("faucet.hydentity".into());
        verify_identity(
            &mut tx,
            "hydentity".into(),
            &executor.hydentity,
            "password".into(),
        )?;
        transfer(&mut tx, "hyllar".into(), "bob.hydentity".into(), 25)?;

        let blob_tx_hash = ctx.send_provable_blob_tx(&tx).await?;
        let tx = executor.process(tx)?;
        let mut proofs = tx.iter_prove();

        let hydentity_proof = proofs.next().unwrap().0.await?;
        let bob_transfer_proof = proofs.next().unwrap().0.await?;

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single("hydentity".into(), hydentity_proof, blob_tx_hash.clone())
            .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof_single("hyllar".into(), bob_transfer_proof, blob_tx_hash)
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
        let mut tx = ProvableBlobTx::new("faucet.hydentity".into());
        verify_identity(
            &mut tx,
            "hydentity".into(),
            &executor.hydentity,
            "password".into(),
        )?;
        transfer(&mut tx, "hyllar2".into(), "bob.hydentity".into(), 50)?;

        let blob_tx_hash = ctx.send_provable_blob_tx(&tx).await?;
        let tx = executor.process(tx)?;
        let mut proofs = tx.iter_prove();

        let hydentity_proof = proofs.next().unwrap().0.await?;
        let bob_transfer_proof = proofs.next().unwrap().0.await?;

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single("hydentity".into(), hydentity_proof, blob_tx_hash.clone())
            .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof_single("hyllar2".into(), bob_transfer_proof, blob_tx_hash)
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
        const AMM_CONTRACT_NAME: &str = "amm";
        ctx.register_contract::<AmmContract>(AMM_CONTRACT_NAME)
            .await?;
        /////////////////////////////////////////////////////////////////////

        //////////////////// Bob approves AMM on hyllar /////////////////////
        info!("➡️  Sending blob to approve amm on hyllar");
        let mut tx = ProvableBlobTx::new("bob.hydentity".into());
        verify_identity(
            &mut tx,
            "hydentity".into(),
            &executor.hydentity,
            "password".into(),
        )?;
        approve(&mut tx, "hyllar".into(), AMM_CONTRACT_NAME.into(), 100)?;

        let blob_tx_hash = ctx.send_provable_blob_tx(&tx).await?;
        let tx = executor.process(tx)?;
        let mut proofs = tx.iter_prove();

        let hydentity_proof = proofs.next().unwrap().0.await?;
        let bob_approve_hyllar_proof = proofs.next().unwrap().0.await?;

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single("hydentity".into(), hydentity_proof, blob_tx_hash.clone())
            .await?;

        info!("➡️  Sending proof for approve hyllar");
        ctx.send_proof_single(
            "hyllar".into(),
            bob_approve_hyllar_proof,
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        assert_account_allowance(&ctx, "hyllar", "bob.hydentity", AMM_CONTRACT_NAME, 100).await?;
        /////////////////////////////////////////////////////////////////////

        //////////////////// Bob approves AMM on hyllar2 /////////////////////
        info!("➡️  Sending blob to approve amm on hyllar2");

        let mut tx = ProvableBlobTx::new("bob.hydentity".into());
        verify_identity(
            &mut tx,
            "hydentity".into(),
            &executor.hydentity,
            "password".into(),
        )?;
        approve(&mut tx, "hyllar2".into(), AMM_CONTRACT_NAME.into(), 100)?;

        let blob_tx_hash = ctx.send_provable_blob_tx(&tx).await?;
        let tx = executor.process(tx)?;
        let mut proofs = tx.iter_prove();

        let hydentity_proof = proofs.next().unwrap().0.await?;
        let bob_approve_hyllar2_proof = proofs.next().unwrap().0.await?;

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single("hydentity".into(), hydentity_proof, blob_tx_hash.clone())
            .await?;

        info!("➡️  Sending proof for approve hyllar2");
        ctx.send_proof_single(
            "hyllar2".into(),
            bob_approve_hyllar2_proof,
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        assert_account_allowance(&ctx, "hyllar2", "bob.hydentity", AMM_CONTRACT_NAME, 100).await?;
        /////////////////////////////////////////////////////////////////////

        /////////////// Creating new pair hyllar/hyllar2 on amm ///////////////
        info!("➡️  Creating new pair hyllar/hyllar2 on amm");

        let mut tx = ProvableBlobTx::new("bob.hydentity".into());
        verify_identity(
            &mut tx,
            "hydentity".into(),
            &executor.hydentity,
            "password".into(),
        )?;

        new_pair(
            &mut tx,
            AMM_CONTRACT_NAME.into(),
            ("hyllar".into(), "hyllar2".into()),
            (20, 50),
        )?;

        let blob_tx_hash = ctx.send_provable_blob_tx(&tx).await?;
        let tx = executor.process(tx)?;
        let mut proofs = tx.iter_prove();

        let hydentity_proof = proofs.next().unwrap().0.await?;
        let bob_new_pair_proof = proofs.next().unwrap().0.await?;
        let bob_transfer_hyllar_proof = proofs.next().unwrap().0.await?;
        let bob_transfer_hyllar2_proof = proofs.next().unwrap().0.await?;

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single("hydentity".into(), hydentity_proof, blob_tx_hash.clone())
            .await?;

        info!("➡️  Sending proof for new pair");
        ctx.send_proof_single(
            AMM_CONTRACT_NAME.into(),
            bob_new_pair_proof,
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof_single(
            "hyllar".into(),
            bob_transfer_hyllar_proof,
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for hyllar2");
        ctx.send_proof_single("hyllar2".into(), bob_transfer_hyllar2_proof, blob_tx_hash)
            .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        assert_multiple_balances(
            &ctx,
            "hyllar",
            &[
                ("bob.hydentity", 5),
                (AMM_CONTRACT_NAME, 20),
                ("faucet.hydentity", hyllar_initial_total_amount - 25),
            ],
        )
        .await?;

        assert_multiple_balances(
            &ctx,
            "hyllar2",
            &[
                ("bob.hydentity", 0),
                (AMM_CONTRACT_NAME, 50),
                ("faucet.hydentity", hyllar2_initial_total_amount - 50),
            ],
        )
        .await?;
        //////////////////////////////////////////////////////////////////////

        /////////////////////// Bob actually swaps //////////////////////////
        info!("➡️ Bob actually swaps");

        let mut tx = ProvableBlobTx::new("bob.hydentity".into());
        verify_identity(
            &mut tx,
            "hydentity".into(),
            &executor.hydentity,
            "password".into(),
        )?;

        swap(
            &mut tx,
            AMM_CONTRACT_NAME.into(),
            ("hyllar".into(), "hyllar2".into()),
            (5, 10),
        )?;

        let blob_tx_hash = ctx.send_provable_blob_tx(&tx).await?;
        let tx = executor.process(tx)?;
        let mut proofs = tx.iter_prove();

        let hydentity_proof = proofs.next().unwrap().0.await?.0;
        let bob_swap_proof = proofs.next().unwrap().0.await?.0;
        let bob_transfer_proof = proofs.next().unwrap().0.await?.0;
        let amm_transfer_from_proof = proofs.next().unwrap().0.await?.0;

        let recursive_proof = generate_recursive_proof(
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
            ProofData(recursive_proof),
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
                (AMM_CONTRACT_NAME, 25),
                ("faucet.hydentity", hyllar_initial_total_amount - 25),
            ],
        )
        .await?;

        assert_multiple_balances(
            &ctx,
            "hyllar2",
            &[
                ("bob.hydentity", 10),
                (AMM_CONTRACT_NAME, 40),
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
