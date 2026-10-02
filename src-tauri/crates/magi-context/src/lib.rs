mod capture;
mod extraction;
mod freshness;
mod pdf_selection;
pub use extraction::{
    ExtractedPage, ExtractionCompletionProof, ExtractionHelperAuthority, ExtractionInvocation,
    ExtractionOperationLease, NativeExtraction, close_extraction_helper_authority,
    configure_extraction_helper_authority,
};
pub use freshness::{FreshnessGrant, FreshnessRead};
pub use pdf_selection::{captured_pdf_page_count, extract_pdf_page_selection};
mod manifest;

pub use capture::{
    CandidateSummary, CaptureBatch, CaptureDirective, CaptureError, CaptureLimits, CaptureProblem,
    CaptureReport, CapturedObject, ContentRepresentation, EnumerationReport, LineRange,
    MAX_FILE_BYTES, MAX_MANIFEST_BYTES, MAX_MANIFEST_ITEMS, SourceGrant, SourceScopeRoot,
    classify_selected_path, contains_sensitive_content,
};
pub use manifest::{
    DisclosureContentKind, DisclosureError, DisclosureGrant, DisclosureRequest, DisclosureState,
    DispatchDisclosure, EvidenceLocator, FreshnessObservation, FreshnessStatus, ManifestSource,
    ManifestSourceState, Recipient, RepresentationKind, SafeSourceSummary, SecretPatternFinding,
    SecretPatternKind, SourceCaptureManifest, SourceCaptureManifestContent, SourceOmission,
    SourceOmissionCode, freshness_from_digests,
};

pub use magi_domain::Digest;
