use crate::{Blob, BlobData, BlobIndex, ContractAction, ContractName, Identity};

pub const RISC0_1: &str = "risc0-1";
pub const NOIR: &str = "noir";
pub const SP1_4: &str = "sp1-4";

/// Format of the BlobData for native secp256k1 contract
#[derive(Debug, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct Secp256k1Blob {
    pub identity: Identity,
    pub data: [u8; 32],
    pub public_key: [u8; 33],
    pub signature: [u8; 64],
}

impl Secp256k1Blob {
    #[cfg(feature = "full")]
    /// Allow to create a Secp256k1Blob from the data, public_key and signature
    pub fn new(
        identity: Identity,
        data: &[u8],
        public_key: &str,
        signature: &str,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;
        use sha2::Digest;

        let public_key = secp256k1::PublicKey::from_slice(
            &hex::decode(public_key).context("invalid public_key format")?,
        )
        .context("cannot parse public_key")?
        .serialize();

        let signature = secp256k1::ecdsa::Signature::from_der(
            &hex::decode(signature).context("invalid signature format")?,
        )
        .context("canot parse signature")?
        .serialize_compact();

        let mut hasher = sha2::Sha256::new();
        hasher.update(data);
        let data: [u8; 32] = hasher.finalize().into();

        Ok(Self {
            identity,
            data,
            public_key,
            signature,
        })
    }

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
