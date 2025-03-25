use std::{sync::Arc, time::Duration};

use hyle::{
    entrypoint::main_process,
    utils::{conf::Conf, crypto::BlstCrypto},
};
use hyle_contract_sdk::info;
use hyle_net::api::NodeApiHttpClient;
use rand::{rngs::StdRng, RngCore, SeedableRng};
use tokio::time::timeout;

#[derive(Clone)]
pub struct TurmoilProcess<Connector: Clone> {
    pub conf: Conf,
    pub client: NodeApiHttpClient<Connector>,
}

impl<Connector: Clone> TurmoilProcess<Connector> {
    pub async fn start(&self) -> anyhow::Result<()> {
        let crypto = Arc::new(BlstCrypto::new(&self.conf.id).expect("Creating crypto"));

        main_process(self.conf.clone(), Some(crypto)).await?;

        Ok(())
    }
}

pub async fn wait_height<Connector: Clone>(
    client: &NodeApiHttpClient<Connector>,
    heights: u64,
) -> anyhow::Result<()> {
    wait_height_timeout(client, heights, 30).await
}

pub async fn wait_height_timeout<Connector: Clone>(
    client: &NodeApiHttpClient<Connector>,
    heights: u64,
    timeout_duration: u64,
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(timeout_duration), async {
        loop {
            if let Ok(mut current_height) = client.get_block_height().await {
                let target_height = current_height + heights;
                while current_height.0 < target_height.0 {
                    info!(
                        "⏰ Waiting for height {} to be reached. Current is {}",
                        target_height, current_height
                    );
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    current_height = client.get_block_height().await?;
                }
                return anyhow::Ok(());
            } else {
                info!("⏰ Waiting for node to be ready");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("Timeout reached while waiting for height: {e}"))?
}

/// Structure personnalisée qui encapsule un RNG seedable
pub struct MySeedableRng {
    rng: StdRng, // Utilisation d'un générateur déterministe basé sur une seed
}

impl MySeedableRng {
    /// Constructeur : Initialise un RNG avec une seed donnée
    pub fn new(seed: u64) -> Self {
        MySeedableRng {
            rng: StdRng::seed_from_u64(seed),
        }
    }
}

impl RngCore for MySeedableRng {
    /// Génère un nombre aléatoire de 32 bits
    fn next_u32(&mut self) -> u32 {
        self.rng.next_u32()
    }

    /// Génère un nombre aléatoire de 64 bits
    fn next_u64(&mut self) -> u64 {
        self.rng.next_u64()
    }

    /// Remplit un buffer de bytes avec des valeurs aléatoires
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.rng.fill_bytes(dest)
    }

    /// Remplit un buffer de bytes de manière non déterministe (optionnel)
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        self.rng.try_fill_bytes(dest)
    }
}
