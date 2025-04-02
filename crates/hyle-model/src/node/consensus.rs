use std::fmt::Display;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use strum_macros::IntoStaticStr;
use utoipa::ToSchema;

use crate::{staking::*, *};

pub type Slot = u64;
pub type View = u64;

#[derive(Clone, Serialize, Deserialize, ToSchema, Debug)]
pub struct ConsensusInfo {
    pub slot: Slot,
    pub view: View,
    pub round_leader: ValidatorPublicKey,
    pub validators: Vec<ValidatorPublicKey>,
}

// -----------------------------
// --- Consensus data model ----
// -----------------------------

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
)]
pub struct ValidatorCandidacy {
    pub pubkey: ValidatorPublicKey,
    pub peer_address: String,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
    PartialEq,
    Eq,
    Ord,
    PartialOrd,
)]
pub struct NewValidatorCandidate {
    pub pubkey: ValidatorPublicKey, // TODO: possible optim: the pubkey is already present in the msg,
    pub msg: SignedByValidator<ConsensusNetMessage>,
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    PartialEq,
    Eq,
    Default,
    Ord,
    PartialOrd,
)]
pub struct QuorumCertificateHash(pub Vec<u8>);

pub type QuorumCertificate = AggregateSignature;

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    PartialEq,
    Eq,
    Ord,
    PartialOrd,
)]
pub struct TimeoutCertificate(ConsensusProposalHash, QuorumCertificate);

// A Ticket is necessary to send a valid prepare
#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
)]
pub enum Ticket {
    // Special value for the initial Cut, needed because we don't have a quorum certificate for the genesis block.
    Genesis,
    CommitQC(QuorumCertificate),
    TimeoutQC(QuorumCertificate, TCKind),
    ForcedCommitQc(QuorumCertificate),
}

#[cfg(feature = "sqlx")]
impl sqlx::Type<sqlx::Postgres> for ConsensusProposalHash {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}

#[cfg(feature = "sqlx")]
impl sqlx::Encode<'_, sqlx::Postgres> for ConsensusProposalHash {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> std::result::Result<
        sqlx::encode::IsNull,
        std::boxed::Box<(dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static)>,
    > {
        <String as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.0, buf)
    }
}

#[cfg(feature = "sqlx")]
impl<'r> sqlx::Decode<'r, sqlx::Postgres> for ConsensusProposalHash {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> std::result::Result<
        ConsensusProposalHash,
        std::boxed::Box<(dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static)>,
    > {
        let inner = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(ConsensusProposalHash(inner))
    }
}

#[derive(
    Debug,
    Clone,
    Default,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
    PartialEq,
    Eq,
    Ord,
    PartialOrd,
)]
pub struct ConsensusProposal {
    pub slot: Slot,
    pub parent_hash: ConsensusProposalHash,
    pub cut: Cut,
    pub staking_actions: Vec<ConsensusStakingAction>,
    pub timestamp: u128,
}

/// This is the hash of the proposal, signed by validators
/// Any consensus-critical data should be hashed here.
impl Hashed<ConsensusProposalHash> for ConsensusProposal {
    fn hashed(&self) -> ConsensusProposalHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.slot.to_le_bytes());
        self.cut.iter().for_each(|(lane_id, hash, _, _)| {
            hasher.update(&lane_id.0 .0);
            hasher.update(hash.0.as_bytes());
        });
        self.staking_actions.iter().for_each(|val| match val {
            ConsensusStakingAction::Bond { candidate } => hasher.update(&candidate.pubkey.0),
            ConsensusStakingAction::PayFeesForDaDi {
                lane_id,
                cumul_size,
            } => {
                hasher.update(&lane_id.0 .0);
                hasher.update(cumul_size.0.to_le_bytes())
            }
        });
        hasher.update(self.timestamp.to_le_bytes());
        hasher.update(self.parent_hash.0.as_bytes());
        ConsensusProposalHash(hex::encode(hasher.finalize()))
    }
}

impl std::hash::Hash for ConsensusProposal {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Hashed::<ConsensusProposalHash>::hashed(self).hash(state);
    }
}

/// Represents the operations that can be performed by the consensus
#[derive(
    BorshSerialize,
    BorshDeserialize,
    Debug,
    Clone,
    Deserialize,
    Serialize,
    PartialEq,
    Eq,
    Ord,
    PartialOrd,
)]
pub enum ConsensusStakingAction {
    /// Bonding a new validator candidate
    Bond {
        // Boxed to reduce size of the enum
        // cf https://rust-lang.github.io/rust-clippy/master/index.html#large_enum_variant
        candidate: Box<NewValidatorCandidate>,
    },

    /// DaDi = Data Dissemination
    PayFeesForDaDi {
        lane_id: LaneId,
        cumul_size: LaneBytesSize,
    },
}

impl From<NewValidatorCandidate> for ConsensusStakingAction {
    fn from(val: NewValidatorCandidate) -> Self {
        ConsensusStakingAction::Bond {
            candidate: Box::new(val),
        }
    }
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    PartialEq,
    Eq,
    IntoStaticStr,
    Hash,
    Ord,
    PartialOrd,
)]
pub enum TCKind {
    NilProposal,
    PrepareQC((QuorumCertificate, ConsensusProposal)),
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    PartialEq,
    Eq,
    IntoStaticStr,
    Hash,
    Ord,
    PartialOrd,
)]
/// There are two possible setups on timeout: either we should timeout with no proposal,
/// or there is a chance one had committed on some validator and we must propose the same.
/// To handle these two cases, we need specific data on top of the regular timeout message.
pub enum TimeoutKind {
    /// Sign a different message to signify 'nil proposal' for aggregation
    NilProposal(SignedByValidator<(Slot, View, ConsensusProposalHash, ())>),
    /// Resending the prepare QC & matching proposal (for convenience for the leader to repropose)
    PrepareQC((QuorumCertificate, ConsensusProposal)),
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    PartialEq,
    Eq,
    IntoStaticStr,
    Hash,
    Ord,
    PartialOrd,
)]
pub enum ConsensusNetMessage {
    Prepare(ConsensusProposal, Ticket, View),
    PrepareVote(ConsensusProposalHash),
    Confirm(QuorumCertificate, ConsensusProposalHash),
    ConfirmAck(ConsensusProposalHash),
    Commit(QuorumCertificate, ConsensusProposalHash),
    // We do signature aggregation, so we would need everyone to send the same PQC.
    // To work around that, send two signatures for now.
    // To successfully send a nil certificate, we need a proof of 2f+1 nil timeouts, so we can't really avoid the double-signature.
    // TODO:but we could avoid signing the top-level message
    Timeout(
        // Here we add the parent hash purely to avoid replay attacks
        // This first message will be used if the TC to generate a proof of timeout
        SignedByValidator<(Slot, View, ConsensusProposalHash)>,
        // The second data is sued to pick a specific TC type
        TimeoutKind,
    ),
    TimeoutCertificate(QuorumCertificate, TCKind, Slot, View),
    ValidatorCandidacy(ValidatorCandidacy),
    SyncRequest(ConsensusProposalHash),
    SyncReply((ValidatorPublicKey, ConsensusProposal, Ticket, View)),
}

impl Hashed<QuorumCertificateHash> for QuorumCertificate {
    fn hashed(&self) -> QuorumCertificateHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.signature.0.clone());
        hasher.update(self.validators.len().to_le_bytes());
        for validator in self.validators.iter() {
            hasher.update(validator.0.clone());
        }
        QuorumCertificateHash(hasher.finalize().as_slice().to_owned())
    }
}

impl Display for ValidatorCandidacy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pubkey: {}", self.pubkey)
    }
}

impl Display for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ticket: {:?}", self)
    }
}

impl Display for ConsensusProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Hash: {}, Parent Hash: {}, Slot: {}, Cut: {:?}, staking_actions: {:?}",
            self.hashed(),
            self.parent_hash,
            self.slot,
            self.cut,
            self.staking_actions,
        )
    }
}

impl Display for ConsensusProposalHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl Display for QuorumCertificateHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "0x{}",
            hex::encode(self.0.get(..HASH_DISPLAY_SIZE).unwrap_or(&self.0))
        )
    }
}

impl Display for ConsensusNetMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let enum_variant: &'static str = self.into();

        match self {
            ConsensusNetMessage::Prepare(cp, ticket, view) => {
                write!(
                    f,
                    "{} CP: {}, ticket: {}, view: {}",
                    enum_variant, cp, ticket, view
                )
            }
            ConsensusNetMessage::PrepareVote(cphash) => {
                write!(f, "{} (CP hash: {})", enum_variant, cphash)
            }
            ConsensusNetMessage::Confirm(cert, cphash) => {
                _ = writeln!(f, "{} (CP hash: {})", enum_variant, cphash);
                _ = write!(f, "Certificate {} with validators ", cert.signature);
                for v in cert.validators.iter() {
                    _ = write!(f, "{},", v);
                }
                write!(f, "")
            }
            ConsensusNetMessage::ConfirmAck(cphash, ..) => {
                write!(f, "{} (CP hash: {})", enum_variant, cphash)
            }
            ConsensusNetMessage::Commit(cert, cphash) => {
                _ = writeln!(f, "{} (CP hash: {})", enum_variant, cphash);
                _ = write!(f, "Certificate {} with validators ", cert.signature);
                for v in cert.validators.iter() {
                    _ = write!(f, "{},", v);
                }
                write!(f, "")
            }

            ConsensusNetMessage::ValidatorCandidacy(candidacy) => {
                write!(f, "{} (Candidacy {})", enum_variant, candidacy)
            }
            ConsensusNetMessage::Timeout(signed_slot_view, tk) => {
                _ = writeln!(
                    f,
                    "{} - Slot: {} View: {}",
                    enum_variant, signed_slot_view.msg.0, signed_slot_view.msg.1
                );
                match tk {
                    TimeoutKind::NilProposal(_) => {
                        _ = writeln!(f, "NilProposal Timeout");
                    }
                    TimeoutKind::PrepareQC((cert, cp)) => {
                        _ = writeln!(f, "PrepareQC certificate {}", cert.signature);
                        _ = write!(f, "CP: {}", cp);
                        for v in cert.validators.iter() {
                            _ = write!(f, "{},", v);
                        }
                    }
                }
                write!(f, "")
            }
            ConsensusNetMessage::TimeoutCertificate(cert, kindcert, slot, view) => {
                _ = writeln!(f, "{} - Slot: {} View: {}", enum_variant, slot, view);
                _ = writeln!(f, "Validators {}", cert.signature);
                for v in cert.validators.iter() {
                    _ = write!(f, "{},", v);
                }
                match kindcert {
                    TCKind::NilProposal => {
                        _ = writeln!(f, "NilProposal certificate");
                    }
                    TCKind::PrepareQC((kindcert, cp)) => {
                        _ = writeln!(
                            f,
                            "PrepareQC certificate {} on CP hash {}",
                            kindcert.signature,
                            cp.hashed()
                        );
                        for v in kindcert.validators.iter() {
                            _ = write!(f, "{},", v);
                        }
                    }
                }
                write!(f, "")
            }
            ConsensusNetMessage::SyncRequest(consensus_proposal_hash) => {
                write!(f, "{} (CP hash: {})", enum_variant, consensus_proposal_hash)
            }
            ConsensusNetMessage::SyncReply((sender, consensus_proposal, ticket, view)) => {
                write!(
                    f,
                    "{} sender: {}, CP: {}, ticket: {}, view: {}",
                    enum_variant, sender, consensus_proposal, ticket, view
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_consensus_proposal_hash() {
        use super::*;
        let proposal = ConsensusProposal {
            slot: 1,
            cut: Cut::default(),
            staking_actions: vec![],
            timestamp: 1,
            parent_hash: ConsensusProposalHash("".to_string()),
        };
        let hash = proposal.hashed();
        assert_eq!(hash.0.len(), 64);
    }
    #[test]
    fn test_consensus_proposal_hash_all_items() {
        use super::*;
        let mut a = ConsensusProposal {
            slot: 1,
            cut: vec![(
                LaneId(ValidatorPublicKey(vec![1])),
                DataProposalHash("propA".to_string()),
                LaneBytesSize(1),
                AggregateSignature::default(),
            )],
            staking_actions: vec![NewValidatorCandidate {
                pubkey: ValidatorPublicKey(vec![1, 2, 3]),
                msg: SignedByValidator {
                    msg: ConsensusNetMessage::PrepareVote(ConsensusProposalHash("".to_string())),
                    signature: ValidatorSignature {
                        signature: Signature(vec![1, 2, 3]),
                        validator: ValidatorPublicKey(vec![1, 2, 3]),
                    },
                },
            }
            .into()],
            timestamp: 1,
            parent_hash: ConsensusProposalHash("parent".to_string()),
        };
        let mut b = ConsensusProposal {
            slot: 1,
            cut: vec![(
                LaneId(ValidatorPublicKey(vec![1])),
                DataProposalHash("propA".to_string()),
                LaneBytesSize(1),
                AggregateSignature {
                    signature: Signature(vec![1, 2, 3]),
                    validators: vec![ValidatorPublicKey(vec![1, 2, 3])],
                },
            )],
            staking_actions: vec![NewValidatorCandidate {
                pubkey: ValidatorPublicKey(vec![1, 2, 3]),
                msg: SignedByValidator {
                    msg: ConsensusNetMessage::PrepareVote(ConsensusProposalHash(
                        "differebt".to_string(),
                    )),
                    signature: ValidatorSignature {
                        signature: Signature(vec![]),
                        validator: ValidatorPublicKey(vec![]),
                    },
                },
            }
            .into()],
            timestamp: 1,
            parent_hash: ConsensusProposalHash("parent".to_string()),
        };
        assert_eq!(a.hashed(), b.hashed());
        a.timestamp = 2;
        assert_ne!(a.hashed(), b.hashed());
        b.timestamp = 2;
        assert_eq!(a.hashed(), b.hashed());

        a.slot = 2;
        assert_ne!(a.hashed(), b.hashed());
        b.slot = 2;
        assert_eq!(a.hashed(), b.hashed());

        a.parent_hash = ConsensusProposalHash("different".to_string());
        assert_ne!(a.hashed(), b.hashed());
        b.parent_hash = ConsensusProposalHash("different".to_string());
        assert_eq!(a.hashed(), b.hashed());
    }
}
