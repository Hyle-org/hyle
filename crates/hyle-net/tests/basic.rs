#![allow(clippy::all)]
#![cfg(feature = "turmoil")]
#![cfg(test)]

use std::{collections::HashSet, error::Error, time::Duration};

use hyle_crypto::BlstCrypto;
use hyle_net::{net::Sim, p2p_server_mod, tcp::P2PTcpMessage};
use rand::{rngs::StdRng, RngCore, SeedableRng};

#[derive(Clone, Debug, borsh::BorshDeserialize, borsh::BorshSerialize)]
pub struct Msg(usize);

impl Into<P2PTcpMessage<Msg>> for Msg {
    fn into(self) -> P2PTcpMessage<Msg> {
        P2PTcpMessage::Data(self)
    }
}

p2p_server_mod! {
  pub test,
  message: crate::Msg
}

macro_rules! turmoil_simple {
    ($seed:literal, $nb:literal, $simulation:ident) => {
        paste::paste! {
        #[test_log::test]
            fn [<turmoil_p2p_ $nb _nodes_ $simulation _ $seed >]() -> anyhow::Result<()> {
                tracing::info!("Starting test {} with seed {}", stringify!([<turmoil_ $simulation _ $seed >]), $seed);
                let rng = StdRng::seed_from_u64($seed);
                let mut sim = hyle_net::turmoil::Builder::new()
                    .simulation_duration(Duration::from_secs(100))
                    .tick_duration(Duration::from_millis(50))
                    .enable_tokio_io()
                    .build_with_rng(Box::new(rng));

                let mut peers = vec![];

                for i in 1..($nb+1) {
                    peers.push(format!("peer-{}", i));
                }

                $simulation(peers, &mut sim, $seed)?;

                Ok(())
            }
        }
    };

    ($seed_from:literal..=$seed_to:literal, $nb:literal, $simulation:ident) => {
        seq_macro::seq!(SEED in $seed_from..=$seed_to {
            turmoil_simple!(SEED, $nb, $simulation);
        });
    };
}

turmoil_simple!(501..=520, 4, setup_basic);
turmoil_simple!(501..=520, 10, setup_basic);
turmoil_simple!(501..=520, 4, setup_drops);
turmoil_simple!(521..=540, 10, setup_drops);

async fn setup_basic_host(
    peer: String,
    peers: Vec<String>,
    _seed: u64,
) -> Result<(), Box<dyn Error>> {
    let crypto = BlstCrypto::new(peer.clone().as_str())?;
    let mut p2p = p2p_server_test::start_server(
        std::sync::Arc::new(crypto),
        peer.clone(),
        9090,
        None,
        format!("{}:{}", peer, 9090),
        format!("{}:{}", peer, 4141),
    )
    .await?;

    let all_other_peers: HashSet<String> =
        HashSet::from_iter(peers.clone().into_iter().filter(|p| p != &peer));

    tracing::info!("All other peers {:?}", all_other_peers);

    for peer in all_other_peers.clone() {
        p2p.start_handshake(format!("{}:{}", peer.clone(), 9090));
    }

    let mut interval = tokio::time::interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let peer_names = HashSet::from_iter(p2p.peers.iter().map(|(_, v)| v.node_connection_data.name.clone()));

                if peer_names == all_other_peers {
                    break {
                        tracing::info!("Breaking {:?}", peer_names);
                        Ok(())
                    };
                }
            }
            tcp_event = p2p.listen_next() => {
                _ = p2p.handle_p2p_tcp_event(tcp_event).await;
            }
        }
    }
}

pub fn setup_basic(peers: Vec<String>, sim: &mut Sim<'_>, seed: u64) -> anyhow::Result<()> {
    tracing::info!("Starting simulation with peers {:?}", peers.clone());
    for peer in peers.clone().into_iter() {
        let peer_clone = peer.clone();
        let peers_clone = peers.clone();
        sim.client(peer.clone(), async move {
            setup_basic_host(peer_clone, peers_clone, seed).await
        })
    }

    sim.run()
        .map_err(|e| anyhow::anyhow!("Simulation error {}", e.to_string()))?;

    Ok(())
}

async fn setup_drop_host(peer: String, peers: Vec<String>) -> Result<(), Box<dyn Error>> {
    let crypto = BlstCrypto::new(peer.clone().as_str())?;
    let mut p2p = p2p_server_test::start_server(
        std::sync::Arc::new(crypto),
        peer.clone(),
        9090,
        None,
        format!("{}:{}", peer, 9090),
        format!("{}:{}", peer, 4141),
    )
    .await?;

    let all_other_peers: HashSet<String> =
        HashSet::from_iter(peers.clone().into_iter().filter(|p| p != &peer));

    tracing::info!("All other peers {:?}", all_other_peers);

    for peer in all_other_peers.clone() {
        p2p.start_handshake(format!("{}:{}", peer.clone(), 9090));
    }

    let mut interval_broadcast = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            tcp_event = p2p.listen_next() => {
                _ = p2p.handle_p2p_tcp_event(tcp_event).await;
            }
            _ = interval_broadcast.tick() => {
                p2p.broadcast(Msg(10)).await;
            }
        }
    }
}
async fn setup_drop_client(
    peer: String,
    peers: Vec<String>,
    duration: u64,
) -> Result<(), Box<dyn Error>> {
    let crypto = BlstCrypto::new(peer.clone().as_str())?;
    let mut p2p = p2p_server_test::start_server(
        std::sync::Arc::new(crypto),
        peer.clone(),
        9090,
        None,
        format!("{}:{}", peer, 9090),
        format!("{}:{}", peer, 4141),
    )
    .await?;

    let all_other_peers: HashSet<String> =
        HashSet::from_iter(peers.clone().into_iter().filter(|p| p != &peer));

    tracing::info!("All other peers {:?}", all_other_peers);

    for peer in all_other_peers.clone() {
        p2p.start_handshake(format!("{}:{}", peer.clone(), 9090));
    }

    let mut interval_broadcast = tokio::time::interval(Duration::from_millis(500));
    let mut interval_start_shutdown = tokio::time::interval(Duration::from_millis(duration));
    loop {
        tokio::select! {
            _ = interval_start_shutdown.tick() => {
                let peer_names = HashSet::from_iter(p2p.peers.iter().map(|(_, v)| v.node_connection_data.name.clone()));

                tracing::info!("Current peers {:?}", peer_names);

                if peer_names == all_other_peers {
                    break {
                        tracing::info!("Breaking tcp peers {:?}", p2p.tcp_server.connected_clients());
                        Ok(())
                    };
                }
            }

            tcp_event = p2p.listen_next() => {
                _ = p2p.handle_p2p_tcp_event(tcp_event).await;
            }
            _ = interval_broadcast.tick() => {
                p2p.broadcast(Msg(10)).await;
            }
        }
    }
}
pub fn setup_drops(peers: Vec<String>, sim: &mut Sim<'_>, seed: u64) -> anyhow::Result<()> {
    tracing::info!("Starting simulation with peers {:?}", peers.clone());

    let sim_duration = 12000;
    let mut host_peers = peers.clone();
    let client_peer = host_peers.pop().unwrap();

    // Setup hosts (long running, restarts, network issues)
    for peer in host_peers.clone().into_iter() {
        let peer_clone = peer.clone();
        let peers_clone = peers.clone();

        sim.host(peer.clone(), move || {
            let peer_clone = peer_clone.clone();
            let peers_clone = peers_clone.clone();
            async move { setup_drop_host(peer_clone, peers_clone).await }
        });
    }

    let cloned_client_peer = client_peer.clone();
    let cloned_peers = peers.clone();

    // Setup client (check all peers are here at the end of the simulation)
    sim.client(client_peer.clone(), async move {
        setup_drop_client(cloned_client_peer, cloned_peers, sim_duration).await
    });

    // Choose 2 random hosts
    let mut gen_hosts_couple = {
        let mut rng = StdRng::seed_from_u64(seed);
        let peers_len = host_peers.len() as u64;
        move || {
            let peerss = host_peers.clone();
            let n1 = rng.next_u64() % peers_len;

            let n2 = loop {
                let n = rng.next_u64() % peers_len;

                if n != n1 {
                    break n;
                }
            };

            (
                peerss.get(n1 as usize).unwrap().clone(),
                peerss.get(n2 as usize).unwrap().clone(),
            )
        }
    };

    let mut last_trigger = Duration::from_secs(0);
    let mut last_couple = gen_hosts_couple();

    let mut drops = 0;

    loop {
        let step = sim
            .step()
            .map_err(|e| anyhow::anyhow!("Simulation error {}", e.to_string()))?;

        if sim.elapsed().abs_diff(last_trigger) > Duration::from_secs(1) && drops < 10 {
            tracing::error!("Repair {} {}", last_couple.0.clone(), last_couple.1.clone());
            sim.repair(last_couple.0.clone(), last_couple.1.clone());

            // regen couple
            last_couple = gen_hosts_couple();
            if drops < 9 {
                sim.partition(last_couple.0.clone(), last_couple.1.clone());
                tracing::error!(
                    "Partition {} {}",
                    last_couple.0.clone(),
                    last_couple.1.clone()
                );
            }

            // bounce
            sim.bounce(last_couple.0.clone());
            tracing::error!("Bounce {}", last_couple.0.clone());

            last_trigger = sim.elapsed();
            drops += 1;
        }

        if step {
            return Ok(());
        }
    }
}
