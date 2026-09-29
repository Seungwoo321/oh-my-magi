use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub path: String,
    pub code: &'static str,
    pub message: String,
}

impl ValidationIssue {
    pub fn new(path: impl Into<String>, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    #[error("validation failed: {0:?}")]
    Validation(Vec<ValidationIssue>),
    #[error("invalid state transition from {from} to {to}")]
    InvalidTransition { from: String, to: String },
    #[error("revision conflict: expected {expected}, current {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("idempotency key was already used with a different payload")]
    IdempotencyConflict,
    #[error("a result already exists for {0}")]
    DuplicateResult(String),
    #[error("the requested operation requires {required}; found {actual}")]
    Precondition { required: String, actual: String },
    #[error("proposal is already frozen for this run")]
    ProposalAlreadyFrozen,
    #[error("no proposal is frozen for this run")]
    ProposalMissing,
    #[error("a terminal run cannot be changed")]
    TerminalRun,
    #[error("serialization failed: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for DomainError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value.to_string())
    }
}
