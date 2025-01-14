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

#[derive(Debug, Copy, Clone)]
pub enum NativeVerifiers {
    Blst,
    Sha3_256,
}

impl From<NativeVerifiers> for ProgramId {
    fn from(value: NativeVerifiers) -> Self {
        match value {
            NativeVerifiers::Blst => ProgramId("blst".as_bytes().to_vec()),
            NativeVerifiers::Sha3_256 => ProgramId("sha3_256".as_bytes().to_vec()),
        }
    }
}

impl TryFrom<&Verifier> for NativeVerifiers {
    type Error = String;
    fn try_from(value: &Verifier) -> Result<Self, Self::Error> {
        match value.0.as_str() {
            "blst" => Ok(Self::Blst),
            "sha3_256" => Ok(Self::Sha3_256),
            _ => Err(format!("Unknown native verifier: {}", value)),
        }
    }
}

/// Format of the BlobData for native contract "blst"
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

/// Format of the BlobData for native hash contract like "sha3_256"
#[derive(Debug, bincode::Encode, bincode::Decode)]
pub struct ShaBlob {
    pub identity: Identity,
    pub data: Vec<u8>,
    pub sha: Vec<u8>,
}

impl ShaBlob {
    pub fn as_blob(&self, contract_name: ContractName) -> Blob {
        <Self as ContractAction>::as_blob(self, contract_name, None, None)
    }
}

impl ContractAction for ShaBlob {
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
