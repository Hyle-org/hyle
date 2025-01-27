#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;

pub mod caller;
pub mod erc20;
pub mod guest;
pub mod identity_provider;
pub mod utils;

// re-export hyle-model
pub use hyle_model::*;

#[cfg(feature = "tracing")]
pub use tracing;

// Si la feature "tracing" est activée, on redirige vers `tracing::info!`
#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::tracing::info!($($arg)*);
    }
}

// Si la feature "tracing" n’est pas activée, on redirige vers la fonction env::log
#[cfg(all(not(feature = "tracing"), feature = "risc0"))]
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        risc0_zkvm::guest::env::log(&format!($($arg)*));
    }
}

#[cfg(all(not(feature = "tracing"), not(feature = "risc0")))]
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        println!($($arg)*);
    }
}

pub type RunResult = Result<String, String>;

pub const fn to_u8_array(val: &[u32; 8]) -> [u8; 32] {
    [
        (val[0] & 0xFF) as u8,
        ((val[0] >> 8) & 0xFF) as u8,
        ((val[0] >> 16) & 0xFF) as u8,
        ((val[0] >> 24) & 0xFF) as u8,
        (val[1] & 0xFF) as u8,
        ((val[1] >> 8) & 0xFF) as u8,
        ((val[1] >> 16) & 0xFF) as u8,
        ((val[1] >> 24) & 0xFF) as u8,
        (val[2] & 0xFF) as u8,
        ((val[2] >> 8) & 0xFF) as u8,
        ((val[2] >> 16) & 0xFF) as u8,
        ((val[2] >> 24) & 0xFF) as u8,
        (val[3] & 0xFF) as u8,
        ((val[3] >> 8) & 0xFF) as u8,
        ((val[3] >> 16) & 0xFF) as u8,
        ((val[3] >> 24) & 0xFF) as u8,
        (val[4] & 0xFF) as u8,
        ((val[4] >> 8) & 0xFF) as u8,
        ((val[4] >> 16) & 0xFF) as u8,
        ((val[4] >> 24) & 0xFF) as u8,
        (val[5] & 0xFF) as u8,
        ((val[5] >> 8) & 0xFF) as u8,
        ((val[5] >> 16) & 0xFF) as u8,
        ((val[5] >> 24) & 0xFF) as u8,
        (val[6] & 0xFF) as u8,
        ((val[6] >> 8) & 0xFF) as u8,
        ((val[6] >> 16) & 0xFF) as u8,
        ((val[6] >> 24) & 0xFF) as u8,
        (val[7] & 0xFF) as u8,
        ((val[7] >> 8) & 0xFF) as u8,
        ((val[7] >> 16) & 0xFF) as u8,
        ((val[7] >> 24) & 0xFF) as u8,
    ]
}

const fn byte_to_u8(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0,
    }
}

pub const fn str_to_u8(s: &str) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let chrs = s.as_bytes();
    bytes[0] = byte_to_u8(chrs[0]) << 4 | byte_to_u8(chrs[1]);
    bytes[1] = byte_to_u8(chrs[2]) << 4 | byte_to_u8(chrs[3]);
    bytes[2] = byte_to_u8(chrs[4]) << 4 | byte_to_u8(chrs[5]);
    bytes[3] = byte_to_u8(chrs[6]) << 4 | byte_to_u8(chrs[7]);
    bytes[4] = byte_to_u8(chrs[8]) << 4 | byte_to_u8(chrs[9]);
    bytes[5] = byte_to_u8(chrs[10]) << 4 | byte_to_u8(chrs[11]);
    bytes[6] = byte_to_u8(chrs[12]) << 4 | byte_to_u8(chrs[13]);
    bytes[7] = byte_to_u8(chrs[14]) << 4 | byte_to_u8(chrs[15]);
    bytes[8] = byte_to_u8(chrs[16]) << 4 | byte_to_u8(chrs[17]);
    bytes[9] = byte_to_u8(chrs[18]) << 4 | byte_to_u8(chrs[19]);
    bytes[10] = byte_to_u8(chrs[20]) << 4 | byte_to_u8(chrs[21]);
    bytes[11] = byte_to_u8(chrs[22]) << 4 | byte_to_u8(chrs[23]);
    bytes[12] = byte_to_u8(chrs[24]) << 4 | byte_to_u8(chrs[25]);
    bytes[13] = byte_to_u8(chrs[26]) << 4 | byte_to_u8(chrs[27]);
    bytes[14] = byte_to_u8(chrs[28]) << 4 | byte_to_u8(chrs[29]);
    bytes[15] = byte_to_u8(chrs[30]) << 4 | byte_to_u8(chrs[31]);
    bytes[16] = byte_to_u8(chrs[32]) << 4 | byte_to_u8(chrs[33]);
    bytes[17] = byte_to_u8(chrs[34]) << 4 | byte_to_u8(chrs[35]);
    bytes[18] = byte_to_u8(chrs[36]) << 4 | byte_to_u8(chrs[37]);
    bytes[19] = byte_to_u8(chrs[38]) << 4 | byte_to_u8(chrs[39]);
    bytes[20] = byte_to_u8(chrs[40]) << 4 | byte_to_u8(chrs[41]);
    bytes[21] = byte_to_u8(chrs[42]) << 4 | byte_to_u8(chrs[43]);
    bytes[22] = byte_to_u8(chrs[44]) << 4 | byte_to_u8(chrs[45]);
    bytes[23] = byte_to_u8(chrs[46]) << 4 | byte_to_u8(chrs[47]);
    bytes[24] = byte_to_u8(chrs[48]) << 4 | byte_to_u8(chrs[49]);
    bytes[25] = byte_to_u8(chrs[50]) << 4 | byte_to_u8(chrs[51]);
    bytes[26] = byte_to_u8(chrs[52]) << 4 | byte_to_u8(chrs[53]);
    bytes[27] = byte_to_u8(chrs[54]) << 4 | byte_to_u8(chrs[55]);
    bytes[28] = byte_to_u8(chrs[56]) << 4 | byte_to_u8(chrs[57]);
    bytes[29] = byte_to_u8(chrs[58]) << 4 | byte_to_u8(chrs[59]);
    bytes[30] = byte_to_u8(chrs[60]) << 4 | byte_to_u8(chrs[61]);
    bytes[31] = byte_to_u8(chrs[62]) << 4 | byte_to_u8(chrs[63]);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{format, string::ToString, vec};

    #[test]
    fn test_identity_from_string() {
        let identity_str = "test_identity".to_string();
        let identity = Identity::from(identity_str.clone());
        assert_eq!(identity.0, identity_str);
    }

    #[test]
    fn test_identity_from_str() {
        let identity_str = "test_identity";
        let identity = Identity::from(identity_str);
        assert_eq!(identity.0, identity_str.to_string());
    }

    #[test]
    fn test_txhash_from_string() {
        let txhash_str = "test_txhash".to_string();
        let txhash = TxHash::from(txhash_str.clone());
        assert_eq!(txhash.0, txhash_str);
    }

    #[test]
    fn test_txhash_from_str() {
        let txhash_str = "test_txhash";
        let txhash = TxHash::from(txhash_str);
        assert_eq!(txhash.0, txhash_str.to_string());
    }

    #[test]
    fn test_txhash_new() {
        let txhash_str = "test_txhash";
        let txhash = TxHash::new(txhash_str);
        assert_eq!(txhash.0, txhash_str.to_string());
    }

    #[test]
    fn test_blobindex_from_u32() {
        let index = 42;
        let blob_index = BlobIndex::from(index);
        assert_eq!(blob_index.0, index);
    }

    #[test]
    fn test_txhash_display() {
        let txhash_str = "test_txhash";
        let txhash = TxHash::new(txhash_str);
        assert_eq!(format!("{}", txhash), txhash_str);
    }

    #[test]
    fn test_blobindex_display() {
        let index = 42;
        let blob_index = BlobIndex::from(index);
        assert_eq!(format!("{}", blob_index), index.to_string());
    }

    #[test]
    fn test_state_digest_encoding() {
        let state_digest = StateDigest(vec![1, 2, 3, 4]);
        let encoded = bincode::encode_to_vec(&state_digest, bincode::config::standard())
            .expect("Failed to encode StateDigest");
        let decoded: StateDigest =
            bincode::decode_from_slice(&encoded, bincode::config::standard())
                .expect("Failed to decode StateDigest")
                .0;
        assert_eq!(state_digest, decoded);
    }

    #[test]
    fn test_identity_encoding() {
        let identity = Identity::new("test_identity");
        let encoded = bincode::encode_to_vec(&identity, bincode::config::standard())
            .expect("Failed to encode Identity");
        let decoded: Identity = bincode::decode_from_slice(&encoded, bincode::config::standard())
            .expect("Failed to decode Identity")
            .0;
        assert_eq!(identity, decoded);
    }

    #[test]
    fn test_txhash_encoding() {
        let txhash = TxHash::new("test_txhash");
        let encoded = bincode::encode_to_vec(&txhash, bincode::config::standard())
            .expect("Failed to encode TxHash");
        let decoded: TxHash = bincode::decode_from_slice(&encoded, bincode::config::standard())
            .expect("Failed to decode TxHash")
            .0;
        assert_eq!(txhash, decoded);
    }

    #[test]
    fn test_blobindex_encoding() {
        let blob_index = BlobIndex(42);
        let encoded = bincode::encode_to_vec(blob_index, bincode::config::standard())
            .expect("Failed to encode BlobIndex");
        let decoded: BlobIndex = bincode::decode_from_slice(&encoded, bincode::config::standard())
            .expect("Failed to decode BlobIndex")
            .0;
        assert_eq!(blob_index, decoded);
    }

    #[test]
    fn test_blobdata_encoding() {
        let blob_data = BlobData(vec![1, 2, 3, 4]);
        let encoded = bincode::encode_to_vec(&blob_data, bincode::config::standard())
            .expect("Failed to encode BlobData");
        let decoded: BlobData = bincode::decode_from_slice(&encoded, bincode::config::standard())
            .expect("Failed to decode BlobData")
            .0;
        assert_eq!(blob_data, decoded);
    }
}
