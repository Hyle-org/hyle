use alloc::string::String;
use hyle_model::{BlobIndex, Calldata, ContractName, Secp256k1Blob};
use sha2::{Digest, Sha256};

pub struct CheckSecp256k1<'a> {
    contract_input: &'a Calldata,
    expected_data: &'a [u8],
    blob_index: Option<BlobIndex>,
}

impl<'a> CheckSecp256k1<'a> {
    pub fn new(contract_input: &'a Calldata, expected_data: &'a [u8]) -> Self {
        Self {
            contract_input,
            expected_data,
            blob_index: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_blob_index(mut self, blob_index: BlobIndex) -> Self {
        self.blob_index = Some(blob_index);
        self
    }

    pub fn expect(self) -> Result<(), &'static str> {
        // Verify Secp256k1Blob
        let secp_blob = match self.blob_index {
            Some(idx) => {
                let blob = self
                    .contract_input
                    .blobs
                    .get(&idx)
                    .ok_or("Invalid blob index for secp256k1")?;
                if blob.contract_name != ContractName(String::from("secp256k1")) {
                    return Err("Invalid contract name for Secp256k1Blob");
                }
                blob
            }
            None => self
                .contract_input
                .blobs
                .iter()
                .map(|(_, b)| b)
                .find(|b| b.contract_name == ContractName(String::from("secp256k1")))
                .ok_or("Missing Secp256k1Blob")?,
        };

        let secp_data: Secp256k1Blob =
            borsh::from_slice(&secp_blob.data.0).map_err(|_| "Failed to decode Secp256k1Blob")?;

        // Verify that the identity matches the user
        if secp_data.identity != self.contract_input.identity {
            return Err("Secp256k1Blob identity does not match");
        }

        let mut hasher = Sha256::new();
        hasher.update(self.expected_data);
        let message_hash: [u8; 32] = hasher.finalize().into();

        if secp_data.data != message_hash {
            return Err("Secp256k1Blob data does not match");
        }

        Ok(())
    }
}
