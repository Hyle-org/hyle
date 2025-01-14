use clap::Parser;

use hyle_test::{conf_maker::ConfMaker, node_process::TestProcess};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ImageExt},
};

#[derive(Debug, Parser)]
#[command(name = "hypervisor")]
#[command(about = "A simple CLI to spin up nodes", long_about = None)]
struct Args {
    #[arg(long, default_value = "4")]
    pub count: u32,

    #[arg(long, default_value = "4")]
    pub genesis_count: u32,

    #[arg(long, default_value = "500")]
    pub slot_duration: u64,

    #[arg(long)]
    pub log: Option<Vec<String>>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    if args.count < args.genesis_count {
        panic!("Genesis count cannot be greater than the total count");
    }

    // Start postgres DB with default settings for the indexer.
    let _ = Postgres::default()
        .with_cmd(["postgres", "-c", "log_statement=all"])
        .start()
        .await
        .unwrap();

    let mut nodes = Vec::new();
    let mut peers = Vec::new();
    let mut confs = Vec::new();
    let mut genesis_stakers = std::collections::HashMap::new();

    let count = args.count;
    let mut conf_maker = ConfMaker::default();
    conf_maker.default.consensus.slot_duration = args.slot_duration;
    conf_maker.default.single_node = Some(false);
    conf_maker.default.run_indexer = false;
    conf_maker.default.consensus.genesis_stakers = std::collections::HashMap::new();

    for i in 0..count {
        let node_conf = conf_maker.build("node");
        if i < args.genesis_count {
            genesis_stakers.insert(node_conf.id.clone(), 100);
        }
        peers.push(node_conf.host.clone());
        confs.push(node_conf);
    }

    let log = confs
        .iter()
        .filter(|c| {
            args.log
                .as_ref()
                .map(|l| l.contains(&c.id) || l == &["all"])
                .unwrap_or(false)
        })
        .map(|c| c.id.clone())
        .collect::<Vec<_>>();

    for node_conf in confs.iter_mut() {
        if !node_conf
            .consensus
            .genesis_stakers
            .contains_key(&node_conf.id)
        {
            node_conf.peers = peers.get(0..1).unwrap().to_vec();
        } else {
            node_conf.peers = peers.clone();
        }
        node_conf.consensus.genesis_stakers = genesis_stakers.clone();
        let node = TestProcess::new("node", node_conf.clone());
        if log.contains(&node_conf.id) {
            nodes.push(
                node.log(&std::env::var("HYLE_LOG").unwrap_or("hyle=info".to_string()))
                    .start(),
            );
        } else {
            nodes.push(node.log("hyle=error,risc0_zkvm=error").start());
        }
    }

    use tokio::signal::unix;
    let mut terminate = unix::signal(unix::SignalKind::interrupt()).unwrap();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Ctrl-C received, shutting down");
        }
        _ = terminate.recv() =>  {
            tracing::info!("SIGTERM received, shutting down");
        }
    }

    let mut shutdown = tokio::task::JoinSet::new();
    for mut node in nodes.drain(..) {
        shutdown.spawn(async move {
            node.stop().await.unwrap();
        });
    }
    shutdown.join_all().await;
}
