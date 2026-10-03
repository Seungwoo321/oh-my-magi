use magi_domain::{CoreId, Digest, RunSnapshot, RunStage, status_name};
use magi_storage::{LiveDispatchProjection, LiveDispatchState, Storage};
use serde::Serialize;
use tauri::{State, WebviewWindow};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreDispatchProjection {
    pub run_id: String,
    pub run_revision: u64,
    pub input_digest: Digest,
    pub generation: u64,
    pub status: &'static str,
    pub stage: Option<RunStage>,
    pub projection_digest: Digest,
    pub core_dispatches: Vec<CoreDispatchView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreDispatchView {
    pub slot_ordinal: u8,
    pub state: LiveDispatchState,
    pub stage: RunStage,
    pub core_id: Option<CoreId>,
    pub binding_core_id: CoreId,
    pub result_ref: Option<String>,
}

pub fn project(
    snapshot: &RunSnapshot,
    live: LiveDispatchProjection,
) -> Result<CoreDispatchProjection, &'static str> {
    if snapshot.run.run_id != live.run_id
        || snapshot.run.revision != live.run_revision
        || snapshot.run.input_digest != live.input_digest
        || snapshot.run.generation != live.run_generation
    {
        return Err("dispatch_projection_revision_changed");
    }
    let digest =
        Digest::from_bytes(&serde_json::to_vec(&live).map_err(|_| "dispatch_projection_invalid")?);
    Ok(CoreDispatchProjection {
        run_id: live.run_id,
        run_revision: live.run_revision,
        input_digest: live.input_digest,
        generation: live.run_generation,
        status: status_name(&snapshot.run.status),
        stage: snapshot.run.status.stage(),
        projection_digest: digest,
        core_dispatches: live
            .dispatches
            .into_iter()
            .map(|slot| CoreDispatchView {
                slot_ordinal: slot.slot_ordinal,
                state: slot.state,
                stage: slot.stage,
                core_id: slot.core_id,
                binding_core_id: slot.binding_core_id,
                result_ref: slot.result_ref,
            })
            .collect(),
    })
}

pub fn load(storage: &Storage, run_id: &str) -> Result<CoreDispatchProjection, String> {
    for _ in 0..3 {
        let snapshot = storage
            .load_run_dossier(run_id)
            .map_err(|_| "dispatch_projection_unavailable")?
            .snapshot;
        let live = storage
            .load_live_dispatch_projection(run_id)
            .map_err(|_| "dispatch_projection_unavailable")?;
        match project(&snapshot, live) {
            Ok(value) => return Ok(value),
            Err("dispatch_projection_revision_changed") => continue,
            Err(error) => return Err(error.to_owned()),
        }
    }
    Err("dispatch_projection_revision_changed".into())
}

#[tauri::command]
pub async fn load_run_core_dispatches(
    window: WebviewWindow,
    state: State<'_, crate::commands::DesktopState>,
    run_id: String,
) -> Result<CoreDispatchProjection, String> {
    if !matches!(window.label(), "main" | "companion") {
        return Err("core dispatch state is restricted to the console and companion".into());
    }
    let storage = state.storage()?;
    tauri::async_runtime::spawn_blocking(move || load(&storage, &run_id))
        .await
        .map_err(|_| "dispatch_projection_worker_failed".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OwnedFixtureRoot(std::path::PathBuf);
    impl Drop for OwnedFixtureRoot {
        fn drop(&mut self) {
            if self.0.exists()
                && let Err(error) = std::fs::remove_dir_all(&self.0)
            {
                if std::thread::panicking() {
                    eprintln!("Owned dispatch fixture cleanup failed: {error}");
                } else {
                    panic!("Owned dispatch fixture cleanup failed: {error}");
                }
            }
        }
    }
    #[test]
    fn persisted_slot_activation_changes_projection_without_fabricating_accepted_results() {
        let prepared = crate::profiles::catalog_selection_ipc_tests::publication_fault_fixture();
        let _fixture = OwnedFixtureRoot(prepared.0.clone());
        let (root, storage, _aggregate, claim) = prepared;
        let reserved = load(&storage, &claim.run_id).unwrap();
        assert_eq!(reserved.core_dispatches.len(), 10);
        assert_eq!(
            reserved.core_dispatches[0].state,
            LiveDispatchState::Settled
        );
        let accepted_result = reserved.core_dispatches[0].result_ref.clone().unwrap();
        assert!(reserved.core_dispatches[1..].iter().all(|slot| {
            slot.state == LiveDispatchState::Reserved && slot.result_ref.is_none()
        }));
        storage
            .activate_live_run_dispatch_slot(&claim, 1, "2026-10-03T00:00:00Z")
            .unwrap();
        let active = load(&storage, &claim.run_id).unwrap();
        assert_ne!(reserved.projection_digest, active.projection_digest);
        assert_eq!(active.core_dispatches[0].state, LiveDispatchState::Settled);
        assert_eq!(
            active.core_dispatches[0].result_ref.as_deref(),
            Some(accepted_result.as_str())
        );
        assert_eq!(active.core_dispatches[1].state, LiveDispatchState::Active);
        assert!(active.core_dispatches[1].result_ref.is_none());
        assert!(
            active.core_dispatches[2..]
                .iter()
                .all(|slot| slot.state == LiveDispatchState::Reserved)
        );
        let snapshot = storage.load_run_dossier(&claim.run_id).unwrap().snapshot;
        let mut forged = storage
            .load_live_dispatch_projection(&claim.run_id)
            .unwrap();
        forged.run_revision += 1;
        assert!(project(&snapshot, forged).is_err());
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }
}
