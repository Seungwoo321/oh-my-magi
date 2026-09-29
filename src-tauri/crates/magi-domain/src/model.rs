use serde::{Deserialize, Serialize};

use crate::{
    CONTRACT_SCHEMA_VERSION, DELIBERATION_PROTOCOL_VERSION, Digest, DomainError, ValidationIssue,
    canonical_json,
};

pub type OpaqueId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum CoreId {
    #[serde(rename = "MELCHIOR-1")]
    Melchior1,
    #[serde(rename = "BALTHASAR-2")]
    Balthasar2,
    #[serde(rename = "CASPER-3")]
    Casper3,
}

impl CoreId {
    pub const ALL: [Self; 3] = [Self::Melchior1, Self::Balthasar2, Self::Casper3];

    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Melchior1 => "MELCHIOR-1",
            Self::Balthasar2 => "BALTHASAR-2",
            Self::Casper3 => "CASPER-3",
        }
    }
}

impl std::fmt::Display for CoreId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire_name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Answer,
    Recommendation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionSnapshot {
    pub schema_version: u16,
    pub question_id: OpaqueId,
    pub kind: QuestionKind,
    pub prompt: String,
    #[serde(default)]
    pub conditions: Vec<String>,
    #[serde(default)]
    pub evaluation_criteria: Vec<String>,
    #[serde(default)]
    pub selected_prior_dossier_ids: Vec<OpaqueId>,
    pub digest: Digest,
}

#[derive(Serialize)]
struct QuestionDigestDocument<'a> {
    schema_version: u16,
    question_id: &'a str,
    kind: QuestionKind,
    prompt: &'a str,
    conditions: &'a [String],
    evaluation_criteria: &'a [String],
    selected_prior_dossier_ids: &'a [OpaqueId],
}

impl QuestionSnapshot {
    pub fn new(
        question_id: OpaqueId,
        kind: QuestionKind,
        prompt: String,
        conditions: Vec<String>,
        evaluation_criteria: Vec<String>,
        selected_prior_dossier_ids: Vec<OpaqueId>,
    ) -> Result<Self, DomainError> {
        let mut snapshot = Self {
            schema_version: CONTRACT_SCHEMA_VERSION,
            question_id,
            kind,
            prompt,
            conditions,
            evaluation_criteria,
            selected_prior_dossier_ids,
            digest: Digest::from_bytes(b""),
        };
        snapshot.digest = snapshot.calculate_digest()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = QuestionDigestDocument {
            schema_version: self.schema_version,
            question_id: &self.question_id,
            kind: self.kind,
            prompt: &self.prompt,
            conditions: &self.conditions,
            evaluation_criteria: &self.evaluation_criteria,
            selected_prior_dossier_ids: &self.selected_prior_dossier_ids,
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        check_id("question_id", &self.question_id, &mut issues);
        check_nonblank("prompt", &self.prompt, 32_000, &mut issues);
        check_strings("conditions", &self.conditions, 256, 8_000, &mut issues);
        check_strings(
            "evaluation_criteria",
            &self.evaluation_criteria,
            128,
            8_000,
            &mut issues,
        );
        check_ids(
            "selected_prior_dossier_ids",
            &self.selected_prior_dossier_ids,
            &mut issues,
        );
        if self.schema_version != CONTRACT_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                format!("expected {CONTRACT_SCHEMA_VERSION}"),
            ));
        }
        if !self.digest.is_valid() || self.calculate_digest()? != self.digest {
            issues.push(ValidationIssue::new(
                "digest",
                "digest_mismatch",
                "question snapshot digest does not match its canonical content",
            ));
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelBindingSnapshot {
    pub provider_profile_id: OpaqueId,
    pub revision: u64,
    pub adapter_id: String,
    pub adapter_version: String,
    pub adapter_digest: Digest,
    pub model_id: String,
    pub context_window_tokens: Option<u32>,
    pub maximum_output_tokens: Option<u32>,
}

impl ModelBindingSnapshot {
    pub fn validate(&self, path: &str) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        check_id(
            &format!("{path}.provider_profile_id"),
            &self.provider_profile_id,
            &mut issues,
        );
        check_nonblank(
            &format!("{path}.adapter_id"),
            &self.adapter_id,
            256,
            &mut issues,
        );
        check_nonblank(
            &format!("{path}.adapter_version"),
            &self.adapter_version,
            128,
            &mut issues,
        );
        check_nonblank(
            &format!("{path}.model_id"),
            &self.model_id,
            512,
            &mut issues,
        );
        if !self.adapter_digest.is_valid() {
            issues.push(ValidationIssue::new(
                format!("{path}.adapter_digest"),
                "digest_format",
                "adapter digest must be a SHA-256 digest",
            ));
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthenticationMethod {
    LocalSubscription,
    ByokApi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfileInput {
    pub provider_profile_id: OpaqueId,
    pub provider_id: String,
    pub display_name: String,
    pub account_alias: String,
    pub authentication_method: ProviderAuthenticationMethod,
    pub secret_reference: Option<String>,
    pub runtime_home_id: OpaqueId,
}

impl ProviderProfileInput {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_provider_profile_fields(
            &self.provider_profile_id,
            &self.provider_id,
            &self.display_name,
            &self.account_alias,
            self.authentication_method,
            self.secret_reference.as_deref(),
            &self.runtime_home_id,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfileRevision {
    pub schema_version: u16,
    pub provider_profile_id: OpaqueId,
    pub revision: u64,
    pub provider_id: String,
    pub display_name: String,
    pub account_alias: String,
    pub authentication_method: ProviderAuthenticationMethod,
    pub secret_reference: Option<String>,
    pub runtime_home_id: OpaqueId,
    pub digest: Digest,
}

#[derive(Serialize)]
struct ProviderProfileDigestDocument<'a> {
    schema_version: u16,
    provider_profile_id: &'a str,
    revision: u64,
    provider_id: &'a str,
    display_name: &'a str,
    account_alias: &'a str,
    authentication_method: ProviderAuthenticationMethod,
    secret_reference: Option<&'a str>,
    runtime_home_id: &'a str,
}

impl ProviderProfileRevision {
    pub fn new(input: ProviderProfileInput, revision: u64) -> Result<Self, DomainError> {
        let mut profile = Self {
            schema_version: CONTRACT_SCHEMA_VERSION,
            provider_profile_id: input.provider_profile_id,
            revision,
            provider_id: input.provider_id,
            display_name: input.display_name,
            account_alias: input.account_alias,
            authentication_method: input.authentication_method,
            secret_reference: input.secret_reference,
            runtime_home_id: input.runtime_home_id,
            digest: Digest::from_bytes(b""),
        };
        profile.digest = profile.calculate_digest()?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = ProviderProfileDigestDocument {
            schema_version: self.schema_version,
            provider_profile_id: &self.provider_profile_id,
            revision: self.revision,
            provider_id: &self.provider_id,
            display_name: &self.display_name,
            account_alias: &self.account_alias,
            authentication_method: self.authentication_method,
            secret_reference: self.secret_reference.as_deref(),
            runtime_home_id: &self.runtime_home_id,
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        if self.schema_version != CONTRACT_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                format!("expected {CONTRACT_SCHEMA_VERSION}"),
            ));
        }
        if let Err(DomainError::Validation(mut nested)) = validate_provider_profile_fields(
            &self.provider_profile_id,
            &self.provider_id,
            &self.display_name,
            &self.account_alias,
            self.authentication_method,
            self.secret_reference.as_deref(),
            &self.runtime_home_id,
        ) {
            issues.append(&mut nested);
        }
        if !self.digest.is_valid() || self.calculate_digest()? != self.digest {
            issues.push(ValidationIssue::new(
                "digest",
                "digest_mismatch",
                "provider profile digest does not match its canonical content",
            ));
        }
        finish(issues)
    }
}

fn validate_provider_profile_fields(
    provider_profile_id: &str,
    provider_id: &str,
    display_name: &str,
    account_alias: &str,
    authentication_method: ProviderAuthenticationMethod,
    secret_reference: Option<&str>,
    runtime_home_id: &str,
) -> Result<(), DomainError> {
    let mut issues = Vec::new();
    check_id("provider_profile_id", provider_profile_id, &mut issues);
    check_safe_component("provider_id", provider_id, 128, &mut issues);
    check_nonblank("display_name", display_name, 128, &mut issues);
    check_nonblank("account_alias", account_alias, 128, &mut issues);
    check_safe_component("runtime_home_id", runtime_home_id, 128, &mut issues);
    if let Some(reference) = secret_reference {
        match reference.strip_prefix("keychain-item:") {
            Some(item_id) => check_safe_component("secret_reference", item_id, 128, &mut issues),
            None => issues.push(ValidationIssue::new(
                "secret_reference",
                "invalid_secret_reference",
                "secret references must be opaque Keychain item identifiers",
            )),
        }
    }
    match (authentication_method, secret_reference) {
        (ProviderAuthenticationMethod::LocalSubscription, Some(_)) => {
            issues.push(ValidationIssue::new(
                "secret_reference",
                "unexpected_secret_reference",
                "local subscription credentials remain in the provider runtime",
            ));
        }
        (ProviderAuthenticationMethod::ByokApi, None) => {
            issues.push(ValidationIssue::new(
                "secret_reference",
                "missing_secret_reference",
                "BYOK profiles require a Keychain item reference",
            ));
        }
        _ => {}
    }
    finish(issues)
}

fn check_safe_component(
    path: &str,
    value: &str,
    maximum: usize,
    issues: &mut Vec<ValidationIssue>,
) {
    if value.is_empty()
        || value.len() > maximum
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        issues.push(ValidationIssue::new(
            path,
            "invalid_identifier_component",
            format!("value must contain only ASCII letters, digits, hyphens, or underscores and be at most {maximum} bytes"),
        ));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreRoleDefinition {
    pub core_id: CoreId,
    pub profile_id: OpaqueId,
    pub display_name: String,
    pub review_purpose: String,
    pub evaluation_criteria: Vec<String>,
    pub falsification_questions: Vec<String>,
    pub response_language: String,
}

impl CoreRoleDefinition {
    pub fn validate(&self) -> Result<(), DomainError> {
        let path = format!("roles.{}", self.core_id.wire_name());
        let mut issues = Vec::new();
        check_id(&format!("{path}.profile_id"), &self.profile_id, &mut issues);
        check_nonblank(
            &format!("{path}.display_name"),
            &self.display_name,
            128,
            &mut issues,
        );
        check_nonblank(
            &format!("{path}.review_purpose"),
            &self.review_purpose,
            8_000,
            &mut issues,
        );
        check_strings(
            &format!("{path}.evaluation_criteria"),
            &self.evaluation_criteria,
            64,
            2_000,
            &mut issues,
        );
        check_strings(
            &format!("{path}.falsification_questions"),
            &self.falsification_questions,
            64,
            2_000,
            &mut issues,
        );
        check_nonblank(
            &format!("{path}.response_language"),
            &self.response_language,
            64,
            &mut issues,
        );
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolePresetRevision {
    pub schema_version: u16,
    pub preset_id: OpaqueId,
    pub revision: u64,
    pub display_name: String,
    pub roles: [CoreRoleDefinition; 3],
    pub digest: Digest,
}

#[derive(Serialize)]
struct RolePresetDigestDocument<'a> {
    schema_version: u16,
    preset_id: &'a str,
    revision: u64,
    display_name: &'a str,
    roles: &'a [CoreRoleDefinition; 3],
}

impl RolePresetRevision {
    pub fn new(
        preset_id: OpaqueId,
        revision: u64,
        display_name: String,
        roles: [CoreRoleDefinition; 3],
    ) -> Result<Self, DomainError> {
        let mut preset = Self {
            schema_version: CONTRACT_SCHEMA_VERSION,
            preset_id,
            revision,
            display_name,
            roles,
            digest: Digest::from_bytes(b""),
        };
        preset.digest = preset.calculate_digest()?;
        preset.validate()?;
        Ok(preset)
    }

    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = RolePresetDigestDocument {
            schema_version: self.schema_version,
            preset_id: &self.preset_id,
            revision: self.revision,
            display_name: &self.display_name,
            roles: &self.roles,
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        check_id("preset_id", &self.preset_id, &mut issues);
        check_nonblank("display_name", &self.display_name, 128, &mut issues);
        if self.schema_version != CONTRACT_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                format!("expected {CONTRACT_SCHEMA_VERSION}"),
            ));
        }
        let mut profile_ids = std::collections::HashSet::new();
        for (index, role) in self.roles.iter().enumerate() {
            if role.core_id != CoreId::ALL[index] {
                issues.push(ValidationIssue::new(
                    format!("roles[{index}].core_id"),
                    "core_order",
                    format!("expected {} in this position", CoreId::ALL[index]),
                ));
            }
            if !profile_ids.insert(&role.profile_id) {
                issues.push(ValidationIssue::new(
                    format!("roles[{index}].profile_id"),
                    "duplicate_profile_id",
                    "each core role must have a distinct profile ID",
                ));
            }
            if let Err(DomainError::Validation(mut nested)) = role.validate() {
                issues.append(&mut nested);
            }
        }
        if !self.digest.is_valid() || self.calculate_digest()? != self.digest {
            issues.push(ValidationIssue::new(
                "digest",
                "digest_mismatch",
                "role preset digest does not match its canonical content",
            ));
        }
        finish(issues)
    }

    pub fn compose(
        &self,
        role_set_id: OpaqueId,
        bindings: [ModelBindingSnapshot; 3],
    ) -> Result<RoleSetSnapshot, DomainError> {
        self.validate()?;
        let mut profiles = Vec::with_capacity(3);
        for (index, (definition, binding)) in self.roles.iter().zip(bindings).enumerate() {
            if binding.provider_profile_id.trim().is_empty() {
                return Err(DomainError::Validation(vec![ValidationIssue::new(
                    format!("bindings[{index}].provider_profile_id"),
                    "invalid_binding",
                    "each role requires a provider binding",
                )]));
            }
            profiles.push(CoreRoleProfile {
                core_id: definition.core_id,
                profile_id: definition.profile_id.clone(),
                revision: self.revision,
                display_name: definition.display_name.clone(),
                review_purpose: definition.review_purpose.clone(),
                evaluation_criteria: definition.evaluation_criteria.clone(),
                falsification_questions: definition.falsification_questions.clone(),
                response_language: definition.response_language.clone(),
                binding,
            });
        }
        let profiles: [CoreRoleProfile; 3] = profiles.try_into().map_err(|_| {
            DomainError::Validation(vec![ValidationIssue::new(
                "roles",
                "role_count",
                "a role preset must contain exactly three core roles",
            )])
        })?;
        RoleSetSnapshot::new(role_set_id, profiles)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreRoleProfile {
    pub core_id: CoreId,
    pub profile_id: OpaqueId,
    pub revision: u64,
    pub display_name: String,
    pub review_purpose: String,
    pub evaluation_criteria: Vec<String>,
    pub falsification_questions: Vec<String>,
    pub response_language: String,
    pub binding: ModelBindingSnapshot,
}

impl CoreRoleProfile {
    pub fn validate(&self) -> Result<(), DomainError> {
        let path = format!("roles.{}", self.core_id.wire_name());
        let mut issues = Vec::new();
        check_id(&format!("{path}.profile_id"), &self.profile_id, &mut issues);
        check_nonblank(
            &format!("{path}.display_name"),
            &self.display_name,
            128,
            &mut issues,
        );
        check_nonblank(
            &format!("{path}.review_purpose"),
            &self.review_purpose,
            8_000,
            &mut issues,
        );
        check_strings(
            &format!("{path}.evaluation_criteria"),
            &self.evaluation_criteria,
            64,
            2_000,
            &mut issues,
        );
        check_strings(
            &format!("{path}.falsification_questions"),
            &self.falsification_questions,
            64,
            2_000,
            &mut issues,
        );
        check_nonblank(
            &format!("{path}.response_language"),
            &self.response_language,
            64,
            &mut issues,
        );
        self.binding.validate(&format!("{path}.binding"))?;
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSetSnapshot {
    pub schema_version: u16,
    pub role_set_id: OpaqueId,
    pub roles: [CoreRoleProfile; 3],
    pub digest: Digest,
}

#[derive(Serialize)]
struct RoleSetDigestDocument<'a> {
    schema_version: u16,
    role_set_id: &'a str,
    roles: &'a [CoreRoleProfile; 3],
}

impl RoleSetSnapshot {
    pub fn new(role_set_id: OpaqueId, roles: [CoreRoleProfile; 3]) -> Result<Self, DomainError> {
        let mut snapshot = Self {
            schema_version: CONTRACT_SCHEMA_VERSION,
            role_set_id,
            roles,
            digest: Digest::from_bytes(b""),
        };
        snapshot.digest = snapshot.calculate_digest()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = RoleSetDigestDocument {
            schema_version: self.schema_version,
            role_set_id: &self.role_set_id,
            roles: &self.roles,
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        check_id("role_set_id", &self.role_set_id, &mut issues);
        if self.schema_version != CONTRACT_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                format!("expected {CONTRACT_SCHEMA_VERSION}"),
            ));
        }
        for (index, role) in self.roles.iter().enumerate() {
            if role.core_id != CoreId::ALL[index] {
                issues.push(ValidationIssue::new(
                    format!("roles[{index}].core_id"),
                    "core_order",
                    format!("expected {} in this position", CoreId::ALL[index]),
                ));
            }
            if let Err(DomainError::Validation(mut nested)) = role.validate() {
                issues.append(&mut nested);
            }
        }
        if !self.digest.is_valid() || self.calculate_digest()? != self.digest {
            issues.push(ValidationIssue::new(
                "digest",
                "digest_mismatch",
                "role set digest does not match its canonical content",
            ));
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSnapshot {
    pub source_id: OpaqueId,
    pub object_digest: Digest,
    #[serde(default)]
    pub allowed_locators: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextManifest {
    pub schema_version: u16,
    pub manifest_id: OpaqueId,
    pub sources: Vec<SourceSnapshot>,
    pub digest: Digest,
}

#[derive(Serialize)]
struct ContextDigestDocument<'a> {
    schema_version: u16,
    manifest_id: &'a str,
    sources: &'a [SourceSnapshot],
}

impl ContextManifest {
    pub fn new(manifest_id: OpaqueId, sources: Vec<SourceSnapshot>) -> Result<Self, DomainError> {
        let mut manifest = Self {
            schema_version: CONTRACT_SCHEMA_VERSION,
            manifest_id,
            sources,
            digest: Digest::from_bytes(b""),
        };
        manifest.digest = manifest.calculate_digest()?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = ContextDigestDocument {
            schema_version: self.schema_version,
            manifest_id: &self.manifest_id,
            sources: &self.sources,
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn contains_evidence(&self, reference: &EvidenceRef) -> bool {
        self.sources.iter().any(|source| {
            source.source_id == reference.source_id
                && source.object_digest == reference.object_digest
                && source
                    .allowed_locators
                    .iter()
                    .any(|locator| locator == &reference.locator)
        })
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        check_id("manifest_id", &self.manifest_id, &mut issues);
        if self.schema_version != CONTRACT_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                format!("expected {CONTRACT_SCHEMA_VERSION}"),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for (index, source) in self.sources.iter().enumerate() {
            check_id(
                &format!("sources[{index}].source_id"),
                &source.source_id,
                &mut issues,
            );
            if !source.object_digest.is_valid() {
                issues.push(ValidationIssue::new(
                    format!("sources[{index}].object_digest"),
                    "digest_format",
                    "source object digest must be a SHA-256 digest",
                ));
            }
            if !seen.insert(&source.source_id) {
                issues.push(ValidationIssue::new(
                    format!("sources[{index}].source_id"),
                    "duplicate_source",
                    "a context manifest cannot contain the same source ID twice",
                ));
            }
            for locator in &source.allowed_locators {
                if locator.trim().is_empty() || locator.len() > 512 {
                    issues.push(ValidationIssue::new(
                        format!("sources[{index}].allowed_locators"),
                        "invalid_locator",
                        "source locator must be non-empty and at most 512 UTF-8 bytes",
                    ));
                }
            }
        }
        if !self.digest.is_valid() || self.calculate_digest()? != self.digest {
            issues.push(ValidationIssue::new(
                "digest",
                "digest_mismatch",
                "context manifest digest does not match its canonical content",
            ));
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSnapshot {
    pub schema_version: u16,
    pub question: QuestionSnapshot,
    pub context_manifest: ContextManifest,
    pub role_set: RoleSetSnapshot,
    pub policy_digest: Digest,
    pub protocol_version: u16,
    pub input_digest: Digest,
}

#[derive(Serialize)]
struct InputDigestDocument<'a> {
    schema_version: u16,
    question_digest: &'a Digest,
    context_digest: &'a Digest,
    roles_digest: &'a Digest,
    policy_digest: &'a Digest,
    protocol_version: u16,
}

impl InputSnapshot {
    pub fn new(
        question: QuestionSnapshot,
        context_manifest: ContextManifest,
        role_set: RoleSetSnapshot,
        policy_digest: Digest,
    ) -> Result<Self, DomainError> {
        let mut snapshot = Self {
            schema_version: CONTRACT_SCHEMA_VERSION,
            question,
            context_manifest,
            role_set,
            policy_digest,
            protocol_version: DELIBERATION_PROTOCOL_VERSION,
            input_digest: Digest::from_bytes(b""),
        };
        snapshot.input_digest = snapshot.calculate_digest()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = InputDigestDocument {
            schema_version: self.schema_version,
            question_digest: &self.question.digest,
            context_digest: &self.context_manifest.digest,
            roles_digest: &self.role_set.digest,
            policy_digest: &self.policy_digest,
            protocol_version: self.protocol_version,
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        if self.schema_version != CONTRACT_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                format!("expected {CONTRACT_SCHEMA_VERSION}"),
            ));
        }
        if self.protocol_version != DELIBERATION_PROTOCOL_VERSION {
            issues.push(ValidationIssue::new(
                "protocol_version",
                "unsupported_protocol_version",
                format!("expected {DELIBERATION_PROTOCOL_VERSION}"),
            ));
        }
        if !self.policy_digest.is_valid() {
            issues.push(ValidationIssue::new(
                "policy_digest",
                "digest_format",
                "policy digest must be a SHA-256 digest",
            ));
        }
        for validation in [
            self.question.validate(),
            self.context_manifest.validate(),
            self.role_set.validate(),
        ] {
            if let Err(DomainError::Validation(mut nested)) = validation {
                issues.append(&mut nested);
            }
        }
        if !self.input_digest.is_valid() || self.calculate_digest()? != self.input_digest {
            issues.push(ValidationIssue::new(
                "input_digest",
                "digest_mismatch",
                "input digest does not match the selected snapshots and policy",
            ));
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub source_id: OpaqueId,
    pub object_digest: Digest,
    pub locator: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    SourceFact,
    ModelKnowledge,
    Inference,
    Preference,
    Assumption,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub claim_id: OpaqueId,
    pub kind: ClaimKind,
    pub text: String,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentStage {
    IndependentReview,
    CrossReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimResponseKind {
    Agree,
    Challenge,
    NeedsEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimResponse {
    pub target_claim_id: OpaqueId,
    pub response: ClaimResponseKind,
    pub rationale: String,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PositionChange {
    pub claim_id: OpaqueId,
    pub influenced_by_claim_ids: Vec<OpaqueId>,
    pub rationale: String,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InformationGap {
    pub missing_information: String,
    pub impact: String,
    pub essential: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counterargument {
    pub target_claim_id: Option<OpaqueId>,
    pub rationale: String,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleAssessment {
    pub schema_version: u16,
    pub run_id: OpaqueId,
    pub attempt_id: OpaqueId,
    pub core_id: CoreId,
    pub stage: AssessmentStage,
    pub input_digest: Digest,
    pub attempt_generation: u64,
    pub position_summary: String,
    pub claims: Vec<Claim>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub information_gaps: Vec<InformationGap>,
    #[serde(default)]
    pub counterarguments: Vec<Counterargument>,
    #[serde(default)]
    pub claim_responses: Vec<ClaimResponse>,
    #[serde(default)]
    pub position_changes: Vec<PositionChange>,
    pub created_at: String,
}

impl RoleAssessment {
    pub fn validate(
        &self,
        manifest: &ContextManifest,
        available_claim_ids: &[OpaqueId],
    ) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        if self.schema_version != CONTRACT_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                format!("expected {CONTRACT_SCHEMA_VERSION}"),
            ));
        }
        check_id("run_id", &self.run_id, &mut issues);
        check_id("attempt_id", &self.attempt_id, &mut issues);
        check_digest("input_digest", &self.input_digest, &mut issues);
        check_nonblank(
            "position_summary",
            &self.position_summary,
            32_000,
            &mut issues,
        );
        check_nonblank("created_at", &self.created_at, 64, &mut issues);
        check_strings("assumptions", &self.assumptions, 128, 8_000, &mut issues);
        if self.claims.len() > 256 {
            issues.push(ValidationIssue::new(
                "claims",
                "item_limit",
                "at most 256 claims are allowed per assessment",
            ));
        }
        let mut claim_ids = std::collections::HashSet::new();
        for (index, claim) in self.claims.iter().enumerate() {
            check_id(
                &format!("claims[{index}].claim_id"),
                &claim.claim_id,
                &mut issues,
            );
            if !claim_ids.insert(&claim.claim_id) {
                issues.push(ValidationIssue::new(
                    format!("claims[{index}].claim_id"),
                    "duplicate_claim",
                    "claim IDs must be unique within an assessment",
                ));
            }
            check_nonblank(
                &format!("claims[{index}].text"),
                &claim.text,
                8_000,
                &mut issues,
            );
            check_strings(
                &format!("claims[{index}].limitations"),
                &claim.limitations,
                64,
                2_000,
                &mut issues,
            );
            validate_evidence(
                &format!("claims[{index}].evidence_refs"),
                &claim.evidence_refs,
                manifest,
                &mut issues,
            );
            if claim.kind == ClaimKind::SourceFact && claim.evidence_refs.is_empty() {
                issues.push(ValidationIssue::new(
                    format!("claims[{index}].evidence_refs"),
                    "source_fact_without_evidence",
                    "source_fact claims require at least one source reference",
                ));
            }
        }
        for (index, gap) in self.information_gaps.iter().enumerate() {
            check_nonblank(
                &format!("information_gaps[{index}].missing_information"),
                &gap.missing_information,
                8_000,
                &mut issues,
            );
            check_nonblank(
                &format!("information_gaps[{index}].impact"),
                &gap.impact,
                8_000,
                &mut issues,
            );
        }
        for (index, counterargument) in self.counterarguments.iter().enumerate() {
            if let Some(id) = &counterargument.target_claim_id {
                validate_claim_reference(
                    &format!("counterarguments[{index}].target_claim_id"),
                    id,
                    available_claim_ids,
                    &mut issues,
                );
            }
            check_nonblank(
                &format!("counterarguments[{index}].rationale"),
                &counterargument.rationale,
                8_000,
                &mut issues,
            );
            validate_evidence(
                &format!("counterarguments[{index}].evidence_refs"),
                &counterargument.evidence_refs,
                manifest,
                &mut issues,
            );
        }
        match self.stage {
            AssessmentStage::IndependentReview
                if !self.claim_responses.is_empty() || !self.position_changes.is_empty() =>
            {
                issues.push(ValidationIssue::new(
                    "stage",
                    "cross_fields_in_independent_review",
                    "independent review cannot contain cross-review fields",
                ));
            }
            AssessmentStage::CrossReview => {
                if available_claim_ids.is_empty() {
                    issues.push(ValidationIssue::new(
                        "stage",
                        "cross_review_without_targets",
                        "cross-review requires the three independent assessments",
                    ));
                }
                for (index, response) in self.claim_responses.iter().enumerate() {
                    validate_claim_reference(
                        &format!("claim_responses[{index}].target_claim_id"),
                        &response.target_claim_id,
                        available_claim_ids,
                        &mut issues,
                    );
                    check_nonblank(
                        &format!("claim_responses[{index}].rationale"),
                        &response.rationale,
                        8_000,
                        &mut issues,
                    );
                    validate_evidence(
                        &format!("claim_responses[{index}].evidence_refs"),
                        &response.evidence_refs,
                        manifest,
                        &mut issues,
                    );
                }
                for (index, change) in self.position_changes.iter().enumerate() {
                    validate_claim_reference(
                        &format!("position_changes[{index}].claim_id"),
                        &change.claim_id,
                        available_claim_ids,
                        &mut issues,
                    );
                    if change.influenced_by_claim_ids.is_empty() {
                        issues.push(ValidationIssue::new(
                            format!("position_changes[{index}].influenced_by_claim_ids"),
                            "missing_change_basis",
                            "a position change must identify the claims that influenced it",
                        ));
                    }
                    for (reference_index, id) in change.influenced_by_claim_ids.iter().enumerate() {
                        validate_claim_reference(
                            &format!(
                                "position_changes[{index}].influenced_by_claim_ids[{reference_index}]"
                            ),
                            id,
                            available_claim_ids,
                            &mut issues,
                        );
                    }
                    check_nonblank(
                        &format!("position_changes[{index}].rationale"),
                        &change.rationale,
                        8_000,
                        &mut issues,
                    );
                    validate_evidence(
                        &format!("position_changes[{index}].evidence_refs"),
                        &change.evidence_refs,
                        manifest,
                        &mut issues,
                    );
                }
            }
            _ => {}
        }
        finish(issues)
    }

    pub fn has_essential_gap(&self) -> bool {
        self.information_gaps.iter().any(|gap| gap.essential)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalClaim {
    pub claim_id: OpaqueId,
    pub kind: ClaimKind,
    pub text: String,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenObjection {
    pub claim_id: OpaqueId,
    pub rationale: String,
    #[serde(default)]
    pub required_information: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalSnapshot {
    pub schema_version: u16,
    pub proposal_id: OpaqueId,
    pub run_id: OpaqueId,
    pub question_digest: Digest,
    pub context_digest: Digest,
    pub roles_digest: Digest,
    pub kind: QuestionKind,
    pub body: String,
    pub claims: Vec<ProposalClaim>,
    #[serde(default)]
    pub conditions: Vec<String>,
    #[serde(default)]
    pub alternatives: Vec<String>,
    #[serde(default)]
    pub open_objections: Vec<OpenObjection>,
    pub digest: Digest,
    pub created_at: String,
}

#[derive(Serialize)]
struct ProposalDigestDocument<'a> {
    schema_version: u16,
    proposal_id: &'a str,
    run_id: &'a str,
    question_digest: &'a Digest,
    context_digest: &'a Digest,
    roles_digest: &'a Digest,
    kind: QuestionKind,
    body: &'a str,
    claims: &'a [ProposalClaim],
    conditions: &'a [String],
    alternatives: &'a [String],
    open_objections: &'a [OpenObjection],
}

impl ProposalSnapshot {
    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = ProposalDigestDocument {
            schema_version: self.schema_version,
            proposal_id: &self.proposal_id,
            run_id: &self.run_id,
            question_digest: &self.question_digest,
            context_digest: &self.context_digest,
            roles_digest: &self.roles_digest,
            kind: self.kind,
            body: &self.body,
            claims: &self.claims,
            conditions: &self.conditions,
            alternatives: &self.alternatives,
            open_objections: &self.open_objections,
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn seal(mut self) -> Result<Self, DomainError> {
        self.digest = self.calculate_digest()?;
        self.validate(None)?;
        Ok(self)
    }

    pub fn validate(&self, manifest: Option<&ContextManifest>) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        if self.schema_version != CONTRACT_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                format!("expected {CONTRACT_SCHEMA_VERSION}"),
            ));
        }
        for (path, id) in [("proposal_id", &self.proposal_id), ("run_id", &self.run_id)] {
            check_id(path, id, &mut issues);
        }
        for (path, digest) in [
            ("question_digest", &self.question_digest),
            ("context_digest", &self.context_digest),
            ("roles_digest", &self.roles_digest),
            ("digest", &self.digest),
        ] {
            check_digest(path, digest, &mut issues);
        }
        check_nonblank("body", &self.body, 256 * 1024, &mut issues);
        check_nonblank("created_at", &self.created_at, 64, &mut issues);
        check_strings("conditions", &self.conditions, 128, 8_000, &mut issues);
        check_strings("alternatives", &self.alternatives, 128, 8_000, &mut issues);
        let mut claim_ids = std::collections::HashSet::new();
        for (index, claim) in self.claims.iter().enumerate() {
            check_id(
                &format!("claims[{index}].claim_id"),
                &claim.claim_id,
                &mut issues,
            );
            if !claim_ids.insert(&claim.claim_id) {
                issues.push(ValidationIssue::new(
                    format!("claims[{index}].claim_id"),
                    "duplicate_claim",
                    "proposal claim IDs must be unique",
                ));
            }
            check_nonblank(
                &format!("claims[{index}].text"),
                &claim.text,
                8_000,
                &mut issues,
            );
            if claim.kind == ClaimKind::SourceFact && claim.evidence_refs.is_empty() {
                issues.push(ValidationIssue::new(
                    format!("claims[{index}].evidence_refs"),
                    "source_fact_without_evidence",
                    "source_fact claims require at least one source reference",
                ));
            }
            if let Some(manifest) = manifest {
                validate_evidence(
                    &format!("claims[{index}].evidence_refs"),
                    &claim.evidence_refs,
                    manifest,
                    &mut issues,
                );
            }
        }
        for (index, objection) in self.open_objections.iter().enumerate() {
            if !claim_ids.contains(&objection.claim_id) {
                issues.push(ValidationIssue::new(
                    format!("open_objections[{index}].claim_id"),
                    "unknown_claim",
                    "open objection must reference a claim in this proposal",
                ));
            }
            check_nonblank(
                &format!("open_objections[{index}].rationale"),
                &objection.rationale,
                8_000,
                &mut issues,
            );
            check_strings(
                &format!("open_objections[{index}].required_information"),
                &objection.required_information,
                64,
                2_000,
                &mut issues,
            );
        }
        if self.calculate_digest()? != self.digest {
            issues.push(ValidationIssue::new(
                "digest",
                "digest_mismatch",
                "proposal digest does not match its canonical JCS content",
            ));
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStage {
    IndependentReview,
    CrossReview,
    Synthesis,
    Balloting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    Auth,
    Quota,
    NeedsInput,
    Validation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunStatus {
    Preparing,
    AwaitingConfirmation,
    IndependentReview,
    CrossReview,
    Synthesis,
    Balloting,
    Paused {
        reason: PauseReason,
        detail: String,
        resume_stage: RunStage,
    },
    Interrupted {
        resume_stage: RunStage,
        detail: String,
        cancellation_requested: bool,
    },
    Cancelling,
    Completed {
        outcome: crate::Outcome,
    },
    Cancelled,
    Failed {
        reason_code: String,
    },
}

impl RunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Cancelled | Self::Failed { .. }
        )
    }

    pub fn stage(&self) -> Option<RunStage> {
        match self {
            Self::IndependentReview => Some(RunStage::IndependentReview),
            Self::CrossReview => Some(RunStage::CrossReview),
            Self::Synthesis => Some(RunStage::Synthesis),
            Self::Balloting => Some(RunStage::Balloting),
            Self::Paused { resume_stage, .. } | Self::Interrupted { resume_stage, .. } => {
                Some(*resume_stage)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub schema_version: u16,
    pub run_id: OpaqueId,
    pub conversation_id: OpaqueId,
    pub parent_run_id: Option<OpaqueId>,
    pub question_id: OpaqueId,
    pub context_manifest_id: OpaqueId,
    pub role_set_id: OpaqueId,
    pub input_digest: Digest,
    pub protocol_version: u16,
    pub status: RunStatus,
    pub revision: u64,
    pub generation: u64,
    pub created_at: String,
    pub updated_at: String,
}

impl Run {
    pub fn new(
        run_id: OpaqueId,
        conversation_id: OpaqueId,
        parent_run_id: Option<OpaqueId>,
        input: &InputSnapshot,
        created_at: String,
    ) -> Result<Self, DomainError> {
        input.validate()?;
        let run = Self {
            schema_version: CONTRACT_SCHEMA_VERSION,
            run_id,
            conversation_id,
            parent_run_id,
            question_id: input.question.question_id.clone(),
            context_manifest_id: input.context_manifest.manifest_id.clone(),
            role_set_id: input.role_set.role_set_id.clone(),
            input_digest: input.input_digest.clone(),
            protocol_version: DELIBERATION_PROTOCOL_VERSION,
            status: RunStatus::Preparing,
            revision: 0,
            generation: 0,
            updated_at: created_at.clone(),
            created_at,
        };
        run.validate()?;
        Ok(run)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        for (path, id) in [
            ("run_id", &self.run_id),
            ("conversation_id", &self.conversation_id),
            ("question_id", &self.question_id),
            ("context_manifest_id", &self.context_manifest_id),
            ("role_set_id", &self.role_set_id),
        ] {
            check_id(path, id, &mut issues);
        }
        if let Some(parent_id) = &self.parent_run_id {
            check_id("parent_run_id", parent_id, &mut issues);
        }
        check_digest("input_digest", &self.input_digest, &mut issues);
        check_nonblank("created_at", &self.created_at, 64, &mut issues);
        check_nonblank("updated_at", &self.updated_at, 64, &mut issues);
        if self.schema_version != CONTRACT_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                format!("expected {CONTRACT_SCHEMA_VERSION}"),
            ));
        }
        if self.protocol_version != DELIBERATION_PROTOCOL_VERSION {
            issues.push(ValidationIssue::new(
                "protocol_version",
                "unsupported_protocol_version",
                format!("expected {DELIBERATION_PROTOCOL_VERSION}"),
            ));
        }
        finish(issues)
    }

    pub fn transition(
        &mut self,
        next: RunStatus,
        expected_revision: u64,
        at: String,
    ) -> Result<(), DomainError> {
        if self.revision != expected_revision {
            return Err(DomainError::RevisionConflict {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        if self.status.is_terminal() {
            return Err(DomainError::TerminalRun);
        }
        if !valid_transition(&self.status, &next) {
            return Err(DomainError::InvalidTransition {
                from: status_name(&self.status).to_owned(),
                to: status_name(&next).to_owned(),
            });
        }
        if self.status != next {
            self.status = next;
            self.revision += 1;
            self.updated_at = at;
        }
        Ok(())
    }

    pub fn fence(&mut self, expected_revision: u64, at: String) -> Result<(), DomainError> {
        if self.revision != expected_revision {
            return Err(DomainError::RevisionConflict {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        if self.status.is_terminal() {
            return Err(DomainError::TerminalRun);
        }
        self.generation += 1;
        self.revision += 1;
        self.updated_at = at;
        Ok(())
    }
}

pub fn valid_transition(current: &RunStatus, next: &RunStatus) -> bool {
    use RunStatus::*;
    match current {
        Preparing => matches!(
            next,
            AwaitingConfirmation | Paused { .. } | Interrupted { .. } | Cancelling | Failed { .. }
        ),
        AwaitingConfirmation => matches!(
            next,
            IndependentReview | Paused { .. } | Interrupted { .. } | Cancelling | Failed { .. }
        ),
        IndependentReview => matches!(
            next,
            CrossReview | Paused { .. } | Interrupted { .. } | Cancelling | Failed { .. }
        ),
        CrossReview => matches!(
            next,
            Synthesis | Paused { .. } | Interrupted { .. } | Cancelling | Failed { .. }
        ),
        Synthesis => matches!(
            next,
            Balloting | Paused { .. } | Interrupted { .. } | Cancelling | Failed { .. }
        ),
        Balloting => matches!(
            next,
            Completed { .. } | Paused { .. } | Interrupted { .. } | Cancelling | Failed { .. }
        ),
        Paused { .. } => matches!(
            next,
            IndependentReview
                | CrossReview
                | Synthesis
                | Balloting
                | Interrupted { .. }
                | Cancelling
                | Failed { .. }
        ),
        Interrupted { .. } => matches!(
            next,
            IndependentReview | CrossReview | Synthesis | Balloting | Cancelling | Failed { .. }
        ),
        Cancelling => matches!(
            next,
            Cancelled
                | Interrupted {
                    cancellation_requested: true,
                    ..
                }
        ),
        Completed { .. } | Cancelled | Failed { .. } => false,
    }
}

pub fn status_name(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Preparing => "preparing",
        RunStatus::AwaitingConfirmation => "awaiting_confirmation",
        RunStatus::IndependentReview => "independent_review",
        RunStatus::CrossReview => "cross_review",
        RunStatus::Synthesis => "synthesis",
        RunStatus::Balloting => "balloting",
        RunStatus::Paused { .. } => "paused",
        RunStatus::Interrupted { .. } => "interrupted",
        RunStatus::Cancelling => "cancelling",
        RunStatus::Completed { .. } => "completed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Failed { .. } => "failed",
    }
}

fn validate_evidence(
    path: &str,
    references: &[EvidenceRef],
    manifest: &ContextManifest,
    issues: &mut Vec<ValidationIssue>,
) {
    for (index, reference) in references.iter().enumerate() {
        if !manifest.contains_evidence(reference) {
            issues.push(ValidationIssue::new(
                format!("{path}[{index}]"),
                "evidence_outside_manifest",
                "evidence must reference an exact source digest and locator in the approved manifest",
            ));
        }
    }
}

fn validate_claim_reference(
    path: &str,
    id: &str,
    available: &[OpaqueId],
    issues: &mut Vec<ValidationIssue>,
) {
    if !available.iter().any(|candidate| candidate == id) {
        issues.push(ValidationIssue::new(
            path,
            "unknown_claim",
            "claim reference is not present in the allowed review inputs",
        ));
    }
}

fn check_id(path: &str, value: &str, issues: &mut Vec<ValidationIssue>) {
    if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        issues.push(ValidationIssue::new(path, "invalid_id", "opaque IDs must be non-empty, at most 128 UTF-8 bytes, and contain no control characters"));
    }
}

fn check_digest(path: &str, value: &Digest, issues: &mut Vec<ValidationIssue>) {
    if !value.is_valid() {
        issues.push(ValidationIssue::new(
            path,
            "digest_format",
            "value must be a SHA-256 digest",
        ));
    }
}

fn check_nonblank(path: &str, value: &str, maximum: usize, issues: &mut Vec<ValidationIssue>) {
    if value.trim().is_empty() || value.len() > maximum {
        issues.push(ValidationIssue::new(
            path,
            "text_bounds",
            format!("value must be non-empty and at most {maximum} UTF-8 bytes"),
        ));
    }
}

fn check_strings(
    path: &str,
    values: &[String],
    max_items: usize,
    max_item_bytes: usize,
    issues: &mut Vec<ValidationIssue>,
) {
    if values.len() > max_items {
        issues.push(ValidationIssue::new(
            path,
            "item_limit",
            format!("at most {max_items} entries are allowed"),
        ));
    }
    for (index, value) in values.iter().enumerate() {
        check_nonblank(&format!("{path}[{index}]"), value, max_item_bytes, issues);
    }
}

fn check_ids(path: &str, values: &[OpaqueId], issues: &mut Vec<ValidationIssue>) {
    for (index, value) in values.iter().enumerate() {
        check_id(&format!("{path}[{index}]"), value, issues);
    }
}

fn finish(issues: Vec<ValidationIssue>) -> Result<(), DomainError> {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(DomainError::Validation(issues))
    }
}
