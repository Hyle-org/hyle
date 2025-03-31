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
    use tokio::sync::Mutex;

    use crate::fixtures::{
        ctx_turmoil::E2ETurmoilCtx,
        test_helpers_turmoil::{wait_height, MySeedableRng},
    };

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
        .into()
    }

    #[test_log::test]
    fn turmoil_test() -> Result<()> {
        let rng = MySeedableRng::new(123);
        let mut sim = hyle_net::turmoil::Builder::new()
            .simulation_duration(Duration::from_secs(100))
            // .min_message_latency(Duration::from_secs(1))
            // .max_message_latency(Duration::from_secs(5))
            // .fail_rate(0.9)
            .tick_duration(Duration::from_millis(100))
            .enable_tokio_io()
            .build_with_rng(Box::new(rng));

        let ctx = E2ETurmoilCtx::new_multi(4, 500)?;

        let mut nodes = ctx.nodes.clone();
        nodes.reverse();

        let client = clients.leak().first().unwrap();

        let tcp_address;
        let turmoil_node = nodes.pop().unwrap();
        {
            let id = turmoil_node.conf.id.clone();
            tcp_address = format!("{}:{}", id, turmoil_node.conf.tcp_server_port);
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

        sim.client("client", async move {
            _ = wait_height(client, 1).await;

            // let mut client =
            //     codec_tcp_server::connect("client-turmoil".to_string(), tcp_address.to_string())
            //         .await
            //         .unwrap();

            let mut i = 0;
            loop {
                i += 1;

                // info!("client iteration");
                // info!("p2p port {}", addr);
                tokio::time::sleep(Duration::from_millis(1000)).await;

                let tx = make_register_contract_tx(format!("contract-{}", i).into());

                _ = log_error!(client.send_tx_blob(&tx).await, "Sending tx blob");
            }
        });
        sim.run()?;

        Ok(())
    }
}
