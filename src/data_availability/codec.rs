#[cfg(test)]
mod test {
    use bytes::BytesMut;
    use client_sdk::tcp::codec_data_availability;
    use tokio_util::codec::{Decoder, Encoder};

    use crate::model::{AggregateSignature, ConsensusProposal};
    use crate::model::{BlockHeight, SignedBlock};

    #[tokio::test]
    async fn test_block_streaming() {
        let mut server_codec = codec_data_availability::ServerCodec;
        let mut client_codec = codec_data_availability::ClientCodec;
        let mut buffer = BytesMut::new();

        let block = codec_data_availability::DataAvailabilityEvent::SignedBlock(SignedBlock {
            data_proposals: vec![],
            certificate: AggregateSignature::default(),
            consensus_proposal: ConsensusProposal::default(),
        });

        server_codec.encode(block.clone(), &mut buffer).unwrap();

        let decoded_block: codec_data_availability::DataAvailabilityEvent =
            client_codec.decode(&mut buffer).unwrap().unwrap();

        // Vérifiez si le buffer a été correctement consommé
        assert_eq!(block, decoded_block);
    }

    #[tokio::test]
    async fn test_da_request_block_height() {
        let mut server_codec = codec_data_availability::ServerCodec;
        let mut client_codec = codec_data_availability::ClientCodec;
        let mut buffer = BytesMut::new();

        let block_height = codec_data_availability::DataAvailabilityRequest(BlockHeight(1));

        client_codec
            .encode(block_height.clone(), &mut buffer)
            .unwrap();

        let decoded_block_height: codec_data_availability::DataAvailabilityRequest =
            server_codec.decode(&mut buffer).unwrap().unwrap();

        // Vérifiez si le buffer a été correctement consommé
        assert_eq!(block_height, decoded_block_height);
    }
}
