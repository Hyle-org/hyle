#![allow(clippy::unwrap_used, clippy::expect_used)]

use hyle::log_error;

use anyhow::Result;
use fixtures::ctx::E2ECtx;

mod fixtures;

mod e2e_consensus {

    use client_sdk::helpers::risc0::Risc0Prover;
    use client_sdk::rest_client::{IndexerApiHttpClient, NodeApiHttpClient};
    use client_sdk::transaction_builder::{ProvableBlobTx, TxExecutor, TxExecutorBuilder};
    use fixtures::test_helpers::send_transaction;
    use hydentity::client::{register_identity, verify_identity};
    use hydentity::Hydentity;
    use hyle::genesis::States;
    use hyle_contract_sdk::HyleContract;
    use hyle_contract_sdk::Identity;
    use hyle_contracts::{HYDENTITY_ELF, HYLLAR_ELF, STAKING_ELF};
    use hyle_model::{ContractName, StateCommitment};
    use hyllar::client::transfer;
    use hyllar::erc20::ERC20;
    use hyllar::{Hyllar, FAUCET_ID};
    use staking::client::{delegate, stake};
    use staking::state::Staking;
    use tracing::{info, warn};

    use super::*;

    #[test_log::test(tokio::test)]
    async fn single_node_generates_blocks() -> Result<()> {
        let ctx = E2ECtx::new_single(50).await?;
        ctx.wait_height(20).await?;
        Ok(())
    }

    #[ignore = "flakky"]
    #[test_log::test(tokio::test)]
    async fn can_run_lot_of_nodes() -> Result<()> {
        let ctx = E2ECtx::new_multi(10, 1000).await?;

        ctx.wait_height(2).await?;

        Ok(())
    }

    async fn scenario_rejoin_common(ctx: &mut E2ECtx, stake_amount: u128) -> Result<()> {
        ctx.wait_height(2).await?;

        let joining_client = ctx.add_node().await?;

        let node_info = joining_client.get_node_info().await?;

        assert!(node_info.pubkey.is_some());

        let hyllar: Hyllar = log_error!(
            ctx.indexer_client()
                .fetch_current_state(&"hyllar".into())
                .await,
            "fetch state failed"
        )
        .unwrap();
        let hydentity: Hydentity = ctx
            .indexer_client()
            .fetch_current_state(&"hydentity".into())
            .await?;

        let staking_state: StateCommitment = StateCommitment(
            ctx.indexer_client()
                .get_indexer_contract(&"staking".into())
                .await?
                .state_commitment,
        );

        let staking: Staking = ctx
            .client()
            .get_consensus_staking_state()
            .await
            .unwrap()
            .into();

        assert_eq!(staking_state, staking.commit());
        let states = States {
            hyllar,
            hydentity,
            staking,
        };

        let mut tx_ctx = TxExecutorBuilder::new(states)
            // Replace prover binaries for non-reproducible mode.
            .with_prover("hydentity".into(), Risc0Prover::new(HYDENTITY_ELF))
            .with_prover("hyllar".into(), Risc0Prover::new(HYLLAR_ELF))
            .with_prover("staking".into(), Risc0Prover::new(STAKING_ELF))
            .build();

        let node_identity = Identity(format!("{}.hydentity", node_info.id));
        {
            let mut transaction = ProvableBlobTx::new(node_identity.clone());

            register_identity(&mut transaction, "hydentity".into(), "password".to_owned())?;

            let tx_hash = send_transaction(ctx.client(), transaction, &mut tx_ctx).await;
            tracing::warn!("Register TX Hash: {:?}", tx_hash);
        }
        {
            let mut transaction = ProvableBlobTx::new(FAUCET_ID.into());

            verify_identity(
                &mut transaction,
                "hydentity".into(),
                &tx_ctx.hydentity,
                "password".to_string(),
            )
            .expect("verify_identity failed");

            transfer(
                &mut transaction,
                "hyllar".into(),
                node_identity.0.clone(),
                stake_amount,
            )
            .expect("transfer failed");

            let tx_hash = send_transaction(ctx.client(), transaction, &mut tx_ctx).await;
            tracing::warn!("Transfer TX Hash: {:?}", tx_hash);
        }
        {
            let mut transaction = ProvableBlobTx::new(node_identity.clone());

            verify_identity(
                &mut transaction,
                "hydentity".into(),
                &tx_ctx.hydentity,
                "password".to_string(),
            )?;

            stake(&mut transaction, ContractName::new("staking"), stake_amount)?;

            transfer(
                &mut transaction,
                "hyllar".into(),
                "staking".to_string(),
                stake_amount,
            )?;

            delegate(&mut transaction, node_info.pubkey.clone().unwrap())?;

            let tx_hash = send_transaction(ctx.client(), transaction, &mut tx_ctx).await;
            tracing::warn!("staking TX Hash: {:?}", tx_hash);
        }

        // 2 slots to get the tx in a blocks
        // 1 slot to send the candidacy
        // 1 slot to add the validator to consensus
        ctx.wait_height(4).await?;

        let consensus = ctx.client().get_consensus_info().await?;

        assert_eq!(consensus.validators.len(), 3, "expected 3 validators");

        assert!(
            consensus.validators.contains(&node_info.pubkey.unwrap()),
            "node pubkey not found in validators",
        );

        ctx.wait_height(1).await?;
        Ok(())
    }

    pub async fn gen_txs(
        client: &NodeApiHttpClient,
        tx_ctx: &mut TxExecutor<States>,
        id: String,
        amount: u128,
    ) -> Result<()> {
        let identity = Identity(format!("{}.hydentity", id));
        {
            let mut transaction = ProvableBlobTx::new(identity.clone());

            register_identity(&mut transaction, "hydentity".into(), "password".to_owned())?;

            let tx_hash = send_transaction(client, transaction, tx_ctx).await;
            tracing::warn!("Register TX Hash: {:?}", tx_hash);
        }

        {
            let mut transaction = ProvableBlobTx::new("faucet.hydentity".into());

            verify_identity(
                &mut transaction,
                "hydentity".into(),
                &tx_ctx.hydentity,
                "password".to_string(),
            )?;

            transfer(
                &mut transaction,
                "hyllar".into(),
                identity.0.clone(),
                amount,
            )?;

            let tx_hash = send_transaction(client, transaction, tx_ctx).await;
            tracing::warn!("Transfer TX Hash: {:?}", tx_hash);
        }

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn can_rejoin_blocking_consensus() -> Result<()> {
        let mut ctx = E2ECtx::new_multi_with_indexer(2, 500).await?;

        scenario_rejoin_common(&mut ctx, 100).await?;

        // TODO: we should be able to exit the consensus and rejoin it again, but this doesn't work when we block it for now.
        /*
        info!("Stopping node");
        ctx.stop_node(3).await?;

        // Wait for a few seconds
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        info!("Experienced timeouts");
        //info!("Waiting for a few blocks");
        //ctx.wait_height(2).await?;

        // We restart during the timeout timer which is fine.
        ctx.restart_node(3)?;

        ctx.wait_height(4).await?;
        */
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn can_rejoin_not_blocking_consensus() -> Result<()> {
        let mut ctx = E2ECtx::new_multi_with_indexer(2, 500).await?;

        scenario_rejoin_common(&mut ctx, 50).await?;

        info!("Stopping node");
        ctx.stop_node(3).await?;

        info!("Waiting for a few blocks");
        ctx.wait_height(2).await?;

        // We restart during the timeout timer which is fine.
        ctx.restart_node(3)?;

        ctx.wait_height(1).await?;

        Ok(())
    }

    pub async fn init_states(indexer_client: &IndexerApiHttpClient) -> TxExecutor<States> {
        let hyllar: Hyllar = indexer_client
            .fetch_current_state(&"hyllar".into())
            .await
            .unwrap();
        let hydentity: Hydentity = indexer_client
            .fetch_current_state(&"hydentity".into())
            .await
            .unwrap();

        let states = States {
            hyllar,
            hydentity,
            staking: Staking::default(),
        };

        TxExecutorBuilder::new(states)
            // Replace prover binaries for non-reproducible mode.
            .with_prover("hydentity".into(), Risc0Prover::new(HYDENTITY_ELF))
            .with_prover("hyllar".into(), Risc0Prover::new(HYLLAR_ELF))
            .build()
    }

    #[test_log::test(tokio::test)]
    async fn can_restart_single_node_after_txs() -> Result<()> {
        let mut ctx = E2ECtx::new_single_with_indexer(500).await?;

        _ = ctx.wait_height(1).await;

        // Gen a few txs
        let states = States {
            hyllar: Hyllar::default(),
            hydentity: Hydentity::default(),
            staking: Staking::default(),
        };

        let mut tx_ctx = TxExecutorBuilder::new(states)
            // Replace prover binaries for non-reproducible mode.
            .with_prover("hydentity".into(), Risc0Prover::new(HYDENTITY_ELF))
            .with_prover("hyllar".into(), Risc0Prover::new(HYLLAR_ELF))
            .build();

        warn!("Starting generating txs");

        for i in 0..6 {
            _ = gen_txs(ctx.client(), &mut tx_ctx, format!("alex{}", i), 100 + i).await;
        }

        ctx.stop_node(0).await?;
        ctx.restart_node(0)?;

        ctx.wait_height(0).await?;

        for i in 6..10 {
            _ = gen_txs(ctx.client(), &mut tx_ctx, format!("alex{}", i), 100 + i).await;
        }

        ctx.wait_height(2).await?;

        let state: Hyllar = ctx
            .indexer_client()
            .fetch_current_state(&ContractName::new("hyllar"))
            .await?;

        for i in 0..10 {
            let balance = state.balance_of(&format!("alex{}.hydentity", i));
            info!("Checking alex{}.hydentity balance: {:?}", i, balance);
            assert_eq!(balance.unwrap(), ((100 + i) as u128));
        }

        Ok(())
    }
}

use rand::{rngs::StdRng, RngCore, SeedableRng};

/// Structure personnalisée qui encapsule un RNG seedable
pub struct MySeedableRng {
    rng: StdRng, // Utilisation d'un générateur déterministe basé sur une seed
}

impl MySeedableRng {
    /// Constructeur : Initialise un RNG avec une seed donnée
    pub fn new(seed: u64) -> Self {
        MySeedableRng {
            rng: StdRng::seed_from_u64(seed),
        }
    }
}

impl RngCore for MySeedableRng {
    /// Génère un nombre aléatoire de 32 bits
    fn next_u32(&mut self) -> u32 {
        self.rng.next_u32()
    }

    /// Génère un nombre aléatoire de 64 bits
    fn next_u64(&mut self) -> u64 {
        self.rng.next_u64()
    }

    /// Remplit un buffer de bytes avec des valeurs aléatoires
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.rng.fill_bytes(dest)
    }

    /// Remplit un buffer de bytes de manière non déterministe (optionnel)
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        self.rng.try_fill_bytes(dest)
    }
}

#[cfg(feature = "turmoil")]
#[cfg(test)]
mod turmoil_tests {
    use std::{net::Ipv4Addr, sync::Arc, time::Duration};

    use anyhow::Context;
    use client_sdk::{
        helpers::risc0::Risc0Prover,
        tcp::{codec_tcp_server, TcpServerMessage},
        transaction_builder::TxExecutorBuilder,
    };
    use futures::TryFutureExt;
    use hydentity::Hydentity;
    use hyle::{genesis::States, log_error};
    use hyle_contract_sdk::info;
    use hyle_contracts::{HYDENTITY_ELF, HYLLAR_ELF};
    use hyle_model::{
        BlobTransaction, ContractAction, ContractName, ProgramId, RegisterContractAction,
        StateCommitment, Transaction,
    };
    use hyllar::{erc20::ERC20, Hyllar};
    use rand::{RngCore, SeedableRng};
    use staking::state::Staking;
    use tokio::sync::Mutex;
    use tracing::{warn, Instrument};
    use turmoil::{net::TcpStream, Result, Sim};

    use crate::{
        e2e_consensus::{gen_txs, init_states},
        fixtures::{ctx::E2ETurmoilCtx, test_helpers::wait_height},
        MySeedableRng,
    };
    pub fn make_register_contract_tx(name: ContractName) -> Transaction {
        BlobTransaction::new(
            "hyle.hyle",
            vec![RegisterContractAction {
                verifier: "test".into(),
                program_id: ProgramId(vec![]),
                state_commitment: StateCommitment(vec![0, 1, 2, 3]),
                contract_name: name,
            }
            .as_blob("hyle".into(), None, None)],
        )
        .into()
    }

    #[test_log::test]
    fn turmoil_test() -> Result<()> {
        let rng = MySeedableRng::new(123);
        let mut sim = turmoil::Builder::new()
            .simulation_duration(Duration::from_secs(120))
            .min_message_latency(Duration::from_secs(1))
            .max_message_latency(Duration::from_secs(5))
            // .fail_rate(0.9)
            .tick_duration(Duration::from_millis(100))
            .enable_tokio_io()
            .build_with_rng(Box::new(rng));

        let mut ctx = E2ETurmoilCtx::new_multi(4, 500)?;

        let mut nodes = ctx.nodes.clone();
        nodes.reverse();

        let tcp_address;
        let turmoil_node = nodes.pop().unwrap();
        {
            let id = turmoil_node.conf.id.clone();
            tcp_address = turmoil_node.conf.tcp_address.clone().unwrap();
            let cloned = Arc::new(Mutex::new(turmoil_node.clone())); // Permet de partager la variable

            let f = {
                let cloned = Arc::clone(&cloned); // Clonage pour éviter de déplacer
                move || {
                    let cloned = Arc::clone(&cloned);
                    async move {
                        let mut node = cloned.lock().await; // Accès mutable au nœud
                        node.start().await;
                        Ok(())
                    }
                    .instrument(tracing::info_span!("server-node-1"))
                }
            };

            sim.host(id, f);
        }
        let turmoil_node = nodes.pop().unwrap();
        {
            let id = turmoil_node.conf.id.clone();
            let cloned = Arc::new(Mutex::new(turmoil_node.clone())); // Permet de partager la variable

            let f = {
                let cloned = Arc::clone(&cloned); // Clonage pour éviter de déplacer
                move || {
                    let cloned = Arc::clone(&cloned);
                    async move {
                        let mut node = cloned.lock().await; // Accès mutable au nœud
                        node.start().await;
                        Ok(())
                    }
                    .instrument(tracing::info_span!("server-node-2"))
                }
            };

            sim.host(id, f);
        }
        let turmoil_node = nodes.pop().unwrap();
        {
            let id = turmoil_node.conf.id.clone();
            let cloned = Arc::new(Mutex::new(turmoil_node.clone())); // Permet de partager la variable

            let f = {
                let cloned = Arc::clone(&cloned); // Clonage pour éviter de déplacer
                move || {
                    let cloned = Arc::clone(&cloned);
                    async move {
                        let mut node = cloned.lock().await; // Accès mutable au nœud
                        node.start().await;
                        Ok(())
                    }
                    .instrument(tracing::info_span!("server-node-3"))
                }
            };

            sim.host(id, f);
        }

        let turmoil_node = nodes.pop().unwrap();
        {
            let id = turmoil_node.conf.id.clone();
            let cloned = Arc::new(Mutex::new(turmoil_node.clone())); // Permet de partager la variable

            let f = {
                let cloned = Arc::clone(&cloned); // Clonage pour éviter de déplacer
                move || {
                    let cloned = Arc::clone(&cloned);
                    async move {
                        let mut node = cloned.lock().await; // Accès mutable au nœud
                        node.start().await;
                        Ok(())
                    }
                    .instrument(tracing::info_span!("server-node-4"))
                }
            };

            sim.host(id, f);
        }

        sim.client("client", async move {
            // _ = ctx.wait_height(1).await;
            // let states = States {
            //     hyllar: Hyllar::default(),
            //     hydentity: Hydentity::default(),
            //     staking: Staking::default(),
            // };

            // let mut tx_ctx = TxExecutorBuilder::new(states)
            //     // Replace prover binaries for non-reproducible mode.
            //     .with_prover("hydentity".into(), Risc0Prover::new(HYDENTITY_ELF))
            //     .with_prover("hyllar".into(), Risc0Prover::new(HYLLAR_ELF))
            //     .build();

            // warn!("Starting generating txs");

            // for i in 0..6 {
            //     _ = gen_txs(ctx.client(), &mut tx_ctx, format!("alex{}", i), 100 + i).await;
            // }

            // ctx.wait_height(5).await?;
            //

            // let state: Hyllar = ctx
            //     .indexer_client()
            //     .fetch_current_state(&ContractName::new("hyllar"))
            //     .await?;

            // for i in 0..10 {
            //     let balance = state.balance_of(&format!("alex{}.hydentity", i));
            //     info!("Checking alex{}.hydentity balance: {:?}", i, balance);
            //     assert_eq!(balance.unwrap(), ((100 + i) as u128));
            // }
            //

            tokio::time::sleep(Duration::from_secs(5)).await;

            let mut client =
                codec_tcp_server::connect("client-turmoil".to_string(), tcp_address.to_string())
                    .await
                    .unwrap();

            let mut i = 0;
            loop {
                i += 1;

                // info!("client iteration");
                // info!("p2p port {}", addr);
                tokio::time::sleep(Duration::from_millis(1000)).await;

                let tx = make_register_contract_tx(format!("contract-{}", i).into());

                _ = client.send(TcpServerMessage::NewTx(tx)).await;
            }
        });

        // sim.set_link_latency("node-1", "node-3", Duration::from_secs(1));
        // sim.set_link_latency("node-1", "node-2", Duration::from_secs(1));
        // sim.set_link_latency("node-3", "node-2", Duration::from_secs(1));
        // sim.set_link_latency("node-3", "node-4", Duration::from_secs(1));
        // sim.set_link_latency("node-4", "node-2", Duration::from_secs(1));
        // sim.set_link_latency("node-4", "node-1", Duration::from_secs(1));

        // sim.set_link_latency("node-1", "node-3", Duration::from_secs(2));
        // sim.set_link_latency("node-1", "node-2", Duration::from_secs(2));
        // sim.set_link_latency("node-3", "node-2", Duration::from_secs(2));
        // sim.set_link_latency("node-3", "node-4", Duration::from_secs(2));
        // sim.set_link_latency("node-4", "node-2", Duration::from_secs(2));
        // sim.set_link_latency("node-4", "node-1", Duration::from_secs(2));

        sim.run()?;

        Ok(())
    }
}
