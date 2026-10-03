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

pub const PROVIDER_CATALOG_SCHEMA_VERSION: u16 = 1;
pub const ACP_MODEL_BINDING_SCHEMA_VERSION: u16 = 1;
pub const MODE_ATTESTED_CATALOG_SCHEMA_VERSION: u16 = 2;
pub const MODE_ATTESTED_BINDING_SCHEMA_VERSION: u16 = 2;
pub const ARTIFACT_ATTESTED_CATALOG_SCHEMA_VERSION: u16 = 3;
pub const ARTIFACT_ATTESTED_BINDING_SCHEMA_VERSION: u16 = 3;
pub const LIVE_RUN_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpMode {
    Acp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCatalogModel {
    pub model_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub context_window_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCatalogMode {
    pub mode_id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NegotiatedModeState {
    pub current_mode_id: Option<String>,
    pub modes: Vec<ProviderCatalogMode>,
}

impl NegotiatedModeState {
    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        if self.modes.len() > 128 {
            issues.push(ValidationIssue::new(
                "negotiatedModes.modes",
                "item_limit",
                "at most 128 modes are accepted",
            ));
        }
        for (index, mode) in self.modes.iter().enumerate() {
            let path = format!("negotiatedModes.modes[{index}]");
            check_nonblank(&format!("{path}.modeId"), &mode.mode_id, 512, &mut issues);
            check_nonblank(&format!("{path}.name"), &mode.name, 512, &mut issues);
            if mode.mode_id.chars().any(char::is_control) {
                issues.push(ValidationIssue::new(
                    format!("{path}.modeId"),
                    "control_character",
                    "mode IDs cannot contain control characters",
                ));
            }
            if let Some(description) = &mode.description {
                check_nonblank(
                    &format!("{path}.description"),
                    description,
                    4096,
                    &mut issues,
                );
            }
            if index > 0 && self.modes[index - 1].mode_id >= mode.mode_id {
                issues.push(ValidationIssue::new(
                    format!("{path}.modeId"),
                    "mode_order_or_duplicate",
                    "mode IDs must be unique and sorted",
                ));
            }
        }
        match (&self.current_mode_id, self.modes.is_empty()) {
            (None, true) => {}
            (Some(id), false) if self.modes.iter().any(|mode| &mode.mode_id == id) => {}
            _ => issues.push(ValidationIssue::new(
                "negotiatedModes.currentModeId",
                "mode_not_in_catalog",
                "the observed current mode must match the advertised facility",
            )),
        }
        finish(issues)
    }

    pub fn validate_selection(&self, mode_id: Option<&str>) -> Result<(), DomainError> {
        self.validate()?;
        if (self.modes.is_empty() && mode_id.is_none())
            || mode_id.is_some_and(|id| self.modes.iter().any(|mode| mode.mode_id == id))
        {
            Ok(())
        } else {
            Err(DomainError::Precondition {
                required: "an explicit advertised mode, or attested absence of modes".into(),
                actual: "selected mode does not match the negotiated facility".into(),
            })
        }
    }
}

fn deserialize_artifact_set_digest<'de, D>(deserializer: D) -> Result<Option<Digest>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Digest::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCatalogSnapshot {
    pub schema_version: u16,
    pub catalog_snapshot_id: OpaqueId,
    pub catalog_digest: Digest,
    pub provider_id: String,
    pub acp_mode: AcpMode,
    pub provider_profile_id: OpaqueId,
    pub profile_revision: u64,
    pub adapter_id: String,
    pub adapter_version: String,
    pub adapter_digest: Digest,
    #[serde(
        default,
        deserialize_with = "deserialize_artifact_set_digest",
        skip_serializing_if = "Option::is_none"
    )]
    pub artifact_set_digest: Option<Digest>,
    pub fetched_at: String,
    pub models: Vec<ProviderCatalogModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negotiated_modes: Option<NegotiatedModeState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogExecutionWitness {
    pub binding: AcpModelBindingSnapshot,
    pub original_catalog: ProviderCatalogSnapshot,
    pub fresh_catalog: ProviderCatalogSnapshot,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderCatalogDigestDocument<'a> {
    schema_version: u16,
    provider_id: &'a str,
    acp_mode: AcpMode,
    provider_profile_id: &'a str,
    profile_revision: u64,
    adapter_id: &'a str,
    adapter_version: &'a str,
    adapter_digest: &'a Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_set_digest: Option<&'a Digest>,
    fetched_at: &'a str,
    models: &'a [ProviderCatalogModel],
    #[serde(skip_serializing_if = "Option::is_none")]
    negotiated_modes: Option<&'a NegotiatedModeState>,
}

pub struct ProviderCatalogInput {
    pub catalog_snapshot_id: OpaqueId,
    pub provider_id: String,
    pub provider_profile_id: OpaqueId,
    pub profile_revision: u64,
    pub adapter_id: String,
    pub adapter_version: String,
    pub adapter_digest: Digest,
    pub fetched_at: String,
}

impl ProviderCatalogSnapshot {
    pub fn execution_equivalent_to(&self, fresh: &Self) -> Result<(), DomainError> {
        self.validate()?;
        fresh.validate()?;
        if self.schema_version != ARTIFACT_ATTESTED_CATALOG_SCHEMA_VERSION
            || fresh.schema_version != ARTIFACT_ATTESTED_CATALOG_SCHEMA_VERSION
        {
            return Err(DomainError::Precondition {
                required: "verified complete catalog execution evidence".into(),
                actual: "catalog execution evidence is unavailable".into(),
            });
        }
        let semantics = |catalog: &Self| -> Result<serde_json::Value, DomainError> {
            let mut value = serde_json::to_value(catalog)?;
            if let Some(fields) = value.as_object_mut() {
                for key in ["catalogSnapshotId", "fetchedAt", "catalogDigest"] {
                    fields.remove(key);
                }
            }
            Ok(value)
        };
        if semantics(self)? != semantics(fresh)? {
            return Err(DomainError::Precondition {
                required: "unchanged complete catalog execution semantics".into(),
                actual: "catalog changed; explicit selection is required".into(),
            });
        }
        Ok(())
    }

    pub fn new(
        input: ProviderCatalogInput,
        mut models: Vec<ProviderCatalogModel>,
    ) -> Result<Self, DomainError> {
        let ProviderCatalogInput {
            catalog_snapshot_id,
            provider_id,
            provider_profile_id,
            profile_revision,
            adapter_id,
            adapter_version,
            adapter_digest,
            fetched_at,
        } = input;
        models.sort_by(|left, right| left.model_id.cmp(&right.model_id));
        let mut snapshot = Self {
            schema_version: PROVIDER_CATALOG_SCHEMA_VERSION,
            catalog_snapshot_id,
            catalog_digest: Digest::from_bytes(b""),
            provider_id,
            acp_mode: AcpMode::Acp,
            provider_profile_id,
            profile_revision,
            adapter_id,
            adapter_version,
            adapter_digest,
            fetched_at,
            models,
            artifact_set_digest: None,
            negotiated_modes: None,
        };
        snapshot.catalog_digest = snapshot.calculate_digest()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn with_artifact_set_digest(mut self, digest: Digest) -> Result<Self, DomainError> {
        self.schema_version = ARTIFACT_ATTESTED_CATALOG_SCHEMA_VERSION;
        self.artifact_set_digest = Some(digest);
        self.catalog_digest = self.calculate_digest()?;
        self.validate()?;
        Ok(self)
    }

    pub fn with_negotiated_modes(
        mut self,
        mut modes: NegotiatedModeState,
    ) -> Result<Self, DomainError> {
        modes
            .modes
            .sort_by(|left, right| left.mode_id.cmp(&right.mode_id));
        modes.validate()?;
        self.schema_version = if self.artifact_set_digest.is_some() {
            ARTIFACT_ATTESTED_CATALOG_SCHEMA_VERSION
        } else {
            MODE_ATTESTED_CATALOG_SCHEMA_VERSION
        };
        self.negotiated_modes = Some(modes);
        self.catalog_digest = self.calculate_digest()?;
        self.validate()?;
        Ok(self)
    }

    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = ProviderCatalogDigestDocument {
            schema_version: self.schema_version,
            provider_id: &self.provider_id,
            acp_mode: self.acp_mode,
            provider_profile_id: &self.provider_profile_id,
            profile_revision: self.profile_revision,
            adapter_id: &self.adapter_id,
            adapter_version: &self.adapter_version,
            adapter_digest: &self.adapter_digest,
            artifact_set_digest: self.artifact_set_digest.as_ref(),
            fetched_at: &self.fetched_at,
            models: &self.models,
            negotiated_modes: self.negotiated_modes.as_ref(),
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        if !matches!(
            (
                self.schema_version,
                self.negotiated_modes.is_some(),
                self.artifact_set_digest.is_some()
            ),
            (PROVIDER_CATALOG_SCHEMA_VERSION, false, false)
                | (MODE_ATTESTED_CATALOG_SCHEMA_VERSION, true, false)
                | (ARTIFACT_ATTESTED_CATALOG_SCHEMA_VERSION, true, true)
        ) {
            issues.push(ValidationIssue::new(
                "schemaVersion",
                "unsupported_schema_version",
                "catalog version must match its mode and runtime artifact evidence",
            ));
        }
        check_id("catalogSnapshotId", &self.catalog_snapshot_id, &mut issues);
        check_nonblank("providerId", &self.provider_id, 128, &mut issues);
        check_id("providerProfileId", &self.provider_profile_id, &mut issues);
        check_nonblank("adapterId", &self.adapter_id, 256, &mut issues);
        check_nonblank("adapterVersion", &self.adapter_version, 128, &mut issues);
        check_nonblank("fetchedAt", &self.fetched_at, 64, &mut issues);
        check_digest("adapterDigest", &self.adapter_digest, &mut issues);
        if let Some(digest) = &self.artifact_set_digest {
            check_digest("artifactSetDigest", digest, &mut issues);
        }
        check_digest("catalogDigest", &self.catalog_digest, &mut issues);
        if self.models.is_empty() {
            issues.push(ValidationIssue::new(
                "models",
                "empty_catalog",
                "a live provider catalog must contain at least one returned model",
            ));
        }
        if self.models.len() > 512 {
            issues.push(ValidationIssue::new(
                "models",
                "item_limit",
                "a provider catalog may contain at most 512 models",
            ));
        }
        for (index, model) in self.models.iter().enumerate() {
            let path = format!("models[{index}]");
            check_nonblank(
                &format!("{path}.modelId"),
                &model.model_id,
                512,
                &mut issues,
            );
            if model.model_id.chars().any(char::is_control) {
                issues.push(ValidationIssue::new(
                    format!("{path}.modelId"),
                    "control_character",
                    "provider model IDs must not contain control characters",
                ));
            }
            if let Some(name) = &model.name {
                check_nonblank(&format!("{path}.name"), name, 512, &mut issues);
            }
            if let Some(description) = &model.description {
                check_nonblank(
                    &format!("{path}.description"),
                    description,
                    4096,
                    &mut issues,
                );
            }
            if index > 0 && self.models[index - 1].model_id >= model.model_id {
                issues.push(ValidationIssue::new(
                    format!("{path}.modelId"),
                    "catalog_order_or_duplicate",
                    "provider model IDs must be unique and sorted by their exact returned value",
                ));
            }
        }
        if let Some(modes) = &self.negotiated_modes {
            modes.validate()?;
        }
        if !self.catalog_digest.is_valid() || self.calculate_digest()? != self.catalog_digest {
            issues.push(ValidationIssue::new(
                "catalogDigest",
                "digest_mismatch",
                "catalog digest does not match its canonical provenance and returned models",
            ));
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcpModelBindingSnapshot {
    pub schema_version: u16,
    pub catalog_snapshot_id: OpaqueId,
    pub catalog_digest: Digest,
    pub provider_id: String,
    pub acp_mode: AcpMode,
    pub provider_profile_id: OpaqueId,
    pub profile_revision: u64,
    pub adapter_id: String,
    pub adapter_version: String,
    pub adapter_digest: Digest,
    #[serde(
        default,
        deserialize_with = "deserialize_artifact_set_digest",
        skip_serializing_if = "Option::is_none"
    )]
    pub artifact_set_digest: Option<Digest>,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode_id: Option<String>,
    pub binding_digest: Digest,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpModelBindingDigestDocument<'a> {
    schema_version: u16,
    catalog_snapshot_id: &'a str,
    catalog_digest: &'a Digest,
    provider_id: &'a str,
    acp_mode: AcpMode,
    provider_profile_id: &'a str,
    profile_revision: u64,
    adapter_id: &'a str,
    adapter_version: &'a str,
    adapter_digest: &'a Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_set_digest: Option<&'a Digest>,
    model_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode_id: Option<&'a str>,
}

impl AcpModelBindingSnapshot {
    pub fn from_catalog(
        catalog: &ProviderCatalogSnapshot,
        exact_model_id: &str,
    ) -> Result<Self, DomainError> {
        catalog.validate()?;
        if !catalog
            .models
            .iter()
            .any(|model| model.model_id == exact_model_id)
        {
            return Err(DomainError::Precondition {
                required: "an exact model ID returned by the bound live provider catalog".into(),
                actual: "model ID was not present in that catalog".into(),
            });
        }
        let mut binding = Self {
            schema_version: catalog.schema_version,
            catalog_snapshot_id: catalog.catalog_snapshot_id.clone(),
            catalog_digest: catalog.catalog_digest.clone(),
            provider_id: catalog.provider_id.clone(),
            acp_mode: catalog.acp_mode,
            provider_profile_id: catalog.provider_profile_id.clone(),
            profile_revision: catalog.profile_revision,
            adapter_id: catalog.adapter_id.clone(),
            adapter_version: catalog.adapter_version.clone(),
            adapter_digest: catalog.adapter_digest.clone(),
            artifact_set_digest: catalog.artifact_set_digest.clone(),
            model_id: exact_model_id.to_owned(),
            mode_id: None,
            binding_digest: Digest::from_bytes(b""),
        };
        binding.binding_digest = binding.calculate_digest()?;
        binding.validate(catalog)?;
        Ok(binding)
    }

    pub fn from_catalog_with_mode(
        catalog: &ProviderCatalogSnapshot,
        exact_model_id: &str,
        mode_id: Option<&str>,
    ) -> Result<Self, DomainError> {
        let modes = catalog
            .negotiated_modes
            .as_ref()
            .ok_or_else(|| DomainError::Precondition {
                required: "a catalog with observed mode negotiation".into(),
                actual: "catalog mode support is unknown; refresh the catalog".into(),
            })?;
        modes.validate_selection(mode_id)?;
        let mut binding = Self::from_catalog(catalog, exact_model_id)?;
        binding.mode_id = mode_id.map(str::to_owned);
        binding.binding_digest = binding.calculate_digest()?;
        binding.validate_ready(catalog)?;
        Ok(binding)
    }

    pub fn validate_ready(&self, catalog: &ProviderCatalogSnapshot) -> Result<(), DomainError> {
        self.validate(catalog)?;
        catalog
            .negotiated_modes
            .as_ref()
            .ok_or_else(|| DomainError::Precondition {
                required: "a catalog with observed mode negotiation".into(),
                actual: "catalog mode support is unknown; refresh the catalog".into(),
            })?
            .validate_selection(self.mode_id.as_deref())
    }

    pub fn validate_for_execution(
        &self,
        catalog: &ProviderCatalogSnapshot,
    ) -> Result<(), DomainError> {
        self.validate_ready(catalog)?;
        if self.artifact_set_digest.is_none() {
            return Err(DomainError::Precondition {
                required: "a catalog and binding with verified runtime artifact-set authority"
                    .into(),
                actual: "runtime artifact-set authority is unknown; refresh the catalog".into(),
            });
        }
        Ok(())
    }

    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = AcpModelBindingDigestDocument {
            schema_version: self.schema_version,
            catalog_snapshot_id: &self.catalog_snapshot_id,
            catalog_digest: &self.catalog_digest,
            provider_id: &self.provider_id,
            acp_mode: self.acp_mode,
            provider_profile_id: &self.provider_profile_id,
            profile_revision: self.profile_revision,
            adapter_id: &self.adapter_id,
            adapter_version: &self.adapter_version,
            adapter_digest: &self.adapter_digest,
            model_id: &self.model_id,
            artifact_set_digest: self.artifact_set_digest.as_ref(),
            mode_id: self.mode_id.as_deref(),
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn validate(&self, catalog: &ProviderCatalogSnapshot) -> Result<(), DomainError> {
        catalog.validate()?;
        let mut issues = Vec::new();
        if !matches!(
            self.schema_version,
            ACP_MODEL_BINDING_SCHEMA_VERSION
                | MODE_ATTESTED_BINDING_SCHEMA_VERSION
                | ARTIFACT_ATTESTED_BINDING_SCHEMA_VERSION
        ) || (self.schema_version == ACP_MODEL_BINDING_SCHEMA_VERSION && self.mode_id.is_some())
            || (self.schema_version == ARTIFACT_ATTESTED_BINDING_SCHEMA_VERSION)
                != self.artifact_set_digest.is_some()
        {
            issues.push(ValidationIssue::new(
                "schemaVersion",
                "unsupported_schema_version",
                "binding version must support its selected mode",
            ));
        }
        check_id("catalogSnapshotId", &self.catalog_snapshot_id, &mut issues);
        check_id("providerProfileId", &self.provider_profile_id, &mut issues);
        check_nonblank("providerId", &self.provider_id, 128, &mut issues);
        check_nonblank("adapterId", &self.adapter_id, 256, &mut issues);
        check_nonblank("adapterVersion", &self.adapter_version, 128, &mut issues);
        check_nonblank("modelId", &self.model_id, 512, &mut issues);
        check_digest("catalogDigest", &self.catalog_digest, &mut issues);
        check_digest("adapterDigest", &self.adapter_digest, &mut issues);
        check_digest("bindingDigest", &self.binding_digest, &mut issues);
        if let Some(digest) = &self.artifact_set_digest {
            check_digest("artifactSetDigest", digest, &mut issues);
        }
        if !self.binding_digest.is_valid() || self.calculate_digest()? != self.binding_digest {
            issues.push(ValidationIssue::new(
                "bindingDigest",
                "digest_mismatch",
                "model binding digest does not match its immutable provenance and selected model",
            ));
        }
        if self.model_id.chars().any(char::is_control) {
            issues.push(ValidationIssue::new(
                "modelId",
                "control_character",
                "provider model IDs must not contain control characters",
            ));
        }
        if self.schema_version != catalog.schema_version
            || self.catalog_snapshot_id != catalog.catalog_snapshot_id
            || self.catalog_digest != catalog.catalog_digest
            || self.provider_id != catalog.provider_id
            || self.acp_mode != catalog.acp_mode
            || self.provider_profile_id != catalog.provider_profile_id
            || self.profile_revision != catalog.profile_revision
            || self.adapter_id != catalog.adapter_id
            || self.adapter_version != catalog.adapter_version
            || self.adapter_digest != catalog.adapter_digest
            || self.artifact_set_digest != catalog.artifact_set_digest
        {
            issues.push(ValidationIssue::new(
                "modelBinding",
                "catalog_binding_mismatch",
                "model binding provenance must exactly match its catalog snapshot",
            ));
        }
        if !catalog
            .models
            .iter()
            .any(|model| model.model_id == self.model_id)
        {
            issues.push(ValidationIssue::new(
                "modelId",
                "model_not_in_catalog",
                "model ID must exactly match a row in the bound provider catalog",
            ));
        }
        if let Some(mode_id) = &self.mode_id {
            match &catalog.negotiated_modes {
                Some(modes) => modes.validate_selection(Some(mode_id))?,
                None => issues.push(ValidationIssue::new(
                    "modeId",
                    "mode_negotiation_missing",
                    "a selected mode requires observed catalog mode support",
                )),
            }
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveRunStatus {
    Queued,
    Claimed,
    SessionCreationIntent,
    Running,
    Paused,
    Cancelling,
    Unknown,
    Completed,
    Cancelled,
    Failed,
}

impl LiveRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::SessionCreationIntent => "session_creation_intent",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Cancelling => "cancelling",
            Self::Unknown => "unknown",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveRunEventKind {
    StatusChanged,
    TextDelta,
    SecurityViolation,
}

impl LiveRunEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StatusChanged => "status_changed",
            Self::TextDelta => "text_delta",
            Self::SecurityViolation => "security_violation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveProviderUsage {
    pub total_tokens: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub thought_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveProviderResult {
    pub final_text: String,
    pub stop_reason: String,
    pub usage: Option<LiveProviderUsage>,
    pub content_digest: Digest,
    pub content_byte_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthFailureProfileBinding {
    #[serde(deserialize_with = "deserialize_auth_failure_profile_id")]
    pub provider_profile_id: String,
    pub profile_revision: u64,
}

fn deserialize_auth_failure_profile_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.trim().is_empty() {
        return Err(serde::de::Error::custom(
            "authentication failure profile ID is blank",
        ));
    }
    Ok(value)
}

impl LiveRunFailure {
    pub fn validate_profile_binding(&self) -> bool {
        self.profile_binding.as_ref().is_none_or(|binding| {
            !binding.provider_profile_id.trim().is_empty()
                && is_profile_authentication_failure(&self.code)
        })
    }
}

pub fn is_profile_authentication_failure(code: &str) -> bool {
    matches!(
        code,
        "authentication_required"
            | "authentication_status_rpc_failed"
            | "authentication_unsupported"
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRunFailure {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_binding: Option<AuthFailureProfileBinding>,
    pub code: String,
    pub detail: String,
    pub external_effect_unknown: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveRunProviderOutcome {
    NotStarted,
    Pending,
    Confirmed,
    Unknown,
}

impl LiveRunProviderOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Pending => "pending",
            Self::Confirmed => "confirmed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRunCancellationSnapshot {
    pub requested_at: String,
    pub provider_outcome: LiveRunProviderOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRunEvent {
    pub sequence: u64,
    pub store_generation: u64,
    pub run_revision: u64,
    pub claim_generation: u64,
    pub kind: LiveRunEventKind,
    pub status: Option<LiveRunStatus>,
    pub text_delta: Option<String>,
    pub failure: Option<LiveRunFailure>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRunQueueProjection {
    pub admission_sequence: u64,
    pub state: LiveRunStatus,
    pub position: Option<u8>,
    pub admitted_count: u8,
    pub capacity: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRunEventCursor {
    pub store_id: String,
    pub store_generation: u64,
    pub run_id: OpaqueId,
    pub after_sequence: u64,
    pub high_water_sequence: u64,
    pub complete: bool,
}

pub fn live_run_failure_wire_version<'a>(
    failures: impl IntoIterator<Item = &'a LiveRunFailure>,
) -> Result<u16, &'static str> {
    let mut version = 1;
    for failure in failures {
        if !failure.validate_profile_binding() {
            return Err("invalid authentication failure profile binding");
        }
        if failure.profile_binding.is_some() {
            version = 2;
        }
    }
    Ok(version)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRunSnapshot {
    pub schema_version: u16,
    pub store_id: String,
    pub store_generation: u64,
    pub run_id: OpaqueId,
    pub question: String,
    pub status: LiveRunStatus,
    pub revision: u64,
    pub model_binding: AcpModelBindingSnapshot,
    pub queue: LiveRunQueueProjection,
    pub event_cursor: LiveRunEventCursor,
    pub events: Vec<LiveRunEvent>,
    pub cancellation: Option<LiveRunCancellationSnapshot>,
    pub result: Option<LiveProviderResult>,
    pub failure: Option<LiveRunFailure>,
    pub created_at: String,
    pub updated_at: String,
}

impl LiveRunSnapshot {
    pub fn validate_failure_wire_version(&self) -> bool {
        if self.status == LiveRunStatus::Paused
            || self
                .events
                .iter()
                .any(|event| event.status == Some(LiveRunStatus::Paused))
        {
            return self.schema_version == 3
                && live_run_failure_wire_version(
                    self.failure.iter().chain(
                        self.events
                            .iter()
                            .filter_map(|event| event.failure.as_ref()),
                    ),
                )
                .is_ok();
        }
        live_run_failure_wire_version(
            self.failure.iter().chain(
                self.events
                    .iter()
                    .filter_map(|event| event.failure.as_ref()),
            ),
        ) == Ok(self.schema_version)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthenticationMethod {
    LocalSubscription,
    ByokApi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialStore {
    File,
    Keychain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCredentialHome {
    pub authority_id: OpaqueId,
    pub canonical_path: String,
    pub device: u64,
    pub inode: u64,
    pub credential_store: ProviderCredentialStore,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_digest: Option<Digest>,
}

impl ProviderCredentialHome {
    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        check_id(
            "credential_home.authority_id",
            &self.authority_id,
            &mut issues,
        );
        let path = std::path::Path::new(&self.canonical_path);
        if self.canonical_path.len() > 4096
            || self.canonical_path.contains('\0')
            || !path.is_absolute()
            || self
                .canonical_path
                .split('/')
                .skip(1)
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            issues.push(ValidationIssue::new(
                "credential_home.canonical_path",
                "invalid_canonical_path",
                "credential home must be a bounded canonical absolute directory path",
            ));
        }
        if self.inode == 0 {
            issues.push(ValidationIssue::new(
                "credential_home.inode",
                "invalid_directory_identity",
                "credential home must pin an observed directory identity",
            ));
        }
        finish(issues)
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_home: Option<ProviderCredentialHome>,
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
        )?;
        validate_profile_credential_home(self.authentication_method, self.credential_home.as_ref())
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_home: Option<ProviderCredentialHome>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    credential_home: Option<&'a ProviderCredentialHome>,
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
            credential_home: input.credential_home,
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
            credential_home: self.credential_home.as_ref(),
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
        if let Err(DomainError::Validation(mut nested)) = validate_profile_credential_home(
            self.authentication_method,
            self.credential_home.as_ref(),
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

fn validate_profile_credential_home(
    method: ProviderAuthenticationMethod,
    home: Option<&ProviderCredentialHome>,
) -> Result<(), DomainError> {
    if let Some(home) = home {
        if method != ProviderAuthenticationMethod::LocalSubscription {
            return Err(DomainError::Validation(vec![ValidationIssue::new(
                "credential_home",
                "unexpected_credential_home",
                "only subscription profiles may reference a credential home",
            )]));
        }
        home.validate()?;
    }
    Ok(())
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
                catalog_binding: None,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_binding: Option<AcpModelBindingSnapshot>,
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
        if let Some(catalog_binding) = &self.catalog_binding
            && (catalog_binding.provider_profile_id != self.binding.provider_profile_id
                || catalog_binding.profile_revision != self.binding.revision
                || catalog_binding.adapter_id != self.binding.adapter_id
                || catalog_binding.adapter_version != self.binding.adapter_version
                || catalog_binding.adapter_digest != self.binding.adapter_digest
                || catalog_binding.model_id != self.binding.model_id)
        {
            issues.push(ValidationIssue::new(
                format!("{path}.catalog_binding"),
                "binding_mismatch",
                "catalog provenance must match the frozen role provider binding",
            ));
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSetSnapshot {
    pub schema_version: u16,
    pub role_set_id: OpaqueId,
    pub roles: [CoreRoleProfile; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_core_selections: Option<[FrozenCoreSelection; 3]>,
    pub digest: Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenCoreSelection {
    pub core_id: CoreId,
    pub provider_profile_id: String,
    pub profile_revision: u64,
    pub model_selection_revision: u64,
    pub core_selection_revision: u64,
}

#[derive(Serialize)]
struct RoleSetDigestDocument<'a> {
    schema_version: u16,
    role_set_id: &'a str,
    roles: &'a [CoreRoleProfile; 3],
    #[serde(skip_serializing_if = "Option::is_none")]
    frozen_core_selections: Option<&'a [FrozenCoreSelection; 3]>,
}

impl RoleSetSnapshot {
    pub fn new(role_set_id: OpaqueId, roles: [CoreRoleProfile; 3]) -> Result<Self, DomainError> {
        let mut snapshot = Self {
            schema_version: CONTRACT_SCHEMA_VERSION,
            role_set_id,
            roles,
            frozen_core_selections: None,
            digest: Digest::from_bytes(b""),
        };
        snapshot.digest = snapshot.calculate_digest()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn with_frozen_core_selections(
        mut self,
        selections: [FrozenCoreSelection; 3],
    ) -> Result<Self, DomainError> {
        self.schema_version = 2;
        self.frozen_core_selections = Some(selections);
        self.digest = self.calculate_digest()?;
        self.validate()?;
        Ok(self)
    }

    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = RoleSetDigestDocument {
            schema_version: self.schema_version,
            role_set_id: &self.role_set_id,
            roles: &self.roles,
            frozen_core_selections: self.frozen_core_selections.as_ref(),
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        check_id("role_set_id", &self.role_set_id, &mut issues);
        if !matches!(
            (self.schema_version, self.frozen_core_selections.is_some()),
            (1, false) | (2, true)
        ) {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                "role set version must match its frozen selection provenance",
            ));
        }
        for (index, role) in self.roles.iter().enumerate() {
            if let Some(selections) = &self.frozen_core_selections {
                let selection = &selections[index];
                if selection.core_id != role.core_id
                    || selection.provider_profile_id != role.binding.provider_profile_id
                    || selection.profile_revision != role.binding.revision
                    || role.catalog_binding.is_none()
                {
                    issues.push(ValidationIssue::new(
                        "frozen_core_selections",
                        "binding_mismatch",
                        "frozen selection provenance must match its canonical role",
                    ));
                }
            }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_provenance: Option<DeliberationRequestProvenance>,
    pub policy_digest: Digest,
    pub protocol_version: u16,
    pub input_digest: Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliberationRequestProvenance {
    pub context_draft_id: Option<String>,
    pub context_revision: Option<u64>,
    pub role_preset_id: String,
    pub role_revision: u64,
    pub disclosure_confirmed: bool,
}

#[derive(Serialize)]
struct InputDigestDocument<'a> {
    schema_version: u16,
    question_digest: &'a Digest,
    context_digest: &'a Digest,
    roles_digest: &'a Digest,
    policy_digest: &'a Digest,
    protocol_version: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_provenance: Option<&'a DeliberationRequestProvenance>,
}

impl InputSnapshot {
    pub fn new(
        question: QuestionSnapshot,
        context_manifest: ContextManifest,
        role_set: RoleSetSnapshot,
        policy_digest: Digest,
    ) -> Result<Self, DomainError> {
        let mut snapshot = Self {
            schema_version: 1,
            question,
            context_manifest,
            role_set,
            request_provenance: None,
            policy_digest,
            protocol_version: DELIBERATION_PROTOCOL_VERSION,
            input_digest: Digest::from_bytes(b""),
        };
        snapshot.input_digest = snapshot.calculate_digest()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn new_with_request_provenance(
        question: QuestionSnapshot,
        context_manifest: ContextManifest,
        role_set: RoleSetSnapshot,
        policy_digest: Digest,
        provenance: DeliberationRequestProvenance,
    ) -> Result<Self, DomainError> {
        let mut snapshot = Self {
            schema_version: 2,
            question,
            context_manifest,
            role_set,
            request_provenance: Some(provenance),
            policy_digest,
            protocol_version: DELIBERATION_PROTOCOL_VERSION,
            input_digest: Digest::from_bytes(b""),
        };
        snapshot.input_digest = snapshot.calculate_digest()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn with_request_provenance(
        mut self,
        provenance: DeliberationRequestProvenance,
    ) -> Result<Self, DomainError> {
        self.request_provenance = Some(provenance);
        self.input_digest = self.calculate_digest()?;
        self.validate()?;
        Ok(self)
    }

    pub fn calculate_digest(&self) -> Result<Digest, DomainError> {
        let document = InputDigestDocument {
            schema_version: self.schema_version,
            question_digest: &self.question.digest,
            context_digest: &self.context_manifest.digest,
            roles_digest: &self.role_set.digest,
            policy_digest: &self.policy_digest,
            protocol_version: self.protocol_version,
            request_provenance: self.request_provenance.as_ref(),
        };
        Ok(Digest::from_bytes(&canonical_json(&document)?))
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        if !matches!(
            (self.schema_version, self.request_provenance.is_some()),
            (1, false) | (2, true)
        ) {
            issues.push(ValidationIssue::new(
                "request_provenance",
                "invalid_version_provenance",
                "input version must match its complete request provenance",
            ));
        }
        if let Some(provenance) = &self.request_provenance {
            check_id(
                "request_provenance.rolePresetId",
                &provenance.role_preset_id,
                &mut issues,
            );
            if let Some(id) = &provenance.context_draft_id {
                check_id("request_provenance.contextDraftId", id, &mut issues);
            }
            if self.schema_version != 2
                || provenance.context_draft_id.is_some() != provenance.context_revision.is_some()
                || !provenance.disclosure_confirmed
                || self
                    .role_set
                    .roles
                    .iter()
                    .any(|role| role.revision != provenance.role_revision)
            {
                issues.push(ValidationIssue::new(
                    "request_provenance",
                    "invalid_admission_intent",
                    "admission intent must match the confirmed frozen input",
                ));
            }
        }
        if !matches!(self.schema_version, 1 | 2)
            || self.schema_version != self.role_set.schema_version
        {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                "input version must match its role set provenance version",
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub source_id: OpaqueId,
    pub object_digest: Digest,
    pub locator: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClaimResponseKind {
    Agree,
    Challenge,
    NeedsEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimResponse {
    pub target_claim_id: OpaqueId,
    pub response: ClaimResponseKind,
    pub rationale: String,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PositionChange {
    pub claim_id: OpaqueId,
    pub influenced_by_claim_ids: Vec<OpaqueId>,
    pub rationale: String,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InformationGap {
    pub missing_information: String,
    pub impact: String,
    pub essential: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[cfg(test)]
mod negotiated_mode_tests {
    use super::*;

    fn catalog() -> ProviderCatalogSnapshot {
        ProviderCatalogSnapshot::new(
            ProviderCatalogInput {
                catalog_snapshot_id: "catalog-fixture".into(),
                provider_id: "codex-acp".into(),
                provider_profile_id: "provider-fixture".into(),
                profile_revision: 1,
                adapter_id: "adapter-fixture".into(),
                adapter_version: "1".into(),
                adapter_digest: Digest::from_bytes(b"adapter-fixture"),
                fetched_at: "2026-10-01T00:00:00Z".into(),
            },
            vec![ProviderCatalogModel {
                model_id: "model-fixture".into(),
                name: None,
                description: None,
                context_window_tokens: None,
                max_output_tokens: None,
            }],
        )
        .unwrap()
    }

    #[test]
    fn catalog_execution_equivalence_covers_every_semantic_field() {
        let original = catalog()
            .with_negotiated_modes(NegotiatedModeState {
                current_mode_id: Some("agent".into()),
                modes: vec![ProviderCatalogMode {
                    mode_id: "agent".into(),
                    name: "Agent".into(),
                    description: None,
                }],
            })
            .unwrap()
            .with_artifact_set_digest(Digest::from_bytes(b"runtime-set"))
            .unwrap();
        let bytes = serde_json::to_vec(&original).unwrap();
        let mut fresh = original.clone();
        fresh.catalog_snapshot_id = "fresh-observation".into();
        fresh.fetched_at = "2026-10-02T00:00:00Z".into();
        fresh.catalog_digest = fresh.calculate_digest().unwrap();
        original.execution_equivalent_to(&fresh).unwrap();
        for index in 0..17 {
            let mut drift = fresh.clone();
            match index {
                0 => drift.provider_id = "other-provider".into(),
                1 => drift.provider_profile_id = "other-profile".into(),
                2 => drift.profile_revision += 1,
                3 => drift.adapter_id = "other-adapter".into(),
                4 => drift.adapter_version = "other-version".into(),
                5 => drift.adapter_digest = Digest::from_bytes(b"other-adapter"),
                6 => drift.artifact_set_digest = Some(Digest::from_bytes(b"other-set")),
                7 => drift.models[0].model_id = "other-model".into(),
                8 => drift.models[0].name = Some("Other name".into()),
                9 => drift.models[0].description = Some("Other description".into()),
                10 => drift.models[0].context_window_tokens = Some(1234),
                11 => drift.models[0].max_output_tokens = Some(100),
                12 => drift.negotiated_modes.as_mut().unwrap().modes[0].name = "Other mode".into(),
                13 => {
                    drift.negotiated_modes.as_mut().unwrap().modes[0].description =
                        Some("Other mode description".into())
                }
                14 => {
                    let modes = drift.negotiated_modes.as_mut().unwrap();
                    modes.modes.push(ProviderCatalogMode {
                        mode_id: "other".into(),
                        name: "Other".into(),
                        description: None,
                    });
                    modes.current_mode_id = Some("other".into());
                }
                15 => {
                    let modes = drift.negotiated_modes.as_mut().unwrap();
                    modes.current_mode_id = Some("other".into());
                    modes.modes[0].mode_id = "other".into();
                }
                _ => {
                    drift.negotiated_modes.as_mut().unwrap().current_mode_id = None;
                    drift.negotiated_modes.as_mut().unwrap().modes.clear();
                }
            }
            drift.catalog_digest = drift.calculate_digest().unwrap();
            drift.validate().unwrap();
            assert!(
                original.execution_equivalent_to(&drift).is_err(),
                "semantic field {index}"
            );
        }
        let mut corrupt = fresh.clone();
        corrupt.catalog_digest = Digest::from_bytes(b"corrupt");
        assert!(original.execution_equivalent_to(&corrupt).is_err());
        let legacy = catalog();
        assert!(legacy.execution_equivalent_to(&legacy).is_err());
        assert_eq!(serde_json::to_vec(&original).unwrap(), bytes);
    }

    #[test]
    fn artifact_set_authority_preserves_history_and_is_required_only_for_execution() {
        let historical = catalog()
            .with_negotiated_modes(NegotiatedModeState {
                current_mode_id: None,
                modes: vec![],
            })
            .unwrap();
        let old =
            AcpModelBindingSnapshot::from_catalog_with_mode(&historical, "model-fixture", None)
                .unwrap();
        old.validate_ready(&historical).unwrap();
        assert!(old.validate_for_execution(&historical).is_err());
        for bytes in [
            serde_json::to_vec(&historical).unwrap(),
            serde_json::to_vec(&old).unwrap(),
        ] {
            assert!(
                !String::from_utf8(bytes)
                    .unwrap()
                    .contains("artifactSetDigest")
            );
        }
        let legacy_bytes = serde_json::to_vec(&historical).unwrap();
        let decoded: ProviderCatalogSnapshot = serde_json::from_slice(&legacy_bytes).unwrap();
        decoded.validate().unwrap();
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), legacy_bytes);
        let current = historical
            .clone()
            .with_artifact_set_digest(Digest::from_bytes(b"verified-runtime-set"))
            .unwrap();
        let binding =
            AcpModelBindingSnapshot::from_catalog_with_mode(&current, "model-fixture", None)
                .unwrap();
        assert_eq!(current.schema_version, 3);
        assert_eq!(binding.schema_version, 3);
        binding.validate_for_execution(&current).unwrap();
        let mut old_version = current.clone();
        old_version.schema_version = 2;
        old_version.catalog_digest = old_version.calculate_digest().unwrap();
        assert!(old_version.validate().is_err());
        let mut missing = current.clone();
        missing.artifact_set_digest = None;
        missing.catalog_digest = missing.calculate_digest().unwrap();
        assert!(missing.validate().is_err());
        for version in [1, 2, 3] {
            let mut raw = serde_json::to_value(&current).unwrap();
            raw["schemaVersion"] = serde_json::json!(version);
            raw["artifactSetDigest"] = serde_json::Value::Null;
            assert!(serde_json::from_value::<ProviderCatalogSnapshot>(raw).is_err());
            let mut raw_binding = serde_json::to_value(&binding).unwrap();
            raw_binding["schemaVersion"] = serde_json::json!(version);
            raw_binding["artifactSetDigest"] = serde_json::Value::Null;
            assert!(serde_json::from_value::<AcpModelBindingSnapshot>(raw_binding).is_err());
        }
        let mut missing_modes = current.clone();
        missing_modes.negotiated_modes = None;
        missing_modes.catalog_digest = missing_modes.calculate_digest().unwrap();
        assert!(missing_modes.validate().is_err());
        assert!(
            catalog()
                .with_artifact_set_digest(Digest::from_bytes(b"runtime"))
                .is_err()
        );
        assert_eq!(binding.adapter_digest, old.adapter_digest);
        assert_ne!(binding.binding_digest, old.binding_digest);
        assert_ne!(current.catalog_digest, historical.catalog_digest);
        let mut changed = binding.clone();
        changed.artifact_set_digest = Some(Digest::from_bytes(b"other-runtime-set"));
        changed.binding_digest = changed.calculate_digest().unwrap();
        assert!(changed.validate_for_execution(&current).is_err());
        let mut changed_catalog = current;
        changed_catalog.artifact_set_digest = Some(Digest::from_bytes(b"other-runtime-set"));
        assert!(changed_catalog.validate().is_err());
    }

    #[test]
    fn actual_absence_is_distinct_from_unknown_and_modes_require_explicit_selection() {
        let legacy = catalog();
        assert!(
            AcpModelBindingSnapshot::from_catalog_with_mode(&legacy, "model-fixture", None)
                .is_err()
        );
        let absent = legacy
            .clone()
            .with_negotiated_modes(NegotiatedModeState {
                current_mode_id: None,
                modes: vec![],
            })
            .unwrap();
        assert_ne!(legacy.catalog_digest, absent.catalog_digest);
        assert_eq!(legacy.schema_version, 1);
        assert_eq!(absent.schema_version, 2);
        let mut invalid = absent.clone();
        invalid.schema_version = 1;
        invalid.catalog_digest = invalid.calculate_digest().unwrap();
        assert!(invalid.validate().is_err());
        let mut invalid = legacy.clone();
        invalid.schema_version = 2;
        invalid.catalog_digest = invalid.calculate_digest().unwrap();
        assert!(invalid.validate().is_err());
        let mut mismatched =
            AcpModelBindingSnapshot::from_catalog_with_mode(&absent, "model-fixture", None)
                .unwrap();
        assert_eq!(mismatched.schema_version, 2);
        mismatched.schema_version = 1;
        mismatched.binding_digest = mismatched.calculate_digest().unwrap();
        assert!(mismatched.validate(&absent).is_err());
        AcpModelBindingSnapshot::from_catalog_with_mode(&absent, "model-fixture", None)
            .unwrap()
            .validate_ready(&absent)
            .unwrap();
        assert!(
            AcpModelBindingSnapshot::from_catalog_with_mode(
                &absent,
                "model-fixture",
                Some("default")
            )
            .is_err()
        );
        let modes = legacy
            .with_negotiated_modes(NegotiatedModeState {
                current_mode_id: Some("review".into()),
                modes: ["review", "plan"]
                    .map(|id| ProviderCatalogMode {
                        mode_id: id.into(),
                        name: id.into(),
                        description: None,
                    })
                    .into(),
            })
            .unwrap();
        assert!(
            AcpModelBindingSnapshot::from_catalog_with_mode(&modes, "model-fixture", None).is_err()
        );
        assert!(
            AcpModelBindingSnapshot::from_catalog_with_mode(
                &modes,
                "model-fixture",
                Some("unknown")
            )
            .is_err()
        );
        let review = AcpModelBindingSnapshot::from_catalog_with_mode(
            &modes,
            "model-fixture",
            Some("review"),
        )
        .unwrap();
        let plan =
            AcpModelBindingSnapshot::from_catalog_with_mode(&modes, "model-fixture", Some("plan"))
                .unwrap();
        assert_ne!(review.binding_digest, plan.binding_digest);
        assert!(
            catalog()
                .with_negotiated_modes(NegotiatedModeState {
                    current_mode_id: Some("missing".into()),
                    modes: vec![]
                })
                .is_err()
        );
    }

    #[test]
    fn legacy_input_full_serialization_and_nested_digests_round_trip_without_mode_fields() {
        let catalog = catalog();
        let binding = AcpModelBindingSnapshot::from_catalog(&catalog, "model-fixture").unwrap();
        let roles = CoreId::ALL.map(|core_id| CoreRoleProfile {
            core_id,
            profile_id: format!("role-{}", core_id.wire_name()),
            revision: 1,
            display_name: core_id.wire_name().into(),
            review_purpose: "Review evidence".into(),
            evaluation_criteria: vec!["Evidence".into()],
            falsification_questions: vec!["What disproves it?".into()],
            response_language: "en".into(),
            binding: ModelBindingSnapshot {
                provider_profile_id: binding.provider_profile_id.clone(),
                revision: binding.profile_revision,
                adapter_id: binding.adapter_id.clone(),
                adapter_version: binding.adapter_version.clone(),
                adapter_digest: binding.adapter_digest.clone(),
                model_id: binding.model_id.clone(),
                context_window_tokens: None,
                maximum_output_tokens: None,
            },
            catalog_binding: Some(binding.clone()),
        });
        let input = InputSnapshot::new(
            QuestionSnapshot::new(
                "question-fixture".into(),
                QuestionKind::Answer,
                "Evaluate evidence".into(),
                vec![],
                vec![],
                vec![],
            )
            .unwrap(),
            ContextManifest::new("manifest-fixture".into(), vec![]).unwrap(),
            RoleSetSnapshot::new("roles-fixture".into(), roles).unwrap(),
            Digest::from_bytes(b"policy"),
        )
        .unwrap();
        let encoded = serde_json::to_string(&input).unwrap();
        assert!(!encoded.contains("modeId"));
        assert!(
            !serde_json::to_string(&catalog)
                .unwrap()
                .contains("negotiatedModes")
        );
        let restored: InputSnapshot = serde_json::from_str(&encoded).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.input_digest, input.input_digest);
        assert_eq!(restored.role_set.digest, input.role_set.digest);
        assert_eq!(serde_json::to_string(&restored).unwrap(), encoded);
        assert!(!encoded.contains("frozen_core_selections"));
        let selections = CoreId::ALL.map(|core_id| FrozenCoreSelection {
            core_id,
            provider_profile_id: binding.provider_profile_id.clone(),
            profile_revision: binding.profile_revision,
            model_selection_revision: 0,
            core_selection_revision: 0,
        });
        let roles = input
            .role_set
            .clone()
            .with_frozen_core_selections(selections.clone())
            .unwrap();
        assert!(
            InputSnapshot::new(
                input.question.clone(),
                input.context_manifest.clone(),
                roles.clone(),
                input.policy_digest.clone(),
            )
            .is_err()
        );
        let frozen = InputSnapshot::new_with_request_provenance(
            input.question.clone(),
            input.context_manifest.clone(),
            roles.clone(),
            input.policy_digest.clone(),
            DeliberationRequestProvenance {
                context_draft_id: None,
                context_revision: None,
                role_preset_id: "preset".into(),
                role_revision: input.role_set.roles[0].revision,
                disclosure_confirmed: true,
            },
        )
        .unwrap();
        assert_eq!(frozen.schema_version, 2);
        let mut missing_intent = frozen.clone();
        missing_intent.request_provenance = None;
        missing_intent.input_digest = missing_intent.calculate_digest().unwrap();
        assert!(missing_intent.validate().is_err());
        for provenance in [
            DeliberationRequestProvenance {
                context_draft_id: Some("draft".into()),
                context_revision: None,
                ..frozen.request_provenance.clone().unwrap()
            },
            DeliberationRequestProvenance {
                disclosure_confirmed: false,
                ..frozen.request_provenance.clone().unwrap()
            },
            DeliberationRequestProvenance {
                role_revision: input.role_set.roles[0].revision + 1,
                ..frozen.request_provenance.clone().unwrap()
            },
        ] {
            assert!(
                InputSnapshot::new_with_request_provenance(
                    input.question.clone(),
                    input.context_manifest.clone(),
                    roles.clone(),
                    input.policy_digest.clone(),
                    provenance,
                )
                .is_err()
            );
        }
        let mut different = selections;
        different[2].core_selection_revision = 1;
        let changed_roles = roles
            .clone()
            .with_frozen_core_selections(different)
            .unwrap();
        assert_ne!(changed_roles.digest, roles.digest);
        let mut wrong_roles = roles;
        wrong_roles.schema_version = 1;
        wrong_roles.digest = wrong_roles.calculate_digest().unwrap();
        assert!(wrong_roles.validate().is_err());
        let mut wrong_input = frozen;
        wrong_input.schema_version = 1;
        wrong_input.input_digest = wrong_input.calculate_digest().unwrap();
        assert!(wrong_input.validate().is_err());
        assert!(binding.validate_ready(&catalog).is_err());
    }
}

#[cfg(test)]
mod credential_authority_tests {
    use super::*;

    #[test]
    fn legacy_profile_digest_is_preserved_and_authority_changes_are_detected() {
        let input = ProviderProfileInput {
            provider_profile_id: "profile-fixture".into(),
            provider_id: "codex-acp".into(),
            display_name: "Fixture".into(),
            account_alias: "Fixture".into(),
            authentication_method: ProviderAuthenticationMethod::LocalSubscription,
            secret_reference: None,
            runtime_home_id: "runtime-fixture".into(),
            credential_home: None,
        };
        let legacy = ProviderProfileRevision::new(input, 0).unwrap();
        let encoded = serde_json::to_value(&legacy).unwrap();
        assert!(encoded.get("credential_home").is_none());
        let restored: ProviderProfileRevision = serde_json::from_value(encoded).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.digest, legacy.digest);
        let mut bound = legacy.clone();
        bound.credential_home = Some(ProviderCredentialHome {
            authority_id: "authority-fixture".into(),
            canonical_path: "/Users/fixture/.codex".into(),
            device: 1,
            inode: 2,
            credential_store: ProviderCredentialStore::File,
            account_digest: Some(Digest::from_bytes(b"fixture-account")),
        });
        assert_ne!(bound.calculate_digest().unwrap(), legacy.digest);
        assert!(bound.validate().is_err());
        bound.digest = bound.calculate_digest().unwrap();
        bound.validate().unwrap();
        bound.credential_home.as_mut().unwrap().inode = 3;
        assert!(bound.validate().is_err());
    }
}

#[cfg(test)]
mod authentication_failure_contract_tests {
    use super::*;
    #[test]
    fn legacy_failure_bytes_and_authentication_binding_are_closed() {
        let legacy =
            r#"{"code":"provider_timeout","detail":"timeout","externalEffectUnknown":true}"#;
        let failure: LiveRunFailure = serde_json::from_str(legacy).unwrap();
        assert_eq!(serde_json::to_string(&failure).unwrap(), legacy);
        assert_eq!(live_run_failure_wire_version([&failure]), Ok(1));
        let bound = LiveRunFailure {
            code: "authentication_status_rpc_failed".into(),
            detail: "verification unavailable".into(),
            external_effect_unknown: false,
            profile_binding: Some(AuthFailureProfileBinding {
                provider_profile_id: "frozen-profile".into(),
                profile_revision: 7,
            }),
        };
        assert_eq!(live_run_failure_wire_version([&failure, &bound]), Ok(2));
        let mut generic = bound.clone();
        generic.code = "provider_timeout".into();
        assert!(live_run_failure_wire_version([&generic]).is_err());
        for bad in [
            r#"{"providerProfileId":" ","profileRevision":1}"#,
            r#"{"providerProfileId":"p","profileRevision":-1}"#,
            r#"{"providerProfileId":"p","profileRevision":1,"canonicalPath":"private"}"#,
        ] {
            assert!(serde_json::from_str::<AuthFailureProfileBinding>(bad).is_err());
        }
    }
}

#[cfg(test)]
mod turn_schema_tests {
    use super::*;

    fn schema<T: schemars::JsonSchema>() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(T)).unwrap()
    }

    #[test]
    fn generated_turn_types_preserve_closed_serde_wire_contracts() {
        let claim = schema::<ClaimKind>();
        assert_eq!(
            claim["enum"],
            serde_json::json!([
                "source_fact",
                "model_knowledge",
                "inference",
                "preference",
                "assumption"
            ])
        );
        for value in claim["enum"].as_array().unwrap() {
            let typed: ClaimKind = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(typed).unwrap(), *value);
        }
        assert!(serde_json::from_value::<ClaimKind>(serde_json::json!("unsupported")).is_err());
        let response = schema::<ClaimResponse>();
        assert_eq!(response["additionalProperties"], false);
        assert_eq!(response["properties"]["evidence_refs"]["type"], "array");
        assert_eq!(
            response["required"],
            serde_json::json!(["rationale", "response", "target_claim_id"])
        );
        assert_eq!(
            response["definitions"]["ClaimResponseKind"]["enum"],
            serde_json::json!(["agree", "challenge", "needs_evidence"])
        );
        let value = serde_json::json!({"target_claim_id":"claim-1", "response":"challenge", "rationale":"Insufficient evidence"});
        assert!(serde_json::from_value::<ClaimResponse>(value.clone()).is_ok());
        for field in ["target_claim_id", "response", "rationale"] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<ClaimResponse>(missing).is_err());
        }
        let mut extra = value.clone();
        extra["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ClaimResponse>(extra).is_err());
        let mut wrong = value;
        wrong["evidence_refs"] = serde_json::json!("not an array");
        assert!(serde_json::from_value::<ClaimResponse>(wrong).is_err());
        let counterargument = schema::<Counterargument>();
        assert_eq!(counterargument["additionalProperties"], false);
        assert_eq!(
            counterargument["properties"]["target_claim_id"]["type"],
            serde_json::json!(["string", "null"])
        );
        for value in [
            serde_json::json!({"rationale":"Review the assumption"}),
            serde_json::json!({"target_claim_id":null,"rationale":"Review the assumption"}),
        ] {
            assert!(serde_json::from_value::<Counterargument>(value).is_ok());
        }
        let gap = schema::<InformationGap>();
        assert_eq!(gap["additionalProperties"], false);
        assert_eq!(gap["properties"]["essential"]["type"], "boolean");
        let change = schema::<PositionChange>();
        assert_eq!(
            change["properties"]["influenced_by_claim_ids"]["type"],
            "array"
        );
        let evidence = schema::<EvidenceRef>();
        assert_eq!(evidence["additionalProperties"], false);
        assert_eq!(evidence["properties"]["object_digest"]["type"], "string");
        assert_eq!(
            schema::<crate::VoteValue>()["enum"],
            serde_json::json!(["support", "oppose", "abstain"])
        );
        let digest = Digest::from_bytes(b"schema wire control");
        assert_eq!(serde_json::to_value(&digest).unwrap(), digest.as_str());
        assert_eq!(schema::<Digest>()["type"], "string");
    }
}
