use magi_domain::ModelBindingSnapshot;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlotBudget {
    pub text_token_upper_bound: u64,
    pub image_count: u64,
    pub vision_token_upper_bound: Option<u64>,
    pub prior_input_reserve_token_upper_bound: u64,
    pub future_artifact_reinput_byte_budget: u64,
    pub future_artifact_count: u64,
    pub output_reserve_tokens: u64,
    pub context_limit_tokens: u64,
    pub requires_provider_token_attestation: bool,
    pub output_request_target_tokens: u64,
    pub provider_enforced_output_limit_tokens: Option<u64>,
    pub accepted_public_output_token_limit_tokens: u64,
    pub parser_byte_limit: u64,
    pub text_budget_basis: &'static str,
}

/// The caller supplies the exact serialized provider text, excluding image base64.
/// Vision bounds require evidence for this binding and adapter's effective detail.
/// UTF-8 byte length is a conservative text bound, never an exact token count.
pub(crate) fn evaluate_slot(
    binding: &ModelBindingSnapshot,
    wire_text: &str,
    image_count: u64,
    verified_vision_token_upper_bound: Option<u64>,
    future_artifact_count: u64,
) -> Result<SlotBudget, String> {
    let context_limit_tokens = u64::from(
        binding
            .context_window_tokens
            .ok_or("The frozen binding has no verified context limit.")?,
    );
    let maximum_output = u64::from(
        binding
            .maximum_output_tokens
            .ok_or("The frozen binding has no verified output limit.")?,
    );
    if maximum_output < 8192 {
        return Err("The frozen model cannot reserve the accepted 8192 output tokens.".into());
    }
    let vision = if image_count == 0 {
        Some(0)
    } else {
        verified_vision_token_upper_bound
    };
    let vision_bound =
        vision.ok_or("This subscription binding has no verified image token budget.")?;
    let text_bound =
        u64::try_from(wire_text.len()).map_err(|_| "The prompt size is unsupported.")?;
    let current = text_bound
        .checked_add(vision_bound)
        .and_then(|v| v.checked_add(8192))
        .ok_or("The input budget overflowed.")?;
    let remaining = context_limit_tokens
        .checked_sub(current)
        .ok_or("The actual serialized prompt exceeds the verified context budget.")?;
    let future_bytes = if future_artifact_count == 0 {
        0
    } else {
        (remaining / future_artifact_count).min(256 * 1024)
    };
    Ok(SlotBudget {
        text_token_upper_bound: text_bound,
        image_count,
        vision_token_upper_bound: vision,
        prior_input_reserve_token_upper_bound: future_artifact_count.saturating_mul(future_bytes),
        future_artifact_reinput_byte_budget: future_bytes,
        future_artifact_count,
        output_reserve_tokens: 8192,
        context_limit_tokens,
        requires_provider_token_attestation: false,
        output_request_target_tokens: 4096,
        provider_enforced_output_limit_tokens: None,
        accepted_public_output_token_limit_tokens: 8192,
        parser_byte_limit: 256 * 1024,
        text_budget_basis: "utf8_byte_upper_bound",
    })
}

pub(crate) fn validate_common_context_with_limit(
    structured_text: &str,
    image_count: u64,
    verified_vision_bound: Option<u64>,
    frozen_token_limit: u32,
) -> Result<(), String> {
    if !(1..=128_000).contains(&frozen_token_limit) {
        return Err("The frozen common context limit is invalid.".into());
    }
    let vision = if image_count == 0 {
        0
    } else {
        verified_vision_bound.ok_or("The common image context has no verified token bound.")?
    };
    let bound = (structured_text.len() as u64)
        .checked_add(vision)
        .ok_or("The common context budget overflowed.")?;
    if bound > u64::from(frozen_token_limit) {
        return Err("The common question and approved sources exceed the frozen common context conservative bound.".into());
    }
    Ok(())
}

fn pinned_model_slug(model_id: &str) -> Option<&str> {
    match model_id.split_once('[') {
        None => Some(model_id),
        Some((slug, "low]" | "medium]" | "high]" | "xhigh]")) => Some(slug),
        _ => None,
    }
}

/// Codex rust-v0.156.1 models-manager/models.json declares 272000 for these exact slugs.
/// protocol/openai_models.rs usable_context_window applies the default 95% headroom.
/// The 128000 output figure is model capability, not an ACP request enforcement claim.
/// https://developers.openai.com/api/docs/models/gpt-5.5
pub(crate) fn pinned_model_limits(model_id: &str) -> Option<(u64, u64)> {
    match pinned_model_slug(model_id)? {
        "gpt-5.5" | "gpt-5.4" => Some((258400, 128000)),
        _ => None,
    }
}

/// Only non-lite pinned models retain High detail after image preparation.
/// With unified_image_budget disabled, 2500 patches * 1.2 plus rounding margin is an upper bound.
/// https://developers.openai.com/api/docs/guides/images-vision
pub(crate) fn high_detail_vision_bound(
    binding: &ModelBindingSnapshot,
    image_count: u64,
) -> Option<u64> {
    if image_count == 0 {
        return Some(0);
    }
    if binding.adapter_id != "codex-acp"
        || binding.adapter_version != magi_provider::CODEX_ACP_VERSION
    {
        return None;
    }
    match pinned_model_slug(&binding.model_id)? {
        "gpt-5.5" | "gpt-5.4" => image_count.checked_mul(3001),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> ModelBindingSnapshot {
        ModelBindingSnapshot {
            provider_profile_id: "profile".into(),
            revision: 1,
            adapter_id: "codex-acp".into(),
            adapter_version: "1.13.1".into(),
            adapter_digest: magi_domain::Digest::from_bytes(b"fixture"),
            model_id: "fixture-model".into(),
            context_window_tokens: Some(64000),
            maximum_output_tokens: Some(8192),
        }
    }
    #[test]
    fn actual_text_budget_preserves_large_valid_output_reinput_and_blocks_unknown_vision() {
        let model = binding();
        let artifact = "a".repeat(12 * 1024);
        let budget = evaluate_slot(&model, &artifact, 0, None, 3).unwrap();
        assert_eq!(budget.text_token_upper_bound, 12288);
        assert_eq!(budget.output_reserve_tokens, 8192);
        assert!(budget.future_artifact_reinput_byte_budget > 12288);
        assert!(evaluate_slot(&model, &artifact, 1, None, 3).is_err());
        assert!(evaluate_slot(&model, &"a".repeat(64000), 0, None, 0).is_err());
        let mut unknown = model;
        unknown.context_window_tokens = None;
        assert!(evaluate_slot(&unknown, "small", 0, None, 0).is_err());
    }
}

#[cfg(test)]
mod common_context_tests {
    use super::*;
    #[test]
    fn configured_common_cap_preserves_verified_vision_and_model_output_reserves() {
        assert!(validate_common_context_with_limit(&"a".repeat(128_000), 0, None, 128_000).is_ok());
        assert!(
            validate_common_context_with_limit(&"a".repeat(128_001), 0, None, 128_000).is_err()
        );
        assert!(
            validate_common_context_with_limit(&"a".repeat(127_999), 1, Some(1), 128_000).is_ok()
        );
        assert!(
            validate_common_context_with_limit(&"a".repeat(128_000), 1, Some(1), 128_000).is_err()
        );
        assert!(validate_common_context_with_limit("a", 1, None, 128_000).is_err());
        for cap in [0, 128_001, u32::MAX] {
            assert!(validate_common_context_with_limit("", 0, None, cap).is_err());
        }
        assert!(validate_common_context_with_limit("a", 0, None, 1).is_ok());
        let model = ModelBindingSnapshot {
            provider_profile_id: "profile".into(),
            revision: 1,
            adapter_id: "codex-acp".into(),
            adapter_version: "1.13.1".into(),
            adapter_digest: magi_domain::Digest::from_bytes(b"fixture"),
            model_id: "fixture-model".into(),
            context_window_tokens: Some(64_000),
            maximum_output_tokens: Some(8192),
        };
        let common = "a".repeat(60_000);
        assert!(validate_common_context_with_limit(&common, 0, None, 128_000).is_ok());
        assert!(evaluate_slot(&model, &common, 0, None, 0).is_err());
        assert!(evaluate_slot(&model, &"a".repeat(64_000 - 8192), 0, None, 0).is_ok());
    }

    #[test]
    fn common_context_default_is_independent_of_model_capacity() {
        assert!(validate_common_context_with_limit("", 0, None, 32_000).is_ok());
        assert!(validate_common_context_with_limit(&"a".repeat(32_000), 0, None, 32_000).is_ok());
        assert!(validate_common_context_with_limit(&"a".repeat(32_001), 0, None, 32_000).is_err());
        assert!(
            validate_common_context_with_limit(&"a".repeat(31_999), 1, Some(1), 32_000).is_ok()
        );
        assert!(
            validate_common_context_with_limit(&"a".repeat(32_000), 1, Some(1), 32_000).is_err()
        );
        assert!(validate_common_context_with_limit("small", 1, None, 32_000).is_err());
    }
}
