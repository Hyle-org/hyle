use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use sparse_merkle_tree::{merge::MergeValue, traits::Hasher, MerkleProof, H256};

// FIXME: Find a better way to make MerkleProof Borshable
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorshableMerkleProof(pub MerkleProof);

impl From<BorshableMerkleProof> for MerkleProof {
    fn from(proof: BorshableMerkleProof) -> Self {
        proof.0
    }
}

impl BorshSerialize for BorshableMerkleProof {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        (self.0.leaves_count() as u32).serialize(writer)?;
        for leaf in self.0.leaves_bitmap() {
            writer.write_all(leaf.as_slice())?;
        }
        (self.0.merkle_path().len() as u32).serialize(writer)?;
        for item in self.0.merkle_path() {
            match item {
                MergeValue::Value(h) => {
                    0u8.serialize(writer)?;
                    writer.write_all(h.as_slice())?;
                }
                MergeValue::MergeWithZero {
                    base_node,
                    zero_bits,
                    zero_count,
                } => {
                    1u8.serialize(writer)?;
                    writer.write_all(base_node.as_slice())?;
                    writer.write_all(zero_bits.as_slice())?;
                    zero_count.serialize(writer)?;
                }
            }
        }
        Ok(())
    }
}

impl BorshDeserialize for BorshableMerkleProof {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let leaves_count = u32::deserialize_reader(reader)?;
        let mut leaves_bitmap = Vec::with_capacity(leaves_count as usize);
        for _ in 0..leaves_count {
            let mut leaf_buf = [0u8; 32];
            reader.read_exact(&mut leaf_buf)?;
            leaves_bitmap.push(H256::from(leaf_buf));
        }

        let path_len = u32::deserialize_reader(reader)?;
        let mut merkle_path = Vec::with_capacity(path_len as usize);
        for _ in 0..path_len {
            let marker = u8::deserialize_reader(reader)?;
            match marker {
                0 => {
                    let mut h_buf = [0u8; 32];
                    reader.read_exact(&mut h_buf)?;
                    merkle_path.push(MergeValue::Value(H256::from(h_buf)));
                }
                1 => {
                    let mut base_node_buf = [0u8; 32];
                    reader.read_exact(&mut base_node_buf)?;

                    let mut zero_bits_buf = [0u8; 32];
                    reader.read_exact(&mut zero_bits_buf)?;

                    let zero_count = u8::deserialize_reader(reader)?;
                    merkle_path.push(MergeValue::MergeWithZero {
                        base_node: H256::from(base_node_buf),
                        zero_bits: H256::from(zero_bits_buf),
                        zero_count,
                    });
                }
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Invalid marker for MergeValue",
                    ));
                }
            }
        }

        Ok(Self(MerkleProof::new(leaves_bitmap, merkle_path)))
    }
}

// Custom SHA256Hasher implementation
#[derive(Default, Debug)]
pub struct SHA256Hasher(Sha256);

impl Hasher for SHA256Hasher {
    fn write_h256(&mut self, h: &H256) {
        self.0.update(h.as_slice());
    }

    fn write_byte(&mut self, b: u8) {
        self.0.update([b]);
    }

    fn finish(self) -> H256 {
        let result = self.0.finalize();
        let mut h = [0u8; 32];
        h.copy_from_slice(&result);
        H256::from(h)
    }
}
