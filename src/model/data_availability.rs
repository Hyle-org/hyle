use bincode::{Decode, Encode};

use crate::model::{BlobsHash, ContractName};
use hyle_contract_sdk::{
    Blob, BlobData, BlobIndex, ContractAction, HyleOutput, Identity, ProgramId, StateDigest,
    TxHash, Verifier,
};
use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub struct Contract {
    pub name: ContractName,
    pub program_id: ProgramId,
    pub state: StateDigest,
    pub verifier: Verifier,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct UnsettledBlobTransaction {
    pub identity: Identity,
    pub hash: TxHash,
    pub blobs_hash: BlobsHash,
    pub blobs: Vec<UnsettledBlobMetadata>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct UnsettledBlobMetadata {
    pub blob: Blob,
    // Each time we receive a proof, we add it to this list
    pub possible_proofs: Vec<(ProgramId, HyleOutput)>,
}

#[derive(
    Debug, Default, Clone, serde::Serialize, serde::Deserialize, Encode, Decode, Eq, PartialEq,
)]
pub struct HandledBlobProofOutput {
    pub proof_tx_hash: TxHash,
    pub blob_tx_hash: TxHash,
    pub blob_index: BlobIndex,
    pub contract_name: ContractName,
    pub hyle_output: HyleOutput,
    pub blob_proof_output_index: usize,
}

#[derive(Debug, bincode::Encode, bincode::Decode)]
pub struct NativeProof {
    pub tx_hash: TxHash,
    pub index: BlobIndex,
    pub blobs: Vec<Blob>,
}

#[derive(Debug, bincode::Encode, bincode::Decode)]
pub struct BlstSignatureBlob {
    pub identity: Identity,
    pub data: Vec<u8>,
    /// Signature for contatenated data + identity.as_bytes()
    pub signature: Vec<u8>,
    pub public_key: Vec<u8>,
}

impl BlstSignatureBlob {
    pub fn as_blob(&self) -> Blob {
        <Self as ContractAction>::as_blob(self, "blst".into(), None, None)
    }
}

impl ContractAction for BlstSignatureBlob {
    fn as_blob(
        &self,
        contract_name: ContractName,
        _caller: Option<BlobIndex>,
        _callees: Option<Vec<BlobIndex>>,
    ) -> Blob {
        Blob {
            contract_name,
            data: BlobData(
                bincode::encode_to_vec(self, bincode::config::standard())
                    .expect("failed to encode program inputs"),
            ),
        }
    }
}

#[derive(Debug, bincode::Encode, bincode::Decode)]
pub struct ShaBlob {
    pub identity: Identity,
    pub data: Vec<u8>,
    pub sha: Vec<u8>,
}
