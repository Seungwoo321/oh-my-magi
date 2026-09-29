mod capture;
mod manifest;

pub use capture::{
    CandidateSummary, CaptureBatch, CaptureDirective, CaptureError, CaptureLimits, CaptureProblem,
    CaptureReport, CapturedObject, ContentRepresentation, EnumerationReport, LineRange,
    MAX_FILE_BYTES, MAX_MANIFEST_BYTES, MAX_MANIFEST_ITEMS, SourceGrant, classify_selected_path,
};
pub use manifest::{
    DisclosureContentKind, DisclosureError, DisclosureGrant, DisclosureRequest, DisclosureState,
    DispatchDisclosure, EvidenceLocator, FreshnessObservation, FreshnessStatus, ManifestSource,
    ManifestSourceState, Recipient, RepresentationKind, SafeSourceSummary, SecretPatternFinding,
    SecretPatternKind, SourceCaptureManifest, SourceCaptureManifestContent, SourceOmission,
    SourceOmissionCode, freshness_from_digests,
};

pub use magi_domain::Digest;
