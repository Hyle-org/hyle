use anyhow::Context;
use borsh::{BorshDeserialize, BorshSerialize};
use bytes::BufMut;

use crate::{
    mempool::MempoolStatusEvent,
    model::{BlockHeight, SignedBlock},
    tcp::implem_tcp_codec,
};

// Server Side
#[derive(Debug, Default)]
pub struct DataAvailabilityServerCodec;

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, PartialEq, Eq)]
pub struct DataAvailabilityServerRequest(pub BlockHeight);

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum DataAvailabilityServerEvent {
    SignedBlock(SignedBlock),
    MempoolStatusEvent(MempoolStatusEvent),
}

implem_tcp_codec! {
    DataAvailabilityServerCodec,
    decode: DataAvailabilityServerRequest,
    encode: DataAvailabilityServerEvent
}

// Client Side

#[derive(Default)]
pub struct DataAvailabilityClientCodec;

implem_tcp_codec! {
    DataAvailabilityClientCodec,
    decode: DataAvailabilityServerEvent,
    encode: DataAvailabilityServerRequest
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

        let block_height = DataAvailabilityServerRequest(BlockHeight(1));

        client_codec
            .encode(block_height.clone(), &mut buffer)
            .unwrap();

        let decoded_block_height: DataAvailabilityServerRequest =
            server_codec.decode(&mut buffer).unwrap().unwrap();

        // Vérifiez si le buffer a été correctement consommé
        assert_eq!(block_height, decoded_block_height);
    }
}
