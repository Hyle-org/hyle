#![allow(clippy::all)]
#![cfg(feature = "turmoil")]
#![cfg(test)]

use std::{collections::HashSet, error::Error, time::Duration};

use hyle_crypto::BlstCrypto;
use hyle_net::{
    net::Sim,
    p2p_server_mod,
    tcp::{p2p_server::P2PServerEvent, tcp_client::TcpClient, P2PTcpMessage},
};
use rand::{rngs::StdRng, SeedableRng};
use tokio::task::JoinSet;

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
            fn [<turmoil_p2p_ $simulation _ $seed >]() -> anyhow::Result<()> {
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

                $simulation(peers, &mut sim)?;

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

turmoil_simple!(501..=520, 10, simulation_realistic_network);

async fn setup_host(peer: String, peers: Vec<String>) -> Result<(), Box<dyn Error>> {
    let crypto = BlstCrypto::new(peer.clone().as_str())?;
    let mut p2p = p2p_server_test::start_server(
        std::sync::Arc::new(crypto),
        peer.clone(),
        9090,
        format!("{}:{}", peer, 9090),
        format!("{}:{}", peer, 4141),
    )
    .await?;

    let mut handshake_clients_tasks = JoinSet::new();

    let all_other_peers: HashSet<String> =
        HashSet::from_iter(peers.clone().into_iter().filter(|p| p != &peer));

    tracing::info!("All other peers {:?}", all_other_peers);

    for peer in all_other_peers.clone() {
        handshake_clients_tasks.spawn(async move {
            TcpClient::connect("turmoil_handshake", format!("{}:{}", peer.clone(), 9090)).await
        });
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
            Some(tcp_event) = p2p.listen_next() => {
                if let Ok(Some(p2p_tcp_event)) = p2p.handle_tcp_event(tcp_event).await {
                    match p2p_tcp_event {
                        P2PServerEvent::NewPeer { name, pubkey: _, da_address: _ } => {
                            let peer_names = p2p.peers.iter().map(|(_, v)| v.node_connection_data.name.clone()).collect::<Vec<String>>();
                            tracing::info!("New peer {} (all: {:?})", name, peer_names);
                        },
                        P2PServerEvent::P2PMessage { msg: _ } => {},
                        P2PServerEvent::DisconnectedPeer { peer_ip } => {
                            handshake_clients_tasks.spawn(async move {
                                TcpClient::connect("turmoil_disconnected_peer", peer_ip.clone()).await
                            });
                        },
                    }
                }
            }

            Some(task_result) = handshake_clients_tasks.join_next() => {
                if let Ok(tcp_client) = task_result {
                    if let Ok(tcp_client) = tcp_client {
                        if let Err(e) = p2p.start_handshake(tcp_client).await {
                            tracing::error!("Error during handshake {:?}", e);
                        }
                    }
                }
            }
        }
    }
}

/// **Simulation**
///
/// Simulate a slow network (with fixed random latencies)
/// *realistic* -> min = 20, max = 500, lambda = 0.025
/// *slow*      -> min = 50, max = 1000, lambda = 0.01
pub fn simulation_realistic_network(peers: Vec<String>, sim: &mut Sim<'_>) -> anyhow::Result<()> {
    tracing::info!("Starting simulation with peers {:?}", peers.clone());
    for peer in peers.clone().into_iter() {
        let peer_clone = peer.clone();
        let peers_clone = peers.clone();
        sim.client(peer.clone(), async move {
            setup_host(peer_clone, peers_clone).await
        })
    }

    sim.run()
        .map_err(|e| anyhow::anyhow!("Simulation error {}", e.to_string()))?;

    Ok(())
}
