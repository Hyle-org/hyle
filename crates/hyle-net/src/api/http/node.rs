use anyhow::{Context, Result};
use http_body_util::BodyExt;
use hyper::client::conn::http1;
use hyper::{Method, Request, Uri};
pub use hyper_util::rt::TokioIo;
use sdk::{
    api::*, BlobTransaction, BlockHeight, ConsensusInfo, Contract, ContractName, ProofTransaction,
    TxHash, UnsettledBlobTransaction,
};
use serde::{de::DeserializeOwned, Serialize};

#[derive(Clone)]
pub struct NodeApiHttpClient {
    pub url: Uri,
    pub api_key: Option<String>,
}

impl NodeApiHttpClient {
    pub fn new(url: String) -> Result<NodeApiHttpClient> {
        Ok(NodeApiHttpClient {
            url: url.parse()?,
            api_key: None,
        })
    }

    pub async fn register_contract(&self, tx: &APIRegisterContract) -> Result<TxHash> {
        self.request::<APIRegisterContract, TxHash>(
            "v1/contract/register",
            Method::POST,
            "application/json".to_string(),
            Some(tx),
            "Registering contract",
        )
        .await
    }

    pub async fn send_tx_blob(&self, tx: &BlobTransaction) -> Result<TxHash> {
        self.request::<BlobTransaction, TxHash>(
            "v1/tx/send/blob",
            Method::POST,
            "application/json".to_string(),
            Some(tx),
            "Sending tx blob",
        )
        .await
    }

    pub async fn send_tx_proof(&self, tx: &ProofTransaction) -> Result<TxHash> {
        self.request::<ProofTransaction, TxHash>(
            "v1/tx/send/proof",
            Method::POST,
            "application/json".to_string(),
            Some(tx),
            "Sending tx proof",
        )
        .await
    }

    pub async fn get_consensus_info(&self) -> Result<ConsensusInfo> {
        self.request::<(), ConsensusInfo>(
            "v1/consensus/info",
            Method::GET,
            "application/json".to_string(),
            None,
            "getting consensus info",
        )
        .await
    }

    pub async fn get_consensus_staking_state(&self) -> Result<APIStaking> {
        self.request::<(), APIStaking>(
            "v1/consensus/staking_state",
            Method::GET,
            "application/json".to_string(),
            None,
            "getting consensus staking state",
        )
        .await
    }

    pub async fn get_node_info(&self) -> Result<NodeInfo> {
        self.request::<(), NodeInfo>(
            "v1/info",
            Method::GET,
            "application/json".to_string(),
            None,
            "getting node info",
        )
        .await
    }

    pub async fn metrics(&self) -> Result<String> {
        self.request::<(), String>(
            "v1/metrics",
            Method::GET,
            "application/text".to_string(),
            None,
            "getting node metrics",
        )
        .await
    }

    pub async fn get_block_height(&self) -> Result<BlockHeight> {
        self.request::<(), BlockHeight>(
            "v1/da/block/height",
            Method::GET,
            "application/json".to_string(),
            None,
            "getting block height",
        )
        .await
    }

    pub async fn get_contract(&self, contract_name: &ContractName) -> Result<Contract> {
        self.request::<(), Contract>(
            &format!("v1/contract/{}", contract_name),
            Method::GET,
            "application/json".to_string(),
            None,
            &format!("getting contract {}", contract_name),
        )
        .await
    }

    pub async fn get_unsettled_tx(
        &self,
        blob_tx_hash: &TxHash,
    ) -> Result<UnsettledBlobTransaction> {
        self.request::<String, UnsettledBlobTransaction>(
            &format!("v1/unsettled_tx/{blob_tx_hash}"),
            Method::GET,
            "application/json".to_string(),
            None,
            &format!("getting tx {}", blob_tx_hash),
        )
        .await
    }
    pub async fn request<T, R>(
        &self,
        endpoint: &str,
        method: Method,
        content_type: String,
        body: Option<&T>,
        context_msg: &str,
    ) -> Result<R>
    where
        T: Serialize,
        R: DeserializeOwned,
    {
        // Construire l'URI complète
        let full_url = format!("{}{}", self.url, endpoint);
        let uri: Uri = full_url.parse().context("URI invalide")?;

        // Établir une connexion TCP
        let authority = uri.authority().context("URI sans autorité")?.clone();
        let host = authority.host().to_string();
        let port = authority.port_u16().unwrap_or(80);
        let addr = format!("{}:{}", host, port);
        let stream = crate::net::TcpStream::connect(addr)
            .await
            .context("Échec de la connexion TCP")?;
        let io = TokioIo::new(stream);

        // Initialiser le client HTTP
        let (mut sender, connection) = http1::handshake(io)
            .await
            .context("Échec de la poignée de main HTTP/1")?;

        // Lancer la tâche pour gérer la connexion
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("Erreur de connexion: {:?}", e);
            }
        });

        // Construire la requête HTTP
        let mut req_builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(hyper::header::CONTENT_TYPE, content_type);

        // Ajouter la clé API si présente
        if let Some(ref key) = self.api_key {
            req_builder = req_builder.header("X-API-KEY", key);
        }

        let request = if let Some(b) = body {
            let json_body = serde_json::to_string(b)
                .context("Échec de la sérialisation du corps de la requête")?;

            req_builder
                .body(json_body)
                .context("Échec de la construction de la requête")?
        } else {
            req_builder.body("".to_string())?
        };

        // Envoyer la requête et obtenir la réponse
        let response = sender
            .send_request(request)
            .await
            .context(format!("{}: échec de la requête", context_msg))?;

        // Vérifier le statut HTTP
        if !response.status().is_success() {
            anyhow::bail!(
                "{}: échec avec le statut {}",
                context_msg,
                response.status()
            );
        }

        // Lire et désérialiser le corps de la réponse
        let mut body = response.into_body();

        let mut response: Vec<u8> = vec![];

        // Stream the body, writing each chunk to stdout as we get it
        // (instead of buffering and printing at the end).
        while let Some(next) = body.frame().await {
            let frame = next?;
            if let Some(chunk) = frame.data_ref() {
                response.extend(chunk);
            }
        }

        let result: R = serde_json::from_slice(&response).context(format!(
            "{}: échec de la désérialisation de la réponse",
            context_msg
        ))?;

        Ok(result)
    }
}

// impl IndexerApiHttpClient {
//     pub fn new(url: String) -> Result<Self> {
//         Ok(Self {
//             url: Url::parse(&url)?,
//             reqwest_client: reqwest::Client::new(),
//         })
//     }

//     pub async fn list_contracts(&self) -> Result<Vec<APIContract>> {
//         self.get("v1/indexer/contracts", "listing contracts").await
//     }

//     pub async fn get_indexer_contract(&self, contract_name: &ContractName) -> Result<APIContract> {
//         self.get(
//             &format!("v1/indexer/contract/{contract_name}"),
//             &format!("getting contract {contract_name}"),
//         )
//         .await
//     }

//     pub async fn fetch_current_state<State>(&self, contract_name: &ContractName) -> Result<State>
//     where
//         State: serde::de::DeserializeOwned,
//     {
//         self.get::<State>(
//             &format!("v1/indexer/contract/{contract_name}/state"),
//             &format!("getting contract {contract_name} state"),
//         )
//         .await
//     }

//     pub async fn get_blocks(&self) -> Result<Vec<APIBlock>> {
//         self.get("v1/indexer/blocks", "getting blocks").await
//     }

//     pub async fn get_last_block(&self) -> Result<APIBlock> {
//         self.get("v1/indexer/block/last", "getting last block")
//             .await
//     }

//     pub async fn get_block_by_height(&self, height: &BlockHeight) -> Result<APIBlock> {
//         self.get(
//             &format!("v1/indexer/block/height/{height}"),
//             &format!("getting block with height {height}"),
//         )
//         .await
//     }

//     pub async fn get_block_by_hash(&self, hash: &BlockHash) -> Result<APIBlock> {
//         self.get(
//             &format!("v1/indexer/block/hash/{hash}"),
//             &format!("getting block with hash {hash}"),
//         )
//         .await
//     }

//     pub async fn get_transactions(&self) -> Result<Vec<APITransaction>> {
//         self.get("v1/indexer/transactions", "getting transactions")
//             .await
//     }

//     pub async fn get_transactions_by_height(
//         &self,
//         height: &BlockHeight,
//     ) -> Result<Vec<APITransaction>> {
//         self.get(
//             &format!("v1/indexer/transactions/block/{height}"),
//             &format!("getting transactions for block height {height}"),
//         )
//         .await
//     }

//     pub async fn get_transactions_by_contract(
//         &self,
//         contract_name: &ContractName,
//     ) -> Result<Vec<APITransaction>> {
//         self.get(
//             &format!("v1/indexer/transactions/contract/{contract_name}"),
//             &format!("getting transactions for contract {contract_name}"),
//         )
//         .await
//     }

//     pub async fn get_transaction_with_hash(&self, tx_hash: &TxHash) -> Result<APITransaction> {
//         self.get(
//             &format!("v1/indexer/transaction/hash/{tx_hash}"),
//             &format!("getting transaction with hash {tx_hash}"),
//         )
//         .await
//     }

//     pub async fn get_blob_transactions_by_contract(
//         &self,
//         contract_name: &ContractName,
//     ) -> Result<Vec<TransactionWithBlobs>> {
//         self.get(
//             &format!("v1/indexer/blob_transactions/contract/{contract_name}"),
//             &format!("getting blob transactions for contract {contract_name}"),
//         )
//         .await
//     }

//     pub async fn get_blobs_by_tx_hash(&self, tx_hash: &TxHash) -> Result<Vec<APIBlob>> {
//         self.get(
//             &format!("v1/indexer/blobs/hash/{tx_hash}"),
//             &format!("getting blob by transaction hash {tx_hash}"),
//         )
//         .await
//     }

//     pub async fn get_blob(&self, tx_hash: &TxHash, blob_index: BlobIndex) -> Result<APIBlob> {
//         self.get(
//             &format!("v1/indexer/blob/hash/{tx_hash}/index/{blob_index}"),
//             &format!("getting blob with hash {tx_hash} and index {blob_index}"),
//         )
//         .await
//     }

//     async fn get<T>(&self, endpoint: &str, context_msg: &str) -> Result<T>
//     where
//         T: serde::de::DeserializeOwned,
//     {
//         self.reqwest_client
//             .get(format!("{}{}", self.url, endpoint))
//             .header("Content-Type", "application/json")
//             .send()
//             .await
//             .context(format!("{} request failed", context_msg))?
//             .json::<T>()
//             .await
//             .context(format!("Failed to deserialize {}", context_msg))
//     }
// }
