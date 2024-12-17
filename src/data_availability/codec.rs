use anyhow::Context;
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

use crate::model::{Block, BlockHeight};

// Server Side
#[derive(Default, Debug)]
pub struct DataAvailibilityServerCodec {
    ldc: LengthDelimitedCodec,
}

pub enum DataAvailibilityServerRequest {
    BlockHeight(BlockHeight),
    Ping,
}

impl Decoder for DataAvailibilityServerCodec {
    type Item = DataAvailibilityServerRequest;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let decoded_bytes = self.ldc.decode(src)?;

        // try decode ping
        if let Some(decoded_bytes) = decoded_bytes {
            if decoded_bytes == *"ok" {
                return Ok(Some(DataAvailibilityServerRequest::Ping));
            }

            let height: u64 =
                bincode::decode_from_slice(&decoded_bytes, bincode::config::standard())
                    .context(format!(
                        "Decoding height from {} bytes",
                        decoded_bytes.len()
                    ))?
                    .0;

            return Ok(Some(DataAvailibilityServerRequest::BlockHeight(
                BlockHeight(height),
            )));
        }

        Ok(None)
    }
}

impl Encoder<Block> for DataAvailibilityServerCodec {
    type Error = anyhow::Error;

    fn encode(&mut self, block: Block, dst: &mut bytes::BytesMut) -> Result<(), Self::Error> {
        let bytes: bytes::Bytes =
            bincode::encode_to_vec(block, bincode::config::standard())?.into();

        self.ldc
            .encode(bytes, dst)
            .context("Encoding block bytes as length delimited")
    }
}

// Client Side

#[derive(Default)]
pub struct DataAvailibilityClientCodec {
    ldc: LengthDelimitedCodec,
}
impl Decoder for DataAvailibilityClientCodec {
    type Item = Block;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let decoded_bytes = self.ldc.decode(src)?;
        if let Some(decoded_bytes) = decoded_bytes {
            let block: Block =
                bincode::decode_from_slice(&decoded_bytes, bincode::config::standard())
                    .context(format!("Decoding block from {} bytes", decoded_bytes.len()))?
                    .0;

            return Ok(Some(block));
        }
        Ok(None)
    }
}

impl Encoder<DataAvailibilityServerRequest> for DataAvailibilityClientCodec {
    type Error = anyhow::Error;

    fn encode(
        &mut self,
        request: DataAvailibilityServerRequest,
        dst: &mut bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        let bytes: bytes::Bytes = match request {
            DataAvailibilityServerRequest::BlockHeight(height) => {
                bincode::encode_to_vec(height, bincode::config::standard())?.into()
            }
            DataAvailibilityServerRequest::Ping => bytes::Bytes::from("ok"),
        };

        self.ldc
            .encode(bytes, dst)
            .context("Encoding block height bytes as length delimited")
    }
}
