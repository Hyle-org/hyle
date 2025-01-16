use bincode::Decode;

use hyle_model::{Blob, BlobIndex, StructuredBlob};

pub fn parse_blob<Parameters>(blobs: &[Blob], index: &BlobIndex) -> Parameters
where
    Parameters: Decode,
{
    let blob = match blobs.get(index.0) {
        Some(v) => v,
        None => {
            panic!("unable to find the payload");
        }
    };

    let (parameters, _) =
        bincode::decode_from_slice(blob.data.0.as_slice(), bincode::config::standard())
            .expect("Failed to decode payload");
    parameters
}

pub fn parse_structured_blob<Parameters>(
    blobs: &[Blob],
    index: &BlobIndex,
) -> StructuredBlob<Parameters>
where
    Parameters: Decode,
{
    let blob = match blobs.get(index.0) {
        Some(v) => v,
        None => {
            panic!("unable to find the payload");
        }
    };

    let parsed_blob: StructuredBlob<Parameters> = StructuredBlob::try_from(blob.clone())
        .unwrap_or_else(|e| {
            panic!("Failed to decode blob: {:?}", e);
        });
    parsed_blob
}
