use anyhow::Context;
use borsh::{BorshDeserialize, BorshSerialize};
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

use crate::{
    mempool::MempoolStatusEvent,
    model::{BlockHeight, SignedBlock},
};

// Server Side
#[derive(Debug)]
pub struct DataAvailabilityServerCodec {
    ldc: LengthDelimitedCodec,
}

impl Default for DataAvailabilityServerCodec {
    fn default() -> Self {
        let mut ldc = LengthDelimitedCodec::new();
        ldc.set_max_frame_length(128 * 1024 * 1024); // Set max frame length to 128 Mb
        DataAvailabilityServerCodec { ldc }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataAvailabilityServerRequest {
    BlockHeight(BlockHeight),
    Ping,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum DataAvailabilityServerEvent {
    SignedBlock(SignedBlock),
    MempoolStatusEvent(MempoolStatusEvent),
}

impl Decoder for DataAvailabilityServerCodec {
    type Item = DataAvailabilityServerRequest;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let decoded_bytes = self.ldc.decode(src)?;

        // try decode ping
        if let Some(decoded_bytes) = decoded_bytes {
            if decoded_bytes == *"ok" {
                return Ok(Some(DataAvailabilityServerRequest::Ping));
            }

            let height: u64 =
                borsh::from_slice(&decoded_bytes)
                    .context(format!(
                        "Decoding height from {} bytes",
                        decoded_bytes.len()
                    ))?
                    .0;

            return Ok(Some(DataAvailabilityServerRequest::BlockHeight(
                BlockHeight(height),
            )));
        }

        Ok(None)
    }
}

impl Encoder<DataAvailabilityServerEvent> for DataAvailabilityServerCodec {
    type Error = anyhow::Error;

    fn encode(
        &mut self,
        event: DataAvailabilityServerEvent,
        dst: &mut bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        let bytes: bytes::Bytes =
            borsh::to_vec(event)?.into();

        self.ldc
            .encode(bytes, dst)
            .context("Encoding event bytes as length delimited")
    }
}

// Client Side

#[derive(Default)]
pub struct DataAvailabilityClientCodec {
    ldc: LengthDelimitedCodec,
}
impl Decoder for DataAvailabilityClientCodec {
    type Item = DataAvailabilityServerEvent;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let decoded_bytes = self.ldc.decode(src)?;
        if let Some(decoded_bytes) = decoded_bytes {
            let event: Self::Item =
                borsh::from_slice(&decoded_bytes)
                    .context(format!(
                        "Decoding DataAvailabilityServerEvent from {} bytes",
                        decoded_bytes.len()
                    ))?
                    .0;

            return Ok(Some(event));
        }
        Ok(None)
    }
}

impl Encoder<DataAvailabilityServerRequest> for DataAvailabilityClientCodec {
    type Error = anyhow::Error;

    fn encode(
        &mut self,
        request: DataAvailabilityServerRequest,
        dst: &mut bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        let bytes: bytes::Bytes = match request {
            DataAvailabilityServerRequest::BlockHeight(height) => {
                borsh::to_vec(height)?.into()
            }
            DataAvailabilityServerRequest::Ping => bytes::Bytes::from("ok"),
        };

        self.ldc
            .encode(bytes, dst)
            .context("Encoding block height bytes as length delimited")
    }
}

#[cfg(test)]
mod test {
    use bytes::BytesMut;
    use tokio_util::codec::{Decoder, Encoder};

    use crate::data_availability::codec::DataAvailabilityServerEvent;
    use crate::model::{AggregateSignature, ConsensusProposal};
    use crate::{
        data_availability::codec::{
            DataAvailabilityClientCodec, DataAvailabilityServerCodec, DataAvailabilityServerRequest,
        },
        model::{BlockHeight, SignedBlock},
    };

    #[tokio::test]
    async fn test_block_streaming() {
        let mut server_codec = DataAvailabilityServerCodec::default();
        let mut client_codec = DataAvailabilityClientCodec::default();
        let mut buffer = BytesMut::new();

        let block = DataAvailabilityServerEvent::SignedBlock(SignedBlock {
            data_proposals: vec![],
            certificate: AggregateSignature::default(),
            consensus_proposal: ConsensusProposal::default(),
        });

        server_codec.encode(block.clone(), &mut buffer).unwrap();

        let decoded_block: DataAvailabilityServerEvent =
            client_codec.decode(&mut buffer).unwrap().unwrap();

        // Vérifiez si le buffer a été correctement consommé
        assert_eq!(block, decoded_block);
    }

    #[tokio::test]
    async fn test_da_request_block_height() {
        let mut server_codec = DataAvailabilityServerCodec::default(); // Votre implémentation du codec
        let mut client_codec = DataAvailabilityClientCodec::default(); // Votre implémentation du codec
        let mut buffer = BytesMut::new();

        let block_height = DataAvailabilityServerRequest::BlockHeight(BlockHeight(1));

        client_codec
            .encode(block_height.clone(), &mut buffer)
            .unwrap();

        let decoded_block_height: DataAvailabilityServerRequest =
            server_codec.decode(&mut buffer).unwrap().unwrap();

        // Vérifiez si le buffer a été correctement consommé
        assert_eq!(block_height, decoded_block_height);
    }

    #[tokio::test]
    async fn test_da_request_ping() {
        let mut server_codec = DataAvailabilityServerCodec::default(); // Votre implémentation du codec
        let mut client_codec = DataAvailabilityClientCodec::default(); // Votre implémentation du codec
        let mut buffer = BytesMut::new();

        let ping = DataAvailabilityServerRequest::Ping;

        client_codec.encode(ping.clone(), &mut buffer).unwrap();

        let decoded_ping: DataAvailabilityServerRequest =
            server_codec.decode(&mut buffer).unwrap().unwrap();

        // Vérifiez si le buffer a été correctement consommé
        assert_eq!(ping, decoded_ping);
    }
}
