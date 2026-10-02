use super::*;
use magi_context::{EvidenceLocator, ManifestSource, RepresentationKind};

pub(super) fn render(
    s: &ManifestSource,
    locator: &EvidenceLocator,
    freshness: FreshnessStatus,
    read: impl Fn(&Digest) -> Result<Vec<u8>, StorageError>,
) -> Result<EvidenceView, StorageError> {
    if s.source_id != locator.source_id
        || s.state != ManifestSourceState::Captured
        || s.object_digest.as_ref() != Some(&locator.object_digest)
    {
        return Err(StorageError::Corrupt(
            "source is not a captured snapshot".into(),
        ));
    }
    if s.representation_kind == Some(RepresentationKind::Utf8Text)
        && (locator.page.is_some() || locator.width.is_some() || locator.height.is_some())
    {
        return Err(StorageError::Corrupt(
            "text source cannot use a binary locator".into(),
        ));
    }
    if !matches!(
        s.representation_kind,
        Some(magi_context::RepresentationKind::Utf8Text)
    ) {
        if !s.included_locators.contains(locator) {
            return Err(StorageError::Corrupt(
                "binary locator outside transmitted snapshot".into(),
            ));
        }
        let mut view = EvidenceView {
            source_id: s.source_id.clone(),
            object_digest: locator.object_digest.clone(),
            start_line: 0,
            end_line: 0,
            total_lines: 0,
            text: None,
            evidence_unavailable: false,
            freshness,
            representation_kind: s.representation_kind,
            page: locator.page,
            width: locator.width,
            height: locator.height,
            data_url: None,
            warnings: Vec::new(),
        };
        match read(&locator.object_digest) {
            Ok(_) => {}
            Err(StorageError::MissingObject(_)) => {
                view.evidence_unavailable = true;
                return Ok(view);
            }
            Err(e) => return Err(e),
        }
        let derived = s
            .derived_digest
            .as_ref()
            .ok_or_else(|| StorageError::Corrupt("missing binary representation digest".into()))?;
        let extracted: magi_context::NativeExtraction = match read(derived) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(StorageError::MissingObject(_)) => {
                view.evidence_unavailable = true;
                return Ok(view);
            }
            Err(e) => return Err(e),
        };
        if extracted.schema_version != 1 || Some(extracted.kind) != s.representation_kind {
            return Err(StorageError::Corrupt(
                "captured representation identity differs".into(),
            ));
        }
        view.warnings = extracted.warnings;
        if let Some(page) = locator.page {
            let p = extracted
                .pages
                .into_iter()
                .find(|p| p.page == page)
                .ok_or_else(|| StorageError::Corrupt("captured page not found".into()))?;
            view.text = p.text;
            view.width = p.width;
            view.height = p.height;
            view.data_url = p
                .image_base64
                .zip(p.mime_type)
                .map(|(data, mime)| format!("data:{mime};base64,{data}"));
        } else {
            view.data_url = extracted
                .image_base64
                .map(|data| format!("data:{};base64,{data}", extracted.mime_type));
        }
        if view.width != locator.width
            || view.height != locator.height
            || view
                .text
                .as_ref()
                .is_some_and(|text| text.len() > 1024 * 1024)
            || view
                .data_url
                .as_ref()
                .is_some_and(|data| !valid_png_data_url(data))
        {
            return Err(StorageError::Corrupt(
                "captured representation differs from selected locator".into(),
            ));
        }
        return Ok(view);
    }
    if s.object_digest.as_ref()!=Some(&locator.object_digest) || !s.included_locators.iter().any(|allowed|{
            if allowed.source_id!=locator.source_id || allowed.object_digest!=locator.object_digest{return false;}
            if allowed.start_line.is_none() && allowed.end_line.is_none() && allowed.total_lines.is_none(){return true;}
            allowed.total_lines==locator.total_lines && matches!((allowed.start_line,allowed.end_line,locator.start_line,locator.end_line),(Some(a),Some(b),Some(c),Some(d)) if a<=c && c<=d && d<=b)
        }) {return Err(StorageError::Corrupt("locator outside transmitted snapshot range".into()));}
    let mut start = locator.start_line.unwrap_or(1);
    let mut end = locator.end_line.unwrap_or(0);
    let mut total = locator.total_lines.unwrap_or(0);
    let text = match read(&locator.object_digest) {
        Ok(b) => {
            let t = String::from_utf8(b).map_err(|_| StorageError::Corrupt("not UTF-8".into()))?;
            let lines: Vec<_> = t.lines().collect();
            if locator.total_lines.is_none() {
                total = lines.len() as u64;
            }
            if locator.end_line.is_none() {
                end = total;
            }
            if total == 0 && locator.start_line.is_none() {
                start = 0;
            }
            if (start == 0 && total != 0) || start > end || end > total {
                return Err(StorageError::Corrupt("invalid evidence range".into()));
            }
            if lines.len() as u64 != total {
                return Err(StorageError::Integrity("line count mismatch".into()));
            }
            Some(if total == 0 {
                String::new()
            } else {
                lines[(start - 1) as usize..end as usize].join("\n")
            })
        }
        Err(StorageError::MissingObject(_)) => None,
        Err(e) => return Err(e),
    };
    Ok(EvidenceView {
        source_id: s.source_id.clone(),
        object_digest: locator.object_digest.clone(),
        start_line: start,
        end_line: end,
        total_lines: total,
        evidence_unavailable: text.is_none(),
        text,
        freshness,
        representation_kind: s.representation_kind,
        page: None,
        width: None,
        height: None,
        data_url: None,
        warnings: Vec::new(),
    })
}

fn valid_png_data_url(data: &str) -> bool {
    let Some(encoded) = data.strip_prefix("data:image/png;base64,") else {
        return false;
    };
    if encoded.is_empty() || encoded.len() > 140 * 1024 * 1024 || encoded.len() % 4 != 0 {
        return false;
    }
    let unpadded = encoded.trim_end_matches('=');
    encoded.len() - unpadded.len() <= 2
        && unpadded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draft_preview_is_revision_bound_and_exposes_only_selected_range() {
        let root = std::env::temp_dir().join(format!("magi-preview-test-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let original = storage
            .put_source_object(b"PRIVATE_BEFORE\nPUBLIC_SELECTED\nPRIVATE_AFTER\n")
            .unwrap();
        let derived = storage.put_source_object(b"PUBLIC_SELECTED\n").unwrap();
        let locator = EvidenceLocator {
            source_id: "memo".into(),
            object_digest: original.digest.clone(),
            start_line: Some(2),
            end_line: Some(2),
            total_lines: Some(3),
            page: None,
            width: None,
            height: None,
        };
        let source = ManifestSource {
            source_id: "memo".into(),
            display_name: "memo.txt".into(),
            state: ManifestSourceState::Captured,
            byte_length: Some(original.byte_length),
            mime_type: Some("text/plain".into()),
            object_digest: Some(original.digest),
            derived_digest: Some(derived.digest),
            representation_kind: Some(RepresentationKind::Utf8Text),
            extractor_id: Some("utf8-text".into()),
            extractor_version: Some("1".into()),
            included_locators: vec![locator.clone()],
            omission: None,
            captured_at_epoch_ms: Some(1),
            secret_pattern_findings: vec![],
            secret_scan_incomplete: false,
        };
        let manifest = SourceCaptureManifest::draft(vec![source], 1).unwrap();
        let draft = storage
            .save_context_draft("preview-draft", None, &manifest, 1)
            .unwrap();
        let view = storage
            .load_context_evidence("preview-draft", draft.revision, &locator)
            .unwrap();
        assert_eq!(view.text.as_deref(), Some("PUBLIC_SELECTED"));
        assert_eq!(view.freshness, FreshnessStatus::Unchecked);
        assert_eq!(view.representation_kind, Some(RepresentationKind::Utf8Text));
        assert!(matches!(
            storage.load_context_evidence("preview-draft", draft.revision + 1, &locator),
            Err(StorageError::DraftRevisionConflict { .. })
        ));
        let mut outside = locator.clone();
        outside.start_line = Some(1);
        assert!(
            storage
                .load_context_evidence("preview-draft", draft.revision, &outside)
                .is_err()
        );
        let mut mismatch = locator;
        mismatch.object_digest = Digest::from_bytes(b"other");
        assert!(
            storage
                .load_context_evidence("preview-draft", draft.revision, &mismatch)
                .is_err()
        );
        assert!(storage.pending_dispatches(10).unwrap().is_empty());
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn binary_preview_requires_exact_capture_locator_and_returns_safe_representation() {
        let original = Digest::from_bytes(b"image-original");
        let bytes=serde_json::to_vec(&serde_json::json!({"schema_version":1,"kind":"image","mime_type":"image/png","width":1,"height":1,"image_base64":"cG5n","pages":[],"warnings":["captured-only"]})).unwrap();
        let derived = Digest::from_bytes(&bytes);
        let locator = EvidenceLocator {
            source_id: "image".into(),
            object_digest: original.clone(),
            start_line: None,
            end_line: None,
            total_lines: None,
            page: None,
            width: Some(1),
            height: Some(1),
        };
        let source = ManifestSource {
            source_id: "image".into(),
            display_name: "image.png".into(),
            state: ManifestSourceState::Captured,
            byte_length: Some(14),
            mime_type: Some("image/png".into()),
            object_digest: Some(original.clone()),
            derived_digest: Some(derived.clone()),
            representation_kind: Some(RepresentationKind::Image),
            extractor_id: Some("native".into()),
            extractor_version: Some("1".into()),
            included_locators: vec![locator.clone()],
            omission: None,
            captured_at_epoch_ms: Some(1),
            secret_pattern_findings: vec![],
            secret_scan_incomplete: false,
        };
        let read = |digest: &Digest| {
            if digest == &original {
                Ok(b"image-original".to_vec())
            } else if digest == &derived {
                Ok(bytes.clone())
            } else {
                Err(StorageError::MissingObject(digest.to_string()))
            }
        };
        let view = render(&source, &locator, FreshnessStatus::Unchecked, read).unwrap();
        assert_eq!(view.data_url.as_deref(), Some("data:image/png;base64,cG5n"));
        assert_eq!(view.warnings, vec!["captured-only"]);
        let invalid = String::from_utf8(bytes.clone())
            .unwrap()
            .replace("image/png", "image/svg+xml");
        assert!(
            render(&source, &locator, FreshnessStatus::Unchecked, |digest| {
                if digest == &original {
                    Ok(b"image-original".to_vec())
                } else {
                    Ok(invalid.as_bytes().to_vec())
                }
            })
            .is_err()
        );
        let unavailable = render(&source, &locator, FreshnessStatus::Unchecked, |digest| {
            Err(StorageError::MissingObject(digest.to_string()))
        })
        .unwrap();
        assert!(unavailable.evidence_unavailable);
        assert!(unavailable.data_url.is_none());
        let mut wrong = locator;
        wrong.width = Some(2);
        assert!(render(&source, &wrong, FreshnessStatus::Unchecked, read).is_err());
    }
}
