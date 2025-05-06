use borsh::{BorshDeserialize, BorshSerialize};
use sdk::{BlockHeight, MempoolStatusEvent, SignedBlock};

// Da Listener
//
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, PartialEq, Eq)]
pub struct DataAvailabilityRequest(pub BlockHeight);

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum DataAvailabilityEvent {
    SignedBlock(SignedBlock),
    MempoolStatusEvent(MempoolStatusEvent),
}

hyle_net::tcp_client_server! {
    pub DataAvailability,
    request: crate::utils::da_codec::DataAvailabilityRequest,
    response: crate::utils::da_codec::DataAvailabilityEvent
}

#[cfg(test)]
mod test {
    use bytes::BytesMut;
    use tokio_util::codec::{Decoder, Encoder};

    use crate::utils::da_codec::{
        codec_data_availability, DataAvailabilityEvent, DataAvailabilityRequest,
    };
    use sdk::{AggregateSignature, BlockHeight, ConsensusProposal, SignedBlock};

    #[tokio::test]
    async fn test_block_streaming() {
        let mut server_codec = codec_data_availability::ServerCodec;
        let mut client_codec = codec_data_availability::ClientCodec;
        let mut buffer = BytesMut::new();

        let block = DataAvailabilityEvent::SignedBlock(SignedBlock {
            data_proposals: vec![],
            certificate: AggregateSignature::default(),
            consensus_proposal: ConsensusProposal::default(),
        });

        server_codec.encode(block.clone(), &mut buffer).unwrap();

        let decoded_block: DataAvailabilityEvent =
            client_codec.decode(&mut buffer).unwrap().unwrap();

        // Vérifiez si le buffer a été correctement consommé
        assert_eq!(block, decoded_block);
    }

    #[tokio::test]
    async fn test_da_request_block_height() {
        let mut server_codec = codec_data_availability::ServerCodec;
        let mut client_codec = codec_data_availability::ClientCodec;
        let mut buffer = BytesMut::new();

        let block_height = DataAvailabilityRequest(BlockHeight(1));

        client_codec
            .encode(block_height.clone(), &mut buffer)
            .unwrap();

        let decoded_block_height: DataAvailabilityRequest =
            server_codec.decode(&mut buffer).unwrap().unwrap();

        // Vérifiez si le buffer a été correctement consommé
        assert_eq!(block_height, decoded_block_height);
    }
}
