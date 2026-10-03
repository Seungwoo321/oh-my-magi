use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod resource_custody;
mod sandbox;
pub mod subscription;
mod transport;
mod verification;

pub use verification::{
    RequestSettlement, RetiredGenerationCapture, RuntimeVerificationService, VerificationMetrics,
    VerificationRequest, VerifiedInstalledGeneration, VerifiedReadOnlyInstallation,
    VerifiedRuntimeArtifact, verify_codesign_identity, verify_installed_generation,
    verify_readonly_installation,
};

pub struct StreamOperationLease {
    _activity: verification::ActivityLease,
    request: VerificationRequest,
}

impl StreamOperationLease {
    /// Keeps unknown external cleanup visible even after the local owner is dropped.
    pub fn retain_unresolved(mut self) {
        self._activity.mark_owned_process();
    }
}
impl VerificationRequest {
    /// Linearizes a synchronous owned effect with revocation under the bound lease.
    pub fn with_stream_effect_admission<T>(
        &self,
        lease: &StreamOperationLease,
        operation: impl FnOnce() -> T,
    ) -> Result<T, ProviderError> {
        if !self.same_root(&lease.request) {
            return Err(ProviderError::InvalidLaunch);
        }
        self.publish(operation)
    }

    /// Yields settlement observation on the async timer, bounded by its absolute deadline.
    pub async fn wait_for_stream_checkpoint(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), ProviderError> {
        if std::time::Instant::now() >= deadline {
            return Err(ProviderError::Timeout);
        }
        let next = (std::time::Instant::now() + std::time::Duration::from_millis(1)).min(deadline);
        tokio::time::sleep_until(next.into()).await;
        if std::time::Instant::now() >= deadline {
            Err(ProviderError::Timeout)
        } else {
            Ok(())
        }
    }

    pub fn track_stream_operation(&self) -> Result<StreamOperationLease, ProviderError> {
        Ok(StreamOperationLease {
            _activity: self.provider_activity()?,
            request: self.clone(),
        })
    }
}

#[cfg(test)]
mod native_fixture_tests;

#[cfg(all(test, target_os = "macos"))]
mod signed_security_tests;

pub use sandbox::{
    CodexAcpLaunch, RuntimeArtifactIdentity, codex_acp_adapter_patch_id, verify_packaged_artifact,
    verify_packaged_artifact_identity,
};
pub use transport::{
    AdapterInfo, AuthMethod, AuthProgressStage, AuthenticationStatus, AvailableMode,
    AvailableModel, CancelOutcome, ClientFileReadError, ClientFileReader, CodexAcpClient,
    HomeBindingProof, InitializeResult, NoticeCategory, NoticeSeverity, PendingCancel,
    PendingModelConfiguration, PendingSession, PreparedSession, PromptEvent, PromptHandle,
    PromptResult, PromptStreamDiagnostic, PromptStreamFailure, PromptStreamSnapshot,
    ProviderNoticeDiagnostic, ProviderUsage, RpcFailureCategory, RpcFailureDiagnostic,
    RpcFailureMethod, SessionInfo,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRemediation {
    Reauthenticate,
    RefreshCatalog,
    ReviewRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxPolicyFailureCode {
    NetworkRuleRejected,
    ProfileRejected,
    ProbeTimedOut,
}

impl std::fmt::Display for ProviderRemediation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Reauthenticate => "reauthenticate",
            Self::RefreshCatalog => "refresh_catalog",
            Self::ReviewRequest => "review_request",
        })
    }
}

#[derive(Debug, Error, Clone)]
pub enum ProviderError {
    #[error("provider execution is supported only on macOS")]
    UnsupportedPlatform,
    #[error("the provider launch configuration is invalid")]
    InvalidLaunch,
    #[error("the bundled provider artifact failed verification")]
    ArtifactVerification,
    #[error("the isolated profile home is unavailable or unsafe")]
    ProfileHomeUnavailable,
    #[error("the per-core work directory is unavailable, unsafe, or not empty")]
    RoleWorkdirUnavailable,
    #[error("the operating system isolation boundary could not be established")]
    IsolationUnavailable,
    #[error("the operating system isolation policy was rejected")]
    SandboxPolicyRejected {
        code: SandboxPolicyFailureCode,
        exit_status: Option<i32>,
    },
    #[error("the provider process could not be started")]
    ProcessStart,
    #[error("the provider process is closed")]
    ProcessClosed,
    #[error("the provider process exited unexpectedly")]
    ProcessExited,
    #[error("the Codex App Server exited during sign-in")]
    CodexAppServerExited { exit_code: Option<i32> },
    #[error("Codex authentication status could not be read")]
    AuthenticationStatusRpcFailed,
    #[error("Codex sign-in could not be started")]
    AuthenticationRpcFailed,
    #[error("macOS could not open the Codex sign-in browser")]
    BrowserOpenFailed,
    #[error("the ACP protocol exchange failed")]
    Protocol,
    #[error("Codex RPC failed; remediation category: {remediation}")]
    RemoteRequestFailed { remediation: ProviderRemediation },
    #[error("Codex RPC timed out; remediation category: {remediation}")]
    RpcTimeout { remediation: ProviderRemediation },
    #[error("the provider endpoint proxy could not be started or completed its request")]
    ProxyUnavailable,
    #[error("the provider response did not match the expected protocol shape")]
    InvalidResponse,
    #[error("the selected authentication method is not supported")]
    AuthenticationUnavailable,
    #[error("the provider profile is not authenticated")]
    Unauthenticated,
    #[error("this core already has an ACP session")]
    SessionAlreadyCreated,
    #[error("a provider model catalog request is already in progress")]
    CatalogBusy,
    #[error("the selected model is not available from the provider")]
    ModelUnavailable,
    #[error("the ACP session does not exist")]
    SessionUnavailable,
    #[error("the durable local source permission store is unavailable")]
    SourceReadUnavailable,
    #[error("this ACP session already has a prompt in progress")]
    PromptInProgress,
    #[error("the provider attempted to use a denied tool")]
    ToolDenied,
    #[error("the provider response exceeded the configured output limit")]
    OutputLimit,
    #[error("the prompt exceeds the provider transport input limit")]
    InputLimit,
    #[error("the provider output stream exceeded its event limit")]
    EventLimit,
    #[error("the provider notice buffer exceeded its configured limit")]
    NoticeLimit,
    #[error("the provider output consumer closed before delivery completed")]
    StreamConsumerClosed,
    #[error("the provider request timed out")]
    Timeout,
    #[error("the provider request was cancelled")]
    Cancelled,
    #[error("prompt failed after receiving buffered provider updates: {source}")]
    PromptFailedWithEvents {
        #[source]
        source: Box<ProviderError>,
        buffered_events: Vec<transport::PromptEvent>,
    },
}

impl ProviderError {
    pub fn remediation_category(&self) -> Option<ProviderRemediation> {
        match self {
            Self::PromptFailedWithEvents { source, .. } => source.remediation_category(),
            Self::RemoteRequestFailed { remediation } => Some(*remediation),
            Self::RpcTimeout { remediation } => Some(*remediation),
            Self::Unauthenticated | Self::AuthenticationUnavailable => {
                Some(ProviderRemediation::Reauthenticate)
            }
            Self::CatalogBusy | Self::ModelUnavailable => Some(ProviderRemediation::RefreshCatalog),
            Self::Timeout | Self::ProcessClosed | Self::ProcessExited => {
                Some(ProviderRemediation::ReviewRequest)
            }
            Self::CodexAppServerExited { .. } => Some(ProviderRemediation::Reauthenticate),
            Self::AuthenticationRpcFailed | Self::BrowserOpenFailed => {
                Some(ProviderRemediation::Reauthenticate)
            }
            _ => None,
        }
    }
}

pub const CODEX_ACP_PACKAGE: &str = "@agentclientprotocol/codex-acp";
pub const CODEX_ACP_VERSION: &str = "1.13.1";
pub const CODEX_ACP_GIT_TAG: &str = "v1.13.1";
pub const CODEX_ACP_GIT_COMMIT: &str = "b1b8490cd165c18626dc3fe83836cdacdef94cd3";
pub const CODEX_ACP_NPM_INTEGRITY: &str = "sha512-NAbXTb6GRReox7B+8RN9VRB+sKUqFJh5Vg7Ex7RskYCO8EsYPJKN1WJvN7mOqjCKKK9lVDrNxtP7Bp/OUKPyAg==";
pub const CODEX_BINARY_PACKAGE: &str = "@openai/codex";
pub const CODEX_BINARY_VERSION: &str = "0.156.1";
pub const CODEX_BINARY_NPM_INTEGRITY: &str = "sha512-nI1iVl/n2SO2lSvlwEsJx63zdSI4C4Me2gR7AG0OWMJiGSakz2tY2hx43E39Zq5aEoeB5bZjJXzp5Sqhog6vyA==";
pub const CODEX_HOME_OVERRIDE_ENV: &str = "CODEX_HOME";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterState {
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionState {
    Unproven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionCheckId {
    InstalledArtifactPin,
    PlatformAndArchitecture,
    ProfileHomeBinding,
    DedicatedAuthBoundary,
    NoPastedCredentialInput,
    GlobalProfileAndMemoryIsolation,
    NativeToolIsolation,
    FilesystemScopeIsolation,
    NetworkIsolation,
    ExtensionAndHookIsolation,
    SessionAndProcessIsolation,
    CancellationAndFencing,
    ContextWindowAndUsageLimits,
    TermsAndAccountEligibility,
    RequestClientImplementation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmissionCheck {
    pub id: AdmissionCheckId,
    pub state: AdmissionState,
    pub requirement: &'static str,
    pub evidence_needed: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodexAcpProfile {
    pub provider_id: &'static str,
    pub package: &'static str,
    pub package_version: &'static str,
    pub source_tag: &'static str,
    pub source_commit: &'static str,
    pub package_integrity: &'static str,
    pub bundled_codex_package: &'static str,
    pub bundled_codex_version: &'static str,
    pub bundled_codex_integrity: &'static str,
    pub home_override_env: &'static str,
    pub adapter_reports_effective_home: bool,
}

pub const PINNED_CODEX_ACP: CodexAcpProfile = CodexAcpProfile {
    provider_id: "codex-acp",
    package: CODEX_ACP_PACKAGE,
    package_version: CODEX_ACP_VERSION,
    source_tag: CODEX_ACP_GIT_TAG,
    source_commit: CODEX_ACP_GIT_COMMIT,
    package_integrity: CODEX_ACP_NPM_INTEGRITY,
    bundled_codex_package: CODEX_BINARY_PACKAGE,
    bundled_codex_version: CODEX_BINARY_VERSION,
    bundled_codex_integrity: CODEX_BINARY_NPM_INTEGRITY,
    home_override_env: CODEX_HOME_OVERRIDE_ENV,
    adapter_reports_effective_home: false,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmissionReport {
    pub provider: CodexAcpProfile,
    pub state: AdapterState,
    pub checks: Vec<AdmissionCheck>,
}

impl AdmissionReport {
    /// This static report does not infer runtime admission from adapter availability.
    pub fn current() -> Self {
        Self {
            provider: PINNED_CODEX_ACP,
            state: AdapterState::Unavailable,
            checks: vec![
                check(
                    AdmissionCheckId::InstalledArtifactPin,
                    "The installed ACP executable and bundled Codex binary match the exact pinned versions and verified digests.",
                    "A host-side artifact verifier attestation tied to the actual executable bytes.",
                ),
                check(
                    AdmissionCheckId::PlatformAndArchitecture,
                    "The artifact is built and verified for a supported macOS version and architecture.",
                    "A runtime OS/architecture check matched against the signed build manifest.",
                ),
                check(
                    AdmissionCheckId::ProfileHomeBinding,
                    "Each ProviderProfile launches Codex ACP with its own explicit CODEX_HOME and a per-core OS boundary that denies other profiles and the default home.",
                    "The host-bound environment and Seatbelt policy are inspectable, but pinned codex-acp 1.13.1 omits app-server codexHome from the ACP initialize result; direct adapter attestation remains unavailable.",
                ),
                check(
                    AdmissionCheckId::DedicatedAuthBoundary,
                    "Codex authentication stays in the selected isolated provider profile; app code never reads or copies a global profile.",
                    "A native auth handoff and profile-specific home verified without exposing credential bytes or the absolute home path to the app UI, logs, or exports.",
                ),
                check(
                    AdmissionCheckId::NoPastedCredentialInput,
                    "The app accepts no pasted OAuth token, API key, or credential-bearing environment value.",
                    "A command-boundary audit and an allowlisted provider environment.",
                ),
                check(
                    AdmissionCheckId::GlobalProfileAndMemoryIsolation,
                    "Global instructions, history, memory, config, and provider state cannot enter the isolated run.",
                    "An OS-enforced clean home/config root and verification that no global profile is inherited.",
                ),
                check(
                    AdmissionCheckId::NativeToolIsolation,
                    "Shell, terminal, write, patch, and other mutating native tools are denied by the OS boundary.",
                    "A negative capability probe against the actual isolated process; ACP mode labels alone do not qualify.",
                ),
                check(
                    AdmissionCheckId::FilesystemScopeIsolation,
                    "The provider can read only the exact user-approved captured objects and cannot access the rest of the machine.",
                    "An OS sandbox profile and negative read probes outside the approved object bridge.",
                ),
                check(
                    AdmissionCheckId::NetworkIsolation,
                    "The process has only the network access required by its declared provider endpoint.",
                    "An OS-enforced egress policy and a verified endpoint allowlist.",
                ),
                check(
                    AdmissionCheckId::ExtensionAndHookIsolation,
                    "MCP servers, plugins, hooks, custom instructions, and extensions are disabled or separately proven safe.",
                    "A clean isolated config plus inspection of the effective provider configuration.",
                ),
                check(
                    AdmissionCheckId::SessionAndProcessIsolation,
                    "Each core has a separate, fenced ACP session and cannot reuse another run's state.",
                    "Run-bound process/session IDs and host-side fencing that rejects stale output.",
                ),
                check(
                    AdmissionCheckId::CancellationAndFencing,
                    "Cancellation terminates or fences the provider process before its output can affect a run.",
                    "A process supervisor with verified kill, timeout, and stale-event rejection behavior.",
                ),
                check(
                    AdmissionCheckId::ContextWindowAndUsageLimits,
                    "The effective model context and provider-reported usage limits are known and cannot silently exceed user consent.",
                    "Model/context discovery and authoritative usage/reset observations from the provider.",
                ),
                check(
                    AdmissionCheckId::TermsAndAccountEligibility,
                    "The selected account and subscription may be used through this product integration.",
                    "An explicit policy review for the provider, account type, and distribution model.",
                ),
                check(
                    AdmissionCheckId::RequestClientImplementation,
                    "The host has a request path that checks the disclosure grant and admission proof before every dispatch.",
                    "A reviewed host integration with no alternate process or network path.",
                ),
            ],
        }
    }

    pub fn is_available(&self) -> bool {
        false
    }
}

fn check(
    id: AdmissionCheckId,
    requirement: &'static str,
    evidence_needed: &'static str,
) -> AdmissionCheck {
    AdmissionCheck {
        id,
        state: AdmissionState::Unproven,
        requirement,
        evidence_needed,
    }
}
