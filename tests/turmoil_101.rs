#![allow(clippy::all)]
#![cfg(feature = "turmoil")]
#![cfg(test)]

mod fixtures;

use std::time::Duration;

use fixtures::turmoil::TurmoilNodeProcess;
use hyle::log_error;
use hyle_model::{
    BlobTransaction, ContractAction, ContractName, ProgramId, RegisterContractAction,
    StateCommitment,
};
use hyle_net::net::Sim;
use rand::{rngs::StdRng, SeedableRng};

use crate::fixtures::{test_helpers::wait_height, turmoil::TurmoilCtx};

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

macro_rules! turmoil_simple {
    ($seed:literal, $simulation:ident, $test:ident) => {
        paste::paste! {
        #[test_log::test]
            fn [<turmoil_ $simulation _ $seed _ $test>]() -> anyhow::Result<()> {
                tracing::info!("Starting test {} with seed {}", stringify!([<turmoil_ $simulation _ $seed _ $test>]), $seed);
                let rng = StdRng::seed_from_u64($seed);
                let mut sim = hyle_net::turmoil::Builder::new()
                    .simulation_duration(Duration::from_secs(100))
                    .tick_duration(Duration::from_millis(20))
                    .min_message_latency(Duration::from_millis(20))
                    .max_message_latency(Duration::from_millis(500))
                    .enable_tokio_io()
                    .build_with_rng(Box::new(rng));

                sim.set_message_latency_curve(0.025);

                // min = 20, max = 500, lambda = 0.025
                // min = 50, max = 1000, lambda = 0.01

                let mut ctx = TurmoilCtx::new_multi(4, 500, $seed, &mut sim)?;

                for node in ctx.nodes.iter() {
                    let cloned_node = node.clone();
                    sim.client(format!("client {}", node.conf.id.clone()), async move {
                        _ = $test(cloned_node).await?;
                        Ok(())
                    });
                }

                $simulation(&mut ctx, &mut sim)?;

                ctx.clean()?;

                Ok(())
            }
        }
    };

    ($seed_from:literal..=$seed_to:literal, $simulation:ident, $test:ident) => {
        seq_macro::seq!(SEED in $seed_from..=$seed_to {
            turmoil_simple!(SEED, $simulation, $test);
        });
    };
}

// turmoil_simple!(401..=420, simulation_basic, submit_10_contracts);
// turmoil_simple!(501..=520, simulation_slow_node, submit_10_contracts);
// turmoil_simple!(501..=520, simulation_two_slow_nodes, submit_10_contracts);
turmoil_simple!(501..=520, simulation_slow_network, submit_10_contracts);
// turmoil_simple!(501..=520, simulation_hold, submit_10_contracts);
// turmoil_simple!(601..=620, simulation_one_more_node, submit_10_contracts);

pub fn simulation_slow_network(ctx: &mut TurmoilCtx, sim: &mut Sim<'_>) -> anyhow::Result<()> {
    for node in ctx.nodes.clone().iter() {
        for other_node in ctx
            .nodes
            .clone()
            .iter()
            .filter(|n| n.conf.id != node.conf.id)
        {
            let slowness = Duration::from_millis(ctx.random_between(250, 1500));
            sim.set_link_latency(node.conf.id.clone(), other_node.conf.id.clone(), slowness);
        }
    }

    sim.set_message_latency_curve(0.01);

    loop {
        let is_finished = sim.step().map_err(|s| anyhow::anyhow!(s.to_string()))?;

        if is_finished {
            tracing::info!("Time spent {}", sim.elapsed().as_millis());
            return Ok(());
        }
    }
}

pub fn simulation_slow_node(ctx: &mut TurmoilCtx, sim: &mut Sim<'_>) -> anyhow::Result<()> {
    let slow_node = ctx.random_id();

    for other_node in ctx.nodes.clone().iter().filter(|n| n.conf.id != slow_node) {
        let slowness = Duration::from_millis(ctx.random_between(150, 1500));
        sim.set_link_latency(slow_node.clone(), other_node.conf.id.clone(), slowness);
    }

    loop {
        let is_finished = sim.step().map_err(|s| anyhow::anyhow!(s.to_string()))?;

        if is_finished {
            tracing::info!("Time spent {}", sim.elapsed().as_millis());
            return Ok(());
        }
    }
}

pub fn simulation_two_slow_nodes(ctx: &mut TurmoilCtx, sim: &mut Sim<'_>) -> anyhow::Result<()> {
    let slow_node = ctx.random_id();
    let slow_node_2 = loop {
        let random2 = ctx.random_id();
        if random2 != slow_node {
            break random2;
        }
    };

    for other_node in ctx.nodes.clone().iter().filter(|n| n.conf.id != slow_node) {
        let slowness = Duration::from_millis(ctx.random_between(150, 1500));
        sim.set_link_latency(slow_node.clone(), other_node.conf.id.clone(), slowness);
    }

    for other_node in ctx
        .nodes
        .clone()
        .iter()
        .filter(|n| n.conf.id != slow_node_2)
    {
        let slowness = Duration::from_millis(ctx.random_between(150, 1500));
        sim.set_link_latency(slow_node_2.clone(), other_node.conf.id.clone(), slowness);
    }

    loop {
        let is_finished = sim.step().map_err(|s| anyhow::anyhow!(s.to_string()))?;

        if is_finished {
            tracing::info!("Time spent {}", sim.elapsed().as_millis());
            return Ok(());
        }
    }
}

pub fn simulation_hold(ctx: &mut TurmoilCtx, sim: &mut Sim<'_>) -> anyhow::Result<()> {
    let mut finished: bool;

    let from = ctx.random_id();
    let to = loop {
        let candidate = ctx.random_id();
        if candidate != from {
            break candidate;
        }
    };

    let when = ctx.random_between(5, 15);
    let duration = ctx.random_between(2, 10);

    tracing::info!(
        "Holding messages from {} to {} at {} for {} seconds",
        from,
        to,
        when,
        duration
    );

    loop {
        finished = sim.step().unwrap();

        let current_time = sim.elapsed();

        if current_time > Duration::from_secs(when)
            && current_time <= Duration::from_secs(when + duration)
        {
            sim.hold(from.clone(), to.clone());
        }

        if current_time > Duration::from_secs(when + duration) {
            sim.release(from.clone(), to.clone());
        }

        if finished {
            tracing::info!("Time spent {}", sim.elapsed().as_millis());
            return Ok(());
        }
    }
}

pub fn simulation_one_more_node(ctx: &mut TurmoilCtx, sim: &mut Sim<'_>) -> anyhow::Result<()> {
    let mut finished: bool;

    let when = ctx.random_between(5, 15);

    let mut added_nodes = 0;

    loop {
        finished = sim.step().unwrap();

        let current_time = sim.elapsed();

        if current_time > Duration::from_secs(when) && added_nodes == 0 {
            added_nodes += 1;
            let client = ctx.add_node_to_simulation(sim)?;

            sim.client("client new-node", async move {
                _ = wait_height(&client, 1).await;

                for i in 1..10 {
                    let contract = client
                        .get_contract(&format!("contract-{}", i).into())
                        .await?;
                    assert_eq!(contract.name.0, format!("contract-{}", i).as_str());
                }
                Ok(())
            })
        }

        if finished {
            tracing::info!("Time spent {}", sim.elapsed().as_millis());
            return Ok(());
        }
    }
}
pub fn simulation_basic(_ctx: &mut TurmoilCtx, sim: &mut Sim<'_>) -> anyhow::Result<()> {
    _ = sim.run();
    Ok(())
}

pub async fn submit_10_contracts(node: TurmoilNodeProcess) -> anyhow::Result<()> {
    let client = node.client;
    _ = wait_height(&client, 1).await;

    if node.conf.id == "node-1" {
        for i in 1..10 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let tx = make_register_contract_tx(format!("contract-{}", i).into());

            _ = log_error!(client.send_tx_blob(&tx).await, "Sending tx blob");
        }
    } else {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }

    for i in 1..10 {
        let contract = client
            .get_contract(&format!("contract-{}", i).into())
            .await?;
        assert_eq!(contract.name.0, format!("contract-{}", i).as_str());
    }

    Ok(())
}
