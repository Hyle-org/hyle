#[cfg(test)]
pub mod test {

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

    macro_rules! broadcast {
        (description: $description:literal, from: $sender:expr, to: [$($node:expr),+]$(, message_matches: $pattern:pat $(=> $asserts:block)? )?) => {
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

    macro_rules! send {
        (
            description: $description:literal,
            from: [$($node:expr),+],
            to: $to:expr,
            message_matches: $pattern:pat
        ) => {
            // Distribute the message to the target node from all specified nodes
            ($({
                let answer = $node.assert_send(
                    &$to.validator_pubkey(),
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
                answer
            },)+)
        };

        (
            description: $description:literal,
            from: [$($node:ident: $pattern:pat $(=> $asserts:block)?)+],
            to: $to:ident
        ) => {
            // Distribute the message to the target node from all specified nodes
            ($({
                let answer = $node.assert_send(
                    &$to.validator_pubkey(),
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
            },)+)
        };
    }

    pub(crate) use broadcast;
    pub(crate) use build_tuple;
    pub(crate) use send;

    macro_rules! build_nodes {
        ($count:tt) => {{
            async {
                let cryptos: Vec<BlstCrypto> = AutobahnTestCtx::generate_cryptos($count);

                let mut nodes = vec![];

                for i in 0..$count {
                    let crypto = cryptos.get(i).unwrap().clone();
                    let mut autobahn_node =
                        AutobahnTestCtx::new(format!("node-{i}").as_ref(), crypto).await;

                    autobahn_node.consensus_ctx.setup_node(i, &cryptos);
                    autobahn_node.mempool_ctx.setup_node(&cryptos);
                    nodes.push(autobahn_node);
                }

                build_tuple!(nodes.remove(0), $count)
            }
        }};
    }

    use crate::bus::command_response::Query;
    use crate::bus::{bus_client, SharedMessageBus};
    use crate::consensus::test::ConsensusTestCtx;
    use crate::consensus::{ConsensusEvent, ConsensusNetMessage};
    use crate::handle_messages;
    use crate::mempool::storage::Cut;
    use crate::mempool::test::{make_register_contract_tx, MempoolTestCtx};
    use crate::mempool::{MempoolNetMessage, QueryNewCut};
    use crate::model::{ContractName, Hashable};
    use crate::p2p::network::OutboundMessage;
    use crate::p2p::P2PCommand;
    use crate::rest::endpoints::RestApiMessage;
    use crate::utils::crypto::{self, BlstCrypto};
    use tracing::info;

    bus_client!(
        pub struct AutobahnBusClient {
            receiver(Query<QueryNewCut, Cut>),
        }
    );

    pub struct AutobahnTestCtx {
        shared_bus: SharedMessageBus,
        consensus_ctx: ConsensusTestCtx,
        mempool_ctx: MempoolTestCtx,
    }

    impl AutobahnTestCtx {
        async fn new(name: &str, crypto: BlstCrypto) -> Self {
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));
            let event_receiver = get_receiver::<ConsensusEvent>(&shared_bus).await;
            let p2p_receiver = get_receiver::<P2PCommand>(&shared_bus).await;
            let consensus_out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;
            let mempool_out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;

            let consensus = ConsensusTestCtx::build_consensus(&shared_bus, crypto.clone()).await;
            let mempool = MempoolTestCtx::build_mempool(&shared_bus, crypto).await;

            AutobahnTestCtx {
                shared_bus,
                consensus_ctx: ConsensusTestCtx {
                    out_receiver: consensus_out_receiver,
                    _event_receiver: event_receiver,
                    _p2p_receiver: p2p_receiver,
                    consensus,
                    name: name.to_string(),
                },
                mempool_ctx: MempoolTestCtx {
                    name: name.to_string(),
                    out_receiver: mempool_out_receiver,
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

        pub async fn setup_query_cut_answer(&mut self) {
            let mut validators = self.consensus_ctx.validators();
            let latest_cut: Cut = self
                .mempool_ctx
                .mempool
                .handle_querynewcut(&mut QueryNewCut(validators.clone()))
                .await
                .expect("latest cut");

            let mut autobahn_client_bus =
                AutobahnBusClient::new_from_bus(self.shared_bus.new_handle()).await;

            use crate::bus::command_response::*;

            tokio::spawn(async move {
                handle_messages! {
                    on_bus autobahn_client_bus,
                    listen<Query<QueryNewCut, Cut>> qnc => {
                        if let Ok(mut value) = qnc.take() {
                            let QueryNewCut(data) = &mut value.data;
                            dbg!(data.clone());
                            assert_eq!(&mut validators, data);
                            value.answer(latest_cut.clone()).expect("error when injecting cut");

                            break;
                        }
                    }
                }
            });
        }
    }

    #[test_log::test(tokio::test)]
    async fn autobahn_basic_flow() {
        let (mut node1, mut node2, mut node3, mut node4) = build_nodes!(4).await;

        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));
        let register_tx_2 = make_register_contract_tx(ContractName("test2".to_owned()));

        node1
            .mempool_ctx
            .mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        node1
            .mempool_ctx
            .mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx_2.clone()))
            .expect("fail to handle new transaction");

        node1.mempool_ctx.mempool.handle_data_proposal_management();

        broadcast! {
            description: "Disseminate Tx",
            from: node1.mempool_ctx, to: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx],
            message_matches: MempoolNetMessage::DataProposal(data) => {
                assert_eq!(data.car.txs.len(), 2);
            }
        };

        send! {
            description: "Disseminated Tx Vote",
            from: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx], to: node1.mempool_ctx,
            message_matches: MempoolNetMessage::DataVote(_)
        };

        let car_hash_node1 = node1
            .mempool_ctx
            .current_hash()
            .expect("Current hash should be there");

        node1.setup_query_cut_answer().await;

        node1.consensus_ctx.start_round().await;

        let consensus_proposal;

        broadcast! {
            description: "Prepare",
            from: node1.consensus_ctx, to: [node2.consensus_ctx, node3.consensus_ctx, node4.consensus_ctx],
            message_matches: ConsensusNetMessage::Prepare(cp, _ticket) => {
                consensus_proposal = cp.clone();
                assert_eq!(
                    cp
                        .get_cut()
                        .iter()
                        .find(|(validator, _hash)|
                            validator == &node1.consensus_ctx.pubkey()
                        ),
                    Some((node1.consensus_ctx.pubkey(), car_hash_node1)).as_ref()
                );
            }
        };

        let (vote2, vote3, vote4) = send! {
            description: "PrepareVote",
            from: [node2.consensus_ctx, node3.consensus_ctx, node4.consensus_ctx], to: node1.consensus_ctx,
            message_matches: ConsensusNetMessage::PrepareVote(_)
        };

        assert_eq!(
            vote2.msg,
            ConsensusNetMessage::PrepareVote(consensus_proposal.hash())
        );
        assert_eq!(
            vote3.msg,
            ConsensusNetMessage::PrepareVote(consensus_proposal.hash())
        );
        assert_eq!(
            vote4.msg,
            ConsensusNetMessage::PrepareVote(consensus_proposal.hash())
        );

        broadcast! {
            description: "Confirm",
            from: node1.consensus_ctx, to: [node2.consensus_ctx, node3.consensus_ctx, node4.consensus_ctx],
            message_matches: ConsensusNetMessage::Confirm(_)
        };

        let (vote2, vote3, vote4) = send! {
            description: "ConfirmAck",
            from: [node2.consensus_ctx, node3.consensus_ctx, node4.consensus_ctx], to: node1.consensus_ctx,
            message_matches: ConsensusNetMessage::ConfirmAck(_)
        };

        assert_eq!(
            vote2.msg,
            ConsensusNetMessage::ConfirmAck(consensus_proposal.hash())
        );
        assert_eq!(
            vote3.msg,
            ConsensusNetMessage::ConfirmAck(consensus_proposal.hash())
        );
        assert_eq!(
            vote4.msg,
            ConsensusNetMessage::ConfirmAck(consensus_proposal.hash())
        );

        broadcast! {
            description: "Commit",
            from: node1.consensus_ctx, to: [node2.consensus_ctx, node3.consensus_ctx, node4.consensus_ctx],
            message_matches: ConsensusNetMessage::Commit(_, _)
        };
    }
}
