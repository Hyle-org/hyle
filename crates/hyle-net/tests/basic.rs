#![allow(clippy::all)]
#![cfg(feature = "turmoil")]
#![cfg(test)]

use std::{error::Error, time::Duration};

use hyle_crypto::BlstCrypto;
use hyle_net::{
    net::Sim,
    p2p_server_mod,
    tcp::{p2p_server::P2PServerEvent, P2PTcpMessage, TcpEvent},
};
use rand::{rngs::StdRng, SeedableRng};

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
                    .simulation_duration(Duration::from_secs(10))
                    .tick_duration(Duration::from_millis(50))
                    // .enable_tokio_io()
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

    ($seed_from:literal..=$seed_to:literal, $simulation:ident, $test:ident) => {
        seq_macro::seq!(SEED in $seed_from..=$seed_to {
            turmoil_simple!(SEED, $simulation, $test);
        });
    };
}

turmoil_simple!(500, 4, simulation_realistic_network);

async fn setup_host(peer: String, peers: Vec<String>) -> Result<(), Box<dyn Error>> {
    let crypto = BlstCrypto::new(peer.clone().as_str())?;
    let mut p2p =
        p2p_server_test::start_server(crypto, peer.clone(), peer.clone(), 4141, 9090).await?;

    // if peer != "peer1".to_string() {
    //     for p in peers.clone().into_iter() {
    //         tracing::info!("Starting handshake with peer {}", p);
    //         if let Err(e) = p2p.start_handshake(format!("{}:{}", p, 9090)).await {
    //             tracing::error!("Error during handshake {:?}", e);
    //         }
    //     }
    // }

    let mut initial_handshakes = peers.clone();
    tracing::info!("finished");

    let mut interval = tokio::time::interval(Duration::from_millis(1000));

    // let mut pub_keys = vec![];

    loop {
        tokio::select! {
            Some(peer) = async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                initial_handshakes.pop()
            } => {
                if let Err(e) = p2p.start_handshake(format!("{}:{}", peer, 9090)).await {
                    tracing::error!("Error during handshake {:?}", e);
                }
            }
            _ = interval.tick() => {


                let _ = p2p.broadcast(Msg(10)).await;

            }
            Some(tcp_event) = p2p.listen_next() => {
                let TcpEvent {
                    dest,
                    data: p2p_tcp_message,
                } = tcp_event;
                match p2p_tcp_message {
                    // When p2p server receives a handshake message; we process it in order to extract information of new peer
                    P2PTcpMessage::Handshake(handshake) => {
                        if let Ok(Some(P2PServerEvent::NewPeer { name, pubkey: _ , da_address: _ })) = p2p.handle_handshake(dest, handshake).await {
                            // pub_keys.push(pubkey);
                            // p2p.

                            let peer_names = p2p.peers.iter().map(|(_, v)| v.node_connection_data.name.clone()).collect::<Vec<String>>();
                            tracing::info!("New peer {} (all: {:?})", name, peer_names);
                        }
                    }
                    // We should expose a handler to consume data, with the peer identity
                    P2PTcpMessage::Data(_net_message) => {
                        // let _ =
                        //     self.handle_net_message(net_message).await,
                        // "Handling P2P net message"
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
    // for peer in peers.clone().into_iter() {
    //     let peer_clone = peer.clone();
    //     let peers_clone = peers.clone();
    //     sim.host(peer.clone(), move || {
    //         let cloned_peers = peers_clone.clone();
    //         let cloned_peer = peer_clone.clone();
    //         async move { setup_host(cloned_peer, cloned_peers).await }
    //     })
    // }
    for peer in peers.clone().into_iter() {
        let peer_clone = peer.clone();
        let peers_clone = peers.clone();
        sim.client(peer.clone(), async move {
            setup_host(peer_clone, peers_clone).await
        })
    }

    // sim.client("client", async move {
    //     setup_host("client".to_string(), peers).await
    // });

    // sim.client("other_client", async move {
    //     loop {
    //         tokio::time::sleep(Duration::from_millis(200)).await;
    //         tracing::info!("plop");
    //     }
    // });

    _ = sim.run();

    Ok(())
}
