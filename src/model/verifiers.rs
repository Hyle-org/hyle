use hyle_contract_sdk::{
    Blob, BlobData, BlobIndex, ContractAction, ContractName, Identity, ProgramId, Verifier,
};

#[derive(Debug, Copy, Clone)]
pub enum NativeVerifiers {
    Blst,
    Sha3_256,
    Secp256k1,
}

impl From<NativeVerifiers> for ProgramId {
    fn from(value: NativeVerifiers) -> Self {
        match value {
            NativeVerifiers::Blst => ProgramId("blst".as_bytes().to_vec()),
            NativeVerifiers::Sha3_256 => ProgramId("sha3_256".as_bytes().to_vec()),
            NativeVerifiers::Secp256k1 => ProgramId("secp256k1".as_bytes().to_vec()),
        }
    }
}

impl TryFrom<&Verifier> for NativeVerifiers {
    type Error = String;
    fn try_from(value: &Verifier) -> Result<Self, Self::Error> {
        match value.0.as_str() {
            "blst" => Ok(Self::Blst),
            "sha3_256" => Ok(Self::Sha3_256),
            "secp256k1" => Ok(Self::Secp256k1),
            _ => Err(format!("Unknown native verifier: {}", value)),
        }
    }
}

/// Format of the BlobData for native contract "blst"
#[derive(Debug, borsh::BorshSerialize, borsh::BorshDeserialize)]
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
        #[allow(clippy::expect_used)]
        Blob {
            contract_name,
            data: BlobData(borsh::to_vec(self).expect("failed to encode BlstSignatureBlob")),
        }
    }
}

/// Format of the BlobData for native hash contract like "sha3_256"
#[derive(Debug, borsh::BorshSerialize, borsh::BorshDeserialize)]
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
        #[allow(clippy::expect_used)]
        Blob {
            contract_name,
            data: BlobData(borsh::to_vec(self).expect("failed to encode ShaBlob")),
        }
    }
}

/// Format of the BlobData for native secp256k1 contract
#[derive(Debug, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct Secp256k1Blob {
    pub identity: Identity,
    pub data: [u8; 32],
    pub public_key: [u8; 33],
    pub signature: [u8; 64],
}

impl Secp256k1Blob {
    pub fn as_blob(&self) -> Blob {
        <Self as ContractAction>::as_blob(self, "secp256k1".into(), None, None)
    }
}

impl ContractAction for Secp256k1Blob {
    fn as_blob(
        &self,
        contract_name: ContractName,
        _caller: Option<BlobIndex>,
        _callees: Option<Vec<BlobIndex>>,
    ) -> Blob {
        #[allow(clippy::expect_used)]
        Blob {
            contract_name,
            data: BlobData(borsh::to_vec(self).expect("failed to encode Secp256k1Blob")),
        }
    }
}
