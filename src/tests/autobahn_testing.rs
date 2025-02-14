//! This module is intended for "integration" testing of the consensus and other modules.

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
        panic!("More than {} nodes isn't supported", $count)
    };
}

macro_rules! broadcast {
    (description: $description:literal, from: $sender:expr, to: [$($node:expr),*]$(, message_matches: $pattern:pat $(=> $asserts:block)? )?) => {
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
            )*

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
        from: [$($node:expr; $pattern:pat $(=> $asserts:block)?),+],
        to: $to:expr
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

macro_rules! simple_commit_round {
    (leader: $leader:expr, followers: [$($follower:expr),+]$(, joining: $joining:expr)?) => {{
        let round_consensus_proposal;
        let round_ticket;
        broadcast! {
            description: "Leader - Prepare",
            from: $leader, to: [$($follower),+$(,$joining)?],
            message_matches: hyle_contract_sdk::ConsensusNetMessage::Prepare(cp, ticket) => {
                round_consensus_proposal = cp.clone();
                round_ticket = ticket.clone();
            }
        };

        send! {
            description: "Follower - PrepareVote",
            from: [$($follower),+], to: $leader,
            message_matches: hyle_contract_sdk::ConsensusNetMessage::PrepareVote(_)
        };

        broadcast! {
            description: "Leader - Confirm",
            from: $leader, to: [$($follower),+$(,$joining)?],
            message_matches: hyle_contract_sdk::ConsensusNetMessage::Confirm(_)
        };

        send! {
            description: "Follower - Confirm Ack",
            from: [$($follower),+], to: $leader,
            message_matches: hyle_contract_sdk::ConsensusNetMessage::ConfirmAck(_)
        };

        broadcast! {
            description: "Leader - Commit",
            from: $leader, to: [$($follower),+$(,$joining)?],
            message_matches: hyle_contract_sdk::ConsensusNetMessage::Commit(_, _)
        };

        (round_consensus_proposal, round_ticket)
    }};
}

macro_rules! assert_chanmsg_matches {
    ($chan: expr, $pat:pat => $block:block) => {{
        let var = $chan.try_recv().unwrap();
        if let $pat = var {
            $block
        } else {
            panic!("Var {:?} should match {}", var, stringify!($pat));
        }
    }};
}

pub(crate) use assert_chanmsg_matches;
pub(crate) use broadcast;
pub(crate) use build_tuple;
use futures::future::join_all;
pub(crate) use send;
pub(crate) use simple_commit_round;

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
use crate::bus::dont_use_this::get_receiver;
use crate::bus::metrics::BusMetrics;
use crate::bus::{bus_client, SharedMessageBus};
use crate::consensus::test::ConsensusTestCtx;
use crate::consensus::ConsensusEvent;
use crate::handle_messages;
use crate::mempool::test::{make_register_contract_tx, MempoolTestCtx};
use crate::mempool::{
    InternalMempoolEvent, MempoolBlockEvent, MempoolNetMessage, MempoolStatusEvent, QueryNewCut,
};
use crate::model::*;
use crate::node_state::module::NodeStateEvent;
use crate::p2p::network::OutboundMessage;
use crate::p2p::P2PCommand;
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
        let mempool_event_receiver = get_receiver::<MempoolBlockEvent>(&shared_bus).await;
        let mempool_status_event_receiver = get_receiver::<MempoolStatusEvent>(&shared_bus).await;
        let mempool_internal_event_receiver =
            get_receiver::<InternalMempoolEvent>(&shared_bus).await;

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
                mempool_event_receiver,
                mempool_status_event_receiver,
                mempool_internal_event_receiver,
                mempool,
            },
        }
    }

    pub fn generate_cryptos(nb: usize) -> Vec<BlstCrypto> {
        (0..nb)
            .map(|i| {
                let crypto = crypto::BlstCrypto::new(format!("node-{i}")).unwrap();
                info!("node {}: {}", i, crypto.validator_pubkey());
                crypto
            })
            .collect()
    }

    /// Spawn a coroutine to answer the command response call of start_round, with the current current of mempool
    async fn start_round_with_cut_from_mempool(&mut self) {
        let staking = self.consensus_ctx.staking();
        let latest_cut: Cut = self.mempool_ctx.gen_cut(&staking);

        let mut autobahn_client_bus =
            AutobahnBusClient::new_from_bus(self.shared_bus.new_handle()).await;

        use crate::bus::command_response::*;

        tokio::spawn(async move {
            handle_messages! {
                on_bus autobahn_client_bus,
                listen<Query<QueryNewCut, Cut>> qnc => {
                    if let Ok(value) = qnc.take() {
                        value.answer(latest_cut.clone()).expect("error when injecting cut");

                        break;
                    }
                }
            }
        });

        self.consensus_ctx.start_round().await;
    }
}

fn create_poda(
    data_proposal_hash: DataProposalHash,
    line_size: LaneBytesSize,
    nodes: &[&AutobahnTestCtx],
) -> crypto::Signed<MempoolNetMessage, crypto::AggregateSignature> {
    let msg = MempoolNetMessage::DataVote(data_proposal_hash, line_size);
    let signed_messages: Vec<crypto::Signed<MempoolNetMessage, crypto::ValidatorSignature>> = nodes
        .iter()
        .map(|node| {
            node.mempool_ctx
                .mempool
                .sign_net_message(msg.clone())
                .unwrap()
        })
        .collect();

    let aggregates: Vec<&crypto::Signed<MempoolNetMessage, crypto::ValidatorSignature>> =
        signed_messages.iter().collect();
    BlstCrypto::aggregate(msg, &aggregates).unwrap()
}

#[test_log::test(tokio::test)]
async fn autobahn_basic_flow() {
    let (mut node1, mut node2, mut node3, mut node4) = build_nodes!(4).await;

    let register_tx = make_register_contract_tx(ContractName::new("test1"));
    let register_tx_2 = make_register_contract_tx(ContractName::new("test2"));

    node1.mempool_ctx.submit_tx(&register_tx);
    node1.mempool_ctx.submit_tx(&register_tx_2);

    node1
        .mempool_ctx
        .make_data_proposal_with_pending_txs()
        .expect("Should create data proposal");

    broadcast! {
        description: "Disseminate Tx",
        from: node1.mempool_ctx, to: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx],
        message_matches: MempoolNetMessage::DataProposal(data) => {
            assert_eq!(data.txs.len(), 2);
        }
    };

    join_all(
        [
            &mut node2.mempool_ctx,
            &mut node3.mempool_ctx,
            &mut node4.mempool_ctx,
        ]
        .iter_mut()
        .map(|ctx| ctx.handle_processed_data_proposals()),
    )
    .await;

    send! {
        description: "Disseminated Tx Vote",
        from: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx], to: node1.mempool_ctx,
        message_matches: MempoolNetMessage::DataVote(_, _)
    };

    let data_proposal_hash_node1 = node1
        .mempool_ctx
        .current_hash(node1.mempool_ctx.validator_pubkey())
        .expect("Current hash should be there");
    let node1_l_size = node1.mempool_ctx.current_size().unwrap();

    node1.start_round_with_cut_from_mempool().await;

    let consensus_proposal;

    let poda = create_poda(
        data_proposal_hash_node1.clone(),
        node1_l_size,
        &[&node1, &node2, &node3, &node4],
    );

    broadcast! {
        description: "Prepare",
        from: node1.consensus_ctx, to: [node2.consensus_ctx, node3.consensus_ctx, node4.consensus_ctx],
        message_matches: ConsensusNetMessage::Prepare(cp, _ticket) => {
            consensus_proposal = cp.clone();
            assert_eq!(
                cp
                    .cut
                    .clone()
                    .iter()
                    .find(|(validator, _hash, _size, _poda)|
                        validator == &node1.consensus_ctx.pubkey()
                    ),
                Some((node1.consensus_ctx.pubkey(), data_proposal_hash_node1, node1_l_size, poda.signature)).as_ref()
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

#[test_log::test(tokio::test)]
async fn mempool_broadcast_multiple_data_proposals() {
    let (mut node1, mut node2, mut node3, mut node4) = build_nodes!(4).await;

    // First data proposal

    let register_tx = make_register_contract_tx(ContractName::new("test1"));
    let register_tx_2 = make_register_contract_tx(ContractName::new("test2"));

    node1.mempool_ctx.submit_tx(&register_tx);
    node1.mempool_ctx.submit_tx(&register_tx_2);

    node1
        .mempool_ctx
        .make_data_proposal_with_pending_txs()
        .expect("Should create data proposal");

    broadcast! {
        description: "Disseminate Tx",
        from: node1.mempool_ctx, to: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx],
        message_matches: MempoolNetMessage::DataProposal(_)
    };

    join_all(
        [
            &mut node2.mempool_ctx,
            &mut node3.mempool_ctx,
            &mut node4.mempool_ctx,
        ]
        .iter_mut()
        .map(|ctx| ctx.handle_processed_data_proposals()),
    )
    .await;

    send! {
        description: "Disseminated Tx Vote",
        from: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx], to: node1.mempool_ctx,
        message_matches: MempoolNetMessage::DataVote(_, _)
    };

    node1.mempool_ctx.assert_broadcast("poda update");
    node1.mempool_ctx.assert_broadcast("poda update");

    // Second data proposal

    let register_tx = make_register_contract_tx(ContractName::new("test3"));
    let register_tx_2 = make_register_contract_tx(ContractName::new("test4"));

    node1.mempool_ctx.submit_tx(&register_tx);
    node1.mempool_ctx.submit_tx(&register_tx_2);

    node1
        .mempool_ctx
        .make_data_proposal_with_pending_txs()
        .expect("Should create data proposal");

    broadcast! {
        description: "Disseminate Tx",
        from: node1.mempool_ctx, to: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx],
        message_matches: MempoolNetMessage::DataProposal(_)
    };

    join_all(
        [
            &mut node2.mempool_ctx,
            &mut node3.mempool_ctx,
            &mut node4.mempool_ctx,
        ]
        .iter_mut()
        .map(|ctx| ctx.handle_processed_data_proposals()),
    )
    .await;

    send! {
        description: "Disseminated Tx Vote",
        from: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx], to: node1.mempool_ctx,
        message_matches: MempoolNetMessage::DataVote(_, _)
    };
}

#[test_log::test(tokio::test)]
async fn mempool_fail_to_vote_on_fork() {
    let (mut node1, mut node2, mut node3, mut node4) = build_nodes!(4).await;

    // Let's create data proposals that will be synchronized with 2 nodes, but not the 3rd.

    // First data proposal

    let register_tx = make_register_contract_tx(ContractName::new("test1"));
    let register_tx_2 = make_register_contract_tx(ContractName::new("test2"));

    node1.mempool_ctx.submit_tx(&register_tx);
    node1.mempool_ctx.submit_tx(&register_tx_2);

    node1
        .mempool_ctx
        .make_data_proposal_with_pending_txs()
        .expect("Should create data proposal");

    let dp1;

    broadcast! {
        description: "Disseminate Tx",
        from: node1.mempool_ctx, to: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx],
        message_matches: MempoolNetMessage::DataProposal(data) => {
            dp1 = data.clone();
        }
    };

    join_all(
        [
            &mut node2.mempool_ctx,
            &mut node3.mempool_ctx,
            &mut node4.mempool_ctx,
        ]
        .iter_mut()
        .map(|ctx| ctx.handle_processed_data_proposals()),
    )
    .await;

    send! {
        description: "Disseminated Tx Vote",
        from: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx], to: node1.mempool_ctx,
        message_matches: MempoolNetMessage::DataVote(_, _)
    };

    node1.mempool_ctx.assert_broadcast("poda update");
    node1.mempool_ctx.assert_broadcast("poda update");

    // Second data proposal

    let register_tx = make_register_contract_tx(ContractName::new("test3"));
    let register_tx_2 = make_register_contract_tx(ContractName::new("test4"));

    node1.mempool_ctx.submit_tx(&register_tx);
    node1.mempool_ctx.submit_tx(&register_tx_2);

    node1
        .mempool_ctx
        .make_data_proposal_with_pending_txs()
        .expect("Should create data proposal");

    broadcast! {
        description: "Disseminate Tx",
        from: node1.mempool_ctx, to: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx],
        message_matches: MempoolNetMessage::DataProposal(_)
    };

    join_all(
        [
            &mut node2.mempool_ctx,
            &mut node3.mempool_ctx,
            &mut node4.mempool_ctx,
        ]
        .iter_mut()
        .map(|ctx| ctx.handle_processed_data_proposals()),
    )
    .await;

    send! {
        description: "Disseminated Tx Vote",
        from: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx], to: node1.mempool_ctx,
        message_matches: MempoolNetMessage::DataVote(_, _)
    };

    // BASIC FORK
    // dp1(1) <- dp2(2)
    //        <-        dp3(3)

    let dp_fork_3 = DataProposal::new(Some(dp1.hash()), vec![Transaction::default()]);

    let data_proposal_fork_3 = node1
        .mempool_ctx
        .sign_data(MempoolNetMessage::DataProposal(dp_fork_3.clone()))
        .unwrap();

    node2
        .mempool_ctx
        .handle_msg(&data_proposal_fork_3, "Fork 3");
    node3
        .mempool_ctx
        .handle_msg(&data_proposal_fork_3, "Fork 3");
    node4
        .mempool_ctx
        .handle_msg(&data_proposal_fork_3, "Fork 3");

    // Check fork has not been consumed

    assert_ne!(
        node2
            .mempool_ctx
            .last_validator_lane_entry(node1.mempool_ctx.validator_pubkey())
            .1,
        dp_fork_3.hash()
    );
    assert_ne!(
        node3
            .mempool_ctx
            .last_validator_lane_entry(node1.mempool_ctx.validator_pubkey())
            .1,
        dp_fork_3.hash()
    );
    assert_ne!(
        node4
            .mempool_ctx
            .last_validator_lane_entry(node1.mempool_ctx.validator_pubkey())
            .1,
        dp_fork_3.hash()
    );

    // FORK with already registered id
    // dp1(1) <- dp2(2)
    //        <- dp3(2)
    // Remove data proposal id 1 from node 1, to recreate one on top of data proposal id 0, and create a fork

    node1.mempool_ctx.pop_data_proposal();

    let register_tx = make_register_contract_tx(ContractName::new("test5"));
    let register_tx_2 = make_register_contract_tx(ContractName::new("test6"));

    node1.mempool_ctx.submit_tx(&register_tx);
    node1.mempool_ctx.submit_tx(&register_tx_2);

    node1
        .mempool_ctx
        .make_data_proposal_with_pending_txs()
        .expect("Should create data proposal");

    node1.mempool_ctx.assert_broadcast("poda update");
    node1.mempool_ctx.assert_broadcast("poda update");

    let fork;
    broadcast! {
        description: "Disseminate Tx",
        from: node1.mempool_ctx, to: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx],
        message_matches: MempoolNetMessage::DataProposal(dp) => {
            fork = dp.clone();
        }
    };

    assert_ne!(
        fork,
        node2
            .mempool_ctx
            .pop_validator_data_proposal(node1.mempool_ctx.validator_pubkey())
            .0
    );
    assert_ne!(
        fork,
        node3
            .mempool_ctx
            .pop_validator_data_proposal(node1.mempool_ctx.validator_pubkey())
            .0
    );
    assert_ne!(
        fork,
        node4
            .mempool_ctx
            .pop_validator_data_proposal(node1.mempool_ctx.validator_pubkey())
            .0
    );
}

#[test_log::test(tokio::test)]
async fn autobahn_rejoin_flow() {
    let (mut node1, mut node2) = build_nodes!(2).await;

    // Let's setup the consensus so our joining node has some blocks to catch up.
    ConsensusTestCtx::setup_for_round(
        &mut [&mut node1.consensus_ctx, &mut node2.consensus_ctx],
        0,
        3,
        0,
    );

    let crypto = crypto::BlstCrypto::new("node-3".to_owned()).unwrap();
    let mut joining_node = AutobahnTestCtx::new("node-3", crypto).await;
    joining_node
        .consensus_ctx
        .setup_for_joining(&[&node1.consensus_ctx, &node2.consensus_ctx]);

    // Let's setup a DataAvailability on this bus
    let mut da = crate::data_availability::tests::DataAvailabilityTestCtx::new(
        joining_node.shared_bus.new_handle(),
    )
    .await;

    let mut blocks = vec![SignedBlock {
        data_proposals: vec![],
        certificate: AggregateSignature::default(),
        consensus_proposal: ConsensusProposal {
            slot: 0,
            ..ConsensusProposal::default()
        },
    }];

    for _ in 0..2 {
        blocks.push(SignedBlock {
            data_proposals: vec![],
            certificate: AggregateSignature::default(),
            consensus_proposal: ConsensusProposal {
                slot: blocks.len() as u64,
                parent_hash: blocks[blocks.len() - 1].hash(),
                ..ConsensusProposal::default()
            },
        });
    }

    let mut ns_event_receiver = get_receiver::<NodeStateEvent>(&joining_node.shared_bus).await;

    // Catchup up to the last block, but don't actually process the last block message yet.
    for block in blocks.get(0..blocks.len() - 1).unwrap() {
        da.handle_signed_block(block.clone()).await;
    }
    while let Ok(event) = ns_event_receiver.try_recv() {
        info!("{:?}", event);
        joining_node
            .consensus_ctx
            .handle_node_state_event(event)
            .await
            .expect("should handle data event");
    }

    // Do a few rounds of consensus-with-lag and note that we don't actually catch up.
    // (this is expected because DA stopped receiving new blocks, as it did indeed catch up)
    for _ in 0..3 {
        node1.start_round_with_cut_from_mempool().await;

        simple_commit_round! {
            leader: node1.consensus_ctx,
            followers: [node2.consensus_ctx],
            joining: joining_node.consensus_ctx
        };

        // Swap so we handle leader changes correctly
        std::mem::swap(&mut node1, &mut node2);
    }

    // Now process block 2
    da.handle_signed_block(blocks.get(2).unwrap().clone()).await;
    while let Ok(event) = ns_event_receiver.try_recv() {
        info!("{:?}", event);
        joining_node
            .consensus_ctx
            .handle_node_state_event(event)
            .await
            .expect("should handle data event");
    }

    // Process round
    node1.start_round_with_cut_from_mempool().await;

    simple_commit_round! {
        leader: node1.consensus_ctx,
        followers: [node2.consensus_ctx],
        joining: joining_node.consensus_ctx
    };

    std::mem::swap(&mut node1, &mut node2);

    // We still aren't caught up
    assert!(joining_node.consensus_ctx.is_joining());

    // Catch up
    for _ in 0..4 {
        let block = SignedBlock {
            data_proposals: vec![],
            certificate: AggregateSignature::default(),
            consensus_proposal: ConsensusProposal {
                slot: blocks.len() as u64,
                parent_hash: blocks[blocks.len() - 1].hash(),
                ..ConsensusProposal::default()
            },
        };
        da.handle_signed_block(block.clone()).await;
        blocks.push(block);
        while let Ok(event) = ns_event_receiver.try_recv() {
            info!("{:?}", event);
            joining_node
                .consensus_ctx
                .handle_node_state_event(event)
                .await
                .expect("should handle data event");
        }
    }

    // Process round
    node1.start_round_with_cut_from_mempool().await;

    simple_commit_round! {
        leader: node1.consensus_ctx,
        followers: [node2.consensus_ctx],
        joining: joining_node.consensus_ctx
    };

    // We are caught up
    assert!(!joining_node.consensus_ctx.is_joining());
}

#[test_log::test(tokio::test)]
async fn protocol_fees() {
    let (mut node1, mut node2, mut node3, mut node4) = build_nodes!(4).await;

    // First data proposal

    let register_tx = make_register_contract_tx(ContractName::new("test1"));
    let register_tx_2 = make_register_contract_tx(ContractName::new("test2"));

    node1.mempool_ctx.submit_tx(&register_tx);
    node1.mempool_ctx.submit_tx(&register_tx_2);

    node1
        .mempool_ctx
        .make_data_proposal_with_pending_txs()
        .expect("Should create data proposal");

    let msg = broadcast! {
        description: "Disseminate Tx",
        from: node1.mempool_ctx, to: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx],
        message_matches: MempoolNetMessage::DataProposal(_)
    };

    let dp = match msg.msg {
        MempoolNetMessage::DataProposal(dp) => dp,
        _ => panic!("Should be a DataProposal"),
    };

    join_all(
        [
            &mut node2.mempool_ctx,
            &mut node3.mempool_ctx,
            &mut node4.mempool_ctx,
        ]
        .iter_mut()
        .map(|ctx| ctx.handle_processed_data_proposals()),
    )
    .await;

    send! {
        description: "Disseminated Tx Vote",
        from: [node2.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx], to: node1.mempool_ctx,
        message_matches: MempoolNetMessage::DataVote(_, _)
    };

    node1.mempool_ctx.assert_broadcast("poda update");
    node1.mempool_ctx.assert_broadcast("poda update");

    let dp_size_1 = LaneBytesSize(dp.estimate_size() as u64);
    assert_eq!(node1.mempool_ctx.current_size(), Some(dp_size_1));
    assert_eq!(
        node1
            .mempool_ctx
            .current_size_of(node2.mempool_ctx.validator_pubkey()),
        None
    );

    assert_eq!(node2.mempool_ctx.current_size(), None);
    assert_eq!(
        node2
            .mempool_ctx
            .current_size_of(node1.mempool_ctx.validator_pubkey()),
        Some(dp_size_1)
    );
    assert_eq!(
        node3
            .mempool_ctx
            .current_size_of(node1.mempool_ctx.validator_pubkey()),
        Some(dp_size_1)
    );
    assert_eq!(
        node4
            .mempool_ctx
            .current_size_of(node1.mempool_ctx.validator_pubkey()),
        Some(dp_size_1)
    );

    // Second data proposal

    let register_tx = make_register_contract_tx(ContractName::new("test3"));
    let register_tx_2 = make_register_contract_tx(ContractName::new("test4"));
    let register_tx_3 = make_register_contract_tx(ContractName::new("test5"));

    node2.mempool_ctx.submit_tx(&register_tx);
    node2.mempool_ctx.submit_tx(&register_tx_2);
    node2.mempool_ctx.submit_tx(&register_tx_3);

    node2
        .mempool_ctx
        .make_data_proposal_with_pending_txs()
        .expect("Should create data proposal");

    let msg = broadcast! {
        description: "Disseminate Tx",
        from: node2.mempool_ctx, to: [node1.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx],
        message_matches: MempoolNetMessage::DataProposal(_)
    };
    let dp = match msg.msg {
        MempoolNetMessage::DataProposal(dp) => dp,
        _ => panic!("Should be a DataProposal"),
    };

    join_all(
        [
            &mut node1.mempool_ctx,
            &mut node3.mempool_ctx,
            &mut node4.mempool_ctx,
        ]
        .iter_mut()
        .map(|ctx| ctx.handle_processed_data_proposals()),
    )
    .await;

    send! {
        description: "Disseminated Tx Vote",
        from: [node1.mempool_ctx, node3.mempool_ctx, node4.mempool_ctx], to: node2.mempool_ctx,
        message_matches: MempoolNetMessage::DataVote(_, _)
    };

    let dp_size_2 = LaneBytesSize(dp.estimate_size() as u64);
    assert_eq!(node2.mempool_ctx.current_size(), Some(dp_size_2));
    assert_eq!(
        node2
            .mempool_ctx
            .current_size_of(node1.mempool_ctx.validator_pubkey()),
        Some(dp_size_1)
    );

    assert_eq!(node1.mempool_ctx.current_size(), Some(dp_size_1));
    assert_eq!(
        node1
            .mempool_ctx
            .current_size_of(node2.mempool_ctx.validator_pubkey()),
        Some(dp_size_2)
    );

    assert_eq!(
        node3
            .mempool_ctx
            .current_size_of(node1.mempool_ctx.validator_pubkey()),
        Some(dp_size_1)
    );
    assert_eq!(
        node3
            .mempool_ctx
            .current_size_of(node2.mempool_ctx.validator_pubkey()),
        Some(dp_size_2)
    );

    // Process poda update coming from node2
    let poda_update = node2.mempool_ctx.assert_broadcast("poda update");
    node1.mempool_ctx.handle_poda_update(poda_update);

    // Let's do a consensus round

    node1.start_round_with_cut_from_mempool().await;
    let prepare = broadcast! {
        description: "Prepare",
        from: node1.consensus_ctx, to: [node2.consensus_ctx, node3.consensus_ctx, node4.consensus_ctx],
        message_matches: ConsensusNetMessage::Prepare(_, _)
    };
    let cp = match prepare.msg {
        ConsensusNetMessage::Prepare(cp, _ticket) => cp,
        _ => panic!("Should be a Prepare"),
    };

    assert_eq!(cp.staking_actions.len(), 2);
    assert_eq!(
        cp.staking_actions[0],
        ConsensusStakingAction::PayFeesForDaDi {
            disseminator: node2.mempool_ctx.validator_pubkey().clone(),
            cumul_size: dp_size_2
        }
    );
    assert_eq!(
        cp.staking_actions[1],
        ConsensusStakingAction::PayFeesForDaDi {
            disseminator: node1.mempool_ctx.validator_pubkey().clone(),
            cumul_size: dp_size_1
        }
    );
}
