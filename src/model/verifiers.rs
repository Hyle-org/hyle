use hyle_contract_sdk::{
    Blob, BlobData, BlobIndex, ContractAction, ContractName, Identity, ProgramId, Verifier,
};

#[derive(Debug, Copy, Clone)]
pub enum NativeVerifiers {
    Blst,
    Sha3_256,
    HmacSha256,
}

impl From<NativeVerifiers> for ProgramId {
    fn from(value: NativeVerifiers) -> Self {
        match value {
            NativeVerifiers::Blst => ProgramId("blst".as_bytes().to_vec()),
            NativeVerifiers::Sha3_256 => ProgramId("sha3_256".as_bytes().to_vec()),
            NativeVerifiers::HmacSha256 => ProgramId("hmac_sha256".as_bytes().to_vec()),
        }
    }
}

impl TryFrom<&Verifier> for NativeVerifiers {
    type Error = String;
    fn try_from(value: &Verifier) -> Result<Self, Self::Error> {
        match value.0.as_str() {
            "blst" => Ok(Self::Blst),
            "sha3_256" => Ok(Self::Sha3_256),
            "hmac_sha256" => Ok(Self::HmacSha256),
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

/// Format of the BlobData for native HMAC-SHA256 contract
#[derive(Debug, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct HmacSha256Blob {
    pub identity: Identity,
    pub data: Vec<u8>,
    pub key: Vec<u8>,
    pub hmac: Vec<u8>,
}

impl HmacSha256Blob {
    pub fn as_blob(&self) -> Blob {
        <Self as ContractAction>::as_blob(self, "hmac_sha256".into(), None, None)
    }
}

impl ContractAction for HmacSha256Blob {
    fn as_blob(
        &self,
        contract_name: ContractName,
        _caller: Option<BlobIndex>,
        _callees: Option<Vec<BlobIndex>>,
    ) -> Blob {
        #[allow(clippy::expect_used)]
        Blob {
            contract_name,
            data: BlobData(borsh::to_vec(self).expect("failed to encode HmacSha256Blob")),
        }
    }
}
