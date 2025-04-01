mod fixtures;

#[cfg(feature = "turmoil")]
#[cfg(test)]
mod turmoil_tests {
    use std::{sync::Arc, time::Duration};

    use hyle::log_error;
    use hyle_model::{
        BlobTransaction, ContractAction, ContractName, ProgramId, RegisterContractAction,
        StateCommitment,
    };
    use hyle_net::net::Sim;
    use rand::{rngs::StdRng, SeedableRng};
    use tokio::sync::Mutex;

    use crate::fixtures::{test_helpers::wait_height, turmoil::E2ETurmoilCtx};

    pub fn make_register_contract_tx(name: ContractName) -> BlobTransaction {
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
    }

    fn setup_simulation(ctx: &E2ETurmoilCtx, sim: &mut Sim<'_>) -> anyhow::Result<()> {
        let mut nodes = ctx.nodes.clone();
        nodes.reverse();

        let turmoil_node = nodes.pop().unwrap();
        {
            let id = turmoil_node.conf.id.clone();
            let cloned = Arc::new(Mutex::new(turmoil_node.clone())); // Permet de partager la variable

            let f = {
                let cloned = Arc::clone(&cloned); // Clonage pour éviter de déplacer
                move || {
                    let cloned = Arc::clone(&cloned);
                    async move {
                        let node = cloned.lock().await; // Accès mutable au nœud
                        _ = node.start().await;
                        Ok(())
                    }
                }
            };

            sim.host(id, f);
        }
        while let Some(turmoil_node) = nodes.pop() {
            let id = turmoil_node.conf.id.clone();
            let cloned = Arc::new(Mutex::new(turmoil_node.clone())); // Permet de partager la variable

            let f = {
                let cloned = Arc::clone(&cloned); // Clonage pour éviter de déplacer
                move || {
                    let cloned = Arc::clone(&cloned);
                    async move {
                        let node = cloned.lock().await; // Accès mutable au nœud
                        _ = node.start().await;
                        Ok(())
                    }
                }
            };

            sim.host(id, f);
        }
        Ok(())
    }

    #[test_log::test]
    fn turmoil_test() -> anyhow::Result<()> {
        let rng = StdRng::seed_from_u64(123);
        let mut sim = hyle_net::turmoil::Builder::new()
            .simulation_duration(Duration::from_secs(1000))
            .min_message_latency(Duration::from_secs(1))
            .max_message_latency(Duration::from_secs(10))
            // .fail_rate(0.9)
            .tick_duration(Duration::from_millis(100))
            .enable_tokio_io()
            .build_with_rng(Box::new(rng));

        let ctx = E2ETurmoilCtx::new_multi(4, 500)?;

        setup_simulation(&ctx, &mut sim)?;

        let client = ctx.client();

        sim.client("client", async move {
            for i in 1..50 {
                _ = wait_height(&client, 1).await;

                let tx = make_register_contract_tx(format!("contract-{}", i).into());

                _ = log_error!(client.send_tx_blob(&tx).await, "Sending tx blob");
            }
            for i in 1..50 {
                let contract = ctx.get_contract(format!("contract-{}", i).as_str()).await?;
                assert_eq!(contract.name, format!("contract-{}", i).as_str().into());
            }

            Ok(())
        });

        _ = sim.run();

        Ok(())
    }
}
