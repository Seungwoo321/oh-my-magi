mod coordinator;
mod digest;
mod error;
mod model;
mod vote;

pub use coordinator::{
    CommandEnvelope, CommandKind, CommandReceipt, Coordinator, DomainEvent, EventKind,
    EventPayload, EventPosition, RunAggregate, RunCheckpoint, RunPersistenceState, RunSnapshot,
};
pub use digest::{Digest, canonical_json};
pub use error::{DomainError, ValidationIssue};
pub use model::*;
pub use vote::{Ballot, Outcome, Tally, VoteCounts, VoteValue};

pub const CONTRACT_SCHEMA_VERSION: u16 = 1;
pub const DELIBERATION_PROTOCOL_VERSION: u16 = 1;
