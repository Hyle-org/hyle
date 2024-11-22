#[cfg(test)]
pub mod test {

    #[macro_export]
    macro_rules! build_tuple {
        ($nodes:expr, 1) => {
            ($nodes)
        };
        ($nodes:expr, 2) => {
            ($nodes, $nodes)
        };
        ($nodes:expr, 3) => {
            ($nodes, $nodes, $nodes)
        };
        ($nodes:expr, 4) => {
            ($nodes, $nodes, $nodes, $nodes)
        };
        ($nodes:expr, 5) => {
            ($nodes, $nodes, $nodes, $nodes, $nodes)
        };
        ($nodes:expr, 6) => {
            ($nodes, $nodes, $nodes, $nodes, $nodes, $nodes)
        };
        ($nodes:expr, 7) => {
            ($nodes, $nodes, $nodes, $nodes, $nodes, $nodes, $nodes)
        };
        ($nodes:expr, $count:expr) => {
            panic!("Le nombre de nœuds {} n'est pas supporté", $count)
        };
    }

    #[macro_export]
    macro_rules! broadcast {
        (description: $description:literal, from: $sender:ident, to: [$($node:ident),+]$(, message_matches: $pattern:pat $(=> $asserts:block)? )?) => {
            {
                // Construct the broadcast message with sender information
                let message = $sender.assert_broadcast(format!("[broadcast from: {}] {}", stringify!($sender), $description).as_str());

                $({
                    let msg_variant_name: &'static str = message.msg.clone().into();
                    if let $pattern = &message.msg {
                        $($asserts)?
                    } else {
                        panic!("[broadcast from: {}] {}: Message {} did not match {}", stringify!($sender), $description, msg_variant_name, stringify!($pattern));
                    }
                })?

                // Distribute the message to each specified node
                $(
                    $node.handle_msg(&message, (format!("[handling broadcast message from: {} at: {}] {}", stringify!($sender), stringify!($node), $description).as_str()));
                )+

                message
            }
        };
    }

    #[macro_export]
    macro_rules! send {
        (
            description: $description:literal,
            from: [$($node:ident),+],
            to: $to:ident,
            message_matches: $pattern:pat
        ) => {
            // Distribute the message to the target node from all specified nodes
            $(
                let answer = $node.assert_send(
                    &$to,
                    format!("[send from: {} to: {}] {}", stringify!($node), stringify!($to), $description).as_str()
                );

                // If `message_matches` is provided, perform the pattern match
                if let $pattern = &answer.msg {
                    // Execute optional assertions if provided
                } else {
                    let msg_variant_name: &'static str = answer.msg.clone().into();
                    panic!(
                        "[send from: {}] {}: Message {} did not match {}",
                        stringify!($node),
                        $description,
                        msg_variant_name,
                        stringify!($pattern)
                    );
                }

                // Handle the message
                $to.handle_msg(
                    &answer,
                    format!("[handling sent message from: {} to: {}] {}", stringify!($node), stringify!($to), $description).as_str()
                );
            )+
        };

        (
            description: $description:literal,
            from: [$($node:ident: $pattern:pat $(=> $asserts:block)?)+],
            to: $to:ident
        ) => {
            // Distribute the message to the target node from all specified nodes
            $(
                let answer = $node.assert_send(
                    &$to,
                    format!("[send from: {} to: {}] {}", stringify!($node), stringify!($to), $description).as_str()
                );

                // If `message_matches` is provided, perform the pattern match
                if let $pattern = &answer.msg {
                    // Execute optional assertions if provided
                    $($asserts)?
                } else {
                    let msg_variant_name: &'static str = answer.msg.clone().into();
                    panic!(
                        "[send from: {}] {}: Message {} did not match {}",
                        stringify!($node),
                        $description,
                        msg_variant_name,
                        stringify!($pattern)
                    );
                }

                // Handle the message
                $to.handle_msg(
                    &answer,
                    format!("[handling sent message from: {} to: {}] {}", stringify!($node), stringify!($to), $description).as_str()
                );
            )+
        };
    }

    macro_rules! build_nodes {
        ($count:tt) => {{
            async {
                let cryptos: Vec<BlstCrypto> = AutobahnTestCtx::generate_cryptos($count);

                let mut nodes = vec![];

                for i in 0..$count {
                    let crypto = cryptos.get(i).unwrap().clone();
                    let mut autobahn_node =
                        AutobahnTestCtx::new(format!("node-{i}").as_ref(), crypto).await;

                    autobahn_node.consensus.setup_node(i, &cryptos);
                    nodes.push(autobahn_node);
                }

                build_tuple!(nodes.remove(0), $count)
            }
        }};
    }

    use std::sync::Arc;

    use tracing::info;

    use crate::bus::dont_use_this::get_receiver;
    use crate::bus::metrics::BusMetrics;
    use crate::bus::SharedMessageBus;
    use crate::consensus::metrics::ConsensusMetrics;
    use crate::consensus::test::ConsensusTestCtx;
    use crate::consensus::{Consensus, ConsensusBusClient, ConsensusEvent, ConsensusStore};
    use crate::mempool::metrics::MempoolMetrics;
    use crate::mempool::test::MempoolTestCtx;
    use crate::mempool::{Mempool, MempoolBusClient};
    use crate::node_state::NodeState;
    use crate::p2p::network::OutboundMessage;
    use crate::p2p::P2PCommand;
    use crate::utils::conf::Conf;
    use crate::utils::crypto::{self, BlstCrypto};

    pub struct AutobahnTestCtx {
        consensus: ConsensusTestCtx,
        mempool: MempoolTestCtx,
    }

    impl AutobahnTestCtx {
        async fn new(name: &str, crypto: BlstCrypto) -> Self {
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));
            let out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;
            let event_receiver = get_receiver::<ConsensusEvent>(&shared_bus).await;
            let p2p_receiver = get_receiver::<P2PCommand>(&shared_bus).await;
            let consensus_bus = ConsensusBusClient::new_from_bus(shared_bus.new_handle()).await;

            let store = ConsensusStore::default();
            let conf = Arc::new(Conf::default());
            let consensus = Consensus {
                metrics: ConsensusMetrics::global("id".to_string()),
                bus: consensus_bus,
                file: None,
                store,
                config: conf,
                crypto: Arc::new(crypto.clone()),
            };

            let mempool_bus = MempoolBusClient::new_from_bus(shared_bus.new_handle()).await;

            let mempool = Mempool {
                metrics: MempoolMetrics::global("id".to_string()),
                bus: mempool_bus,
                crypto: Arc::new(crypto.clone()),
                storage: crate::mempool::storage::Storage::new(crypto.validator_pubkey().clone()),
                validators: vec![],
                node_state: NodeState::default(),
            };

            AutobahnTestCtx {
                consensus: ConsensusTestCtx {
                    out_receiver: out_receiver.resubscribe(),
                    _event_receiver: event_receiver,
                    _p2p_receiver: p2p_receiver,
                    consensus,
                    name: name.to_string(),
                },
                mempool: MempoolTestCtx {
                    out_receiver: out_receiver.resubscribe(),
                    mempool,
                },
            }
        }

        pub fn generate_cryptos(nb: usize) -> Vec<BlstCrypto> {
            (0..nb)
                .map(|i| {
                    let crypto = crypto::BlstCrypto::new(format!("node-{i}").into());
                    info!("node {}: {}", i, crypto.validator_pubkey());
                    crypto
                })
                .collect()
        }
    }

    #[tokio::test]
    async fn autobahn_test() {
        let (mut node1, mut node2, mut node3, mut node4) = build_nodes!(4).await;
    }
}
