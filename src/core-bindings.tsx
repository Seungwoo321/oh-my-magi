import { useCallback, useEffect, useRef, useState } from "react";
import { executionValuesEqual, catalogHasExecutionAuthority, catalogsHaveEquivalentExecutionAuthority, loadProviderCatalog, loadCoreModelSelections, loadCoreExecutionWitnesses, type CoreExecutionWitnesses, selectProviderModel, selectCoreModel, type AcpModelBindingSnapshot, type ProviderCatalogSnapshot, type ProfileAuthenticationBinding, type AuthProfileResult, type CoreId, type CoreBindingReference, type CoreModelSelectionState, type ProviderCatalogState } from "./lib/desktop-api";
import type { AcpProfile } from "./screens";

const CORE_IDS: CoreId[] = ["MELCHIOR-1", "BALTHASAR-2", "CASPER-3"];
function savedModelReady(state: ProviderCatalogState | undefined, profile: AcpProfile | undefined, requireOriginalIdentity = false): boolean {
  const catalog = state?.catalog;
  const saved = state?.modelSelection;
  const modes = catalog?.negotiatedModes;
  return Boolean(profile?.authenticationMethod === "local_subscription" && catalogHasExecutionAuthority(catalog) && saved && modes
    && state?.selectionState === "selected" && state.profileRevision === profile.revision
    && catalog.providerProfileId === profile.id && catalog.profileRevision === profile.revision
    && catalog.adapterId === profile.adapterId && catalog.providerId === profile.adapterId
    && saved.selectionRevision === state.modelSelectionRevision
    && saved.binding.providerProfileId === profile.id && saved.binding.profileRevision === profile.revision
    && saved.binding.schemaVersion === catalog.schemaVersion
    && saved.binding.artifactSetDigest === catalog.artifactSetDigest
    && saved.binding.providerId === catalog.providerId && saved.binding.acpMode === catalog.acpMode
    && saved.binding.adapterId === catalog.adapterId && saved.binding.adapterVersion === catalog.adapterVersion && saved.binding.adapterDigest === catalog.adapterDigest
    && (!requireOriginalIdentity || saved.binding.catalogSnapshotId === catalog.catalogSnapshotId && saved.binding.catalogDigest === catalog.catalogDigest)
    && catalog.models.some(model => model.modelId === saved.binding.modelId)
    && (modes.modes.length ? modes.modes.some(mode => mode.modeId === saved.binding.modeId) : modes.currentModeId === null && saved.binding.modeId == null));
}

type ConnectionCheck = { profileId: string; profileRevision: number; request: number; state: "checking" | "failed" | "ready"; authentication?: AuthProfileResult; catalog?: ProviderCatalogSnapshot };

export function useCoreBindings(profiles: AcpProfile[], enabled: boolean) {
  const [catalogProjections, setCatalogProjections] = useState<Record<string, { revision: number; state: "pending" | "ready" | "error" }>>({});
  const [catalogs, setCatalogs] = useState<Record<string, ProviderCatalogState>>({});
  const [witnesses, setWitnesses] = useState<CoreExecutionWitnesses | null>(null);
  const currentWitnesses = useRef<CoreExecutionWitnesses | null>(null);
  const [cores, setCores] = useState<CoreModelSelectionState[]>([]);
  const [drafts, setDrafts] = useState<Partial<Record<CoreId, string>>>({});
  const [busy, setBusy] = useState<string[]>([]);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [connectionChecks, setConnectionChecks] = useState<Record<string, ConnectionCheck>>({});
  const checkSequence = useRef(0);
  const currentChecks = useRef<Record<string, ConnectionCheck>>({});
  const epoch = useRef(0);
  const pending = useRef(new Set<string>());
  const currentProfiles = useRef(profiles);
  currentProfiles.current = profiles;
  const signature = profiles.map(profile => `${profile.id}:${profile.revision}`).join("|");
  const beginConnectionCheck = useCallback((profileId: string, profileRevision: number) => {
    const check: ConnectionCheck = { profileId, profileRevision, request: ++checkSequence.current, state: "checking" };
    currentChecks.current = { ...currentChecks.current, [profileId]: check };
    setConnectionChecks(currentChecks.current);
    return check;
  }, []);
  const finishConnectionCheck = useCallback((check: ConnectionCheck, ready: boolean, authentication?: AuthProfileResult, catalog?: ProviderCatalogSnapshot) => {
    if (currentChecks.current[check.profileId]?.request !== check.request
      || !currentProfiles.current.some(profile => profile.id === check.profileId && profile.revision === check.profileRevision)) return;
    const authenticated = ready && catalogHasExecutionAuthority(catalog)
      && catalog.providerProfileId === check.profileId && catalog.profileRevision === check.profileRevision
      && authentication?.profileId === check.profileId
      && authentication.profileRevision === check.profileRevision && authentication.state === "authenticated"
      && authentication.method === "chat_gpt" && currentProfiles.current.some(profile => profile.id === check.profileId && profile.adapterId === authentication.providerId);
    currentChecks.current = { ...currentChecks.current, [check.profileId]: { ...check, state: authenticated ? "ready" : "failed", authentication: authenticated ? authentication : undefined, catalog: authenticated ? catalog : undefined } };
    setConnectionChecks(currentChecks.current);
  }, []);
  const invalidateAuthentication = useCallback((binding: ProfileAuthenticationBinding) => {
    if (!currentProfiles.current.some(profile => profile.id === binding.providerProfileId && profile.revision === binding.profileRevision)) return;
    const check: ConnectionCheck = { profileId: binding.providerProfileId, profileRevision: binding.profileRevision, request: ++checkSequence.current, state: "failed" };
    currentChecks.current = { ...currentChecks.current, [binding.providerProfileId]: check };
    setConnectionChecks(currentChecks.current);
  }, []);
  const verifiedAuthentication = (profile: AcpProfile | undefined) => {
    const check = profile ? connectionChecks[profile.id] : undefined;
    return check?.state === "ready" && check.profileRevision === profile?.revision ? check.authentication ?? null : null;
  };
  const profileConnectionState = (profile: AcpProfile | undefined): "not_checked" | "stale" | ConnectionCheck["state"] => {
    const check = profile ? connectionChecks[profile.id] : undefined;
    if (!check) return "not_checked";
    if (check.profileRevision !== profile?.revision) return "stale";
    return check.state;
  };
  const profileCatalogProjection = (profile: AcpProfile): "pending" | "ready" | "error" => {
    const projection = catalogProjections[profile.id];
    return projection?.revision === profile.revision ? projection.state : "pending";
  };
  const connectionReady = (profile: AcpProfile | undefined) => Boolean(profile
    && currentProfiles.current.some(current => current.id === profile.id && current.revision === profile.revision)
    && currentChecks.current[profile.id]?.profileRevision === profile.revision
    && currentChecks.current[profile.id]?.state === "ready");
  const bindingCurrentlyVerified = (profile: AcpProfile | undefined, model: AcpModelBindingSnapshot | undefined) => {
    const catalog = profile ? currentChecks.current[profile.id]?.catalog : undefined;
    return Boolean(catalogHasExecutionAuthority(catalog) && model
      && catalogsHaveEquivalentExecutionAuthority(catalog, profile ? catalogs[profile.id]?.catalog : undefined)
      && catalog.artifactSetDigest === model.artifactSetDigest
      && catalog.adapterDigest === model.adapterDigest && catalog.adapterVersion === model.adapterVersion
      && catalog.models.some(item => item.modelId === model.modelId)
      && (catalog.negotiatedModes.modes.length ? catalog.negotiatedModes.modes.some(item => item.modeId === model.modeId) : catalog.negotiatedModes.currentModeId === null && model.modeId == null));
  };

  const reload = useCallback(async () => {
    const request = ++epoch.current;
    currentWitnesses.current = null;
    setWitnesses(null);
    setCores([]);
    setCatalogs({});
    if (!enabled) return;
    const capturedSignature = currentProfiles.current.map(profile => `${profile.id}:${profile.revision}`).join("|");
    const captured = currentProfiles.current.filter(profile => profile.authenticationMethod === "local_subscription");
    setCatalogProjections(Object.fromEntries(captured.map(profile => [profile.id, { revision: profile.revision, state: "pending" as const }])));
    let coreProjectionReady = false;
    const results = await Promise.allSettled(captured.map(profile => loadProviderCatalog(profile.id, profile.revision)));
    const next: Record<string, ProviderCatalogState> = {};
    const failures: Record<string, string> = {};
    results.forEach((result, index) => {
      const profile = captured[index];
      if (result.status === "fulfilled" && result.value?.providerProfileId === profile.id && result.value.profileRevision === profile.revision) next[profile.id] = result.value;
      else failures[profile.id] = "저장된 모델 연결을 확인하지 못했습니다. 연결을 다시 확인하십시오.";
    });
    try {
      const rows = await loadCoreModelSelections();
      if (request !== epoch.current || capturedSignature !== currentProfiles.current.map(profile => `${profile.id}:${profile.revision}`).join("|")) return;
      if (!Array.isArray(rows) || rows.length !== 3 || CORE_IDS.some(coreId => rows.filter(row => row.coreId === coreId).length !== 1)) throw new Error("Invalid core selection set");
      coreProjectionReady = true;
      setCatalogs(next);
      setCores(rows);
      setCatalogProjections(Object.fromEntries(captured.map(profile => [profile.id, { revision: profile.revision, state: next[profile.id] ? "ready" as const : "error" as const }])));
      const references = CORE_IDS.map(coreId => {
        const row = rows.find(item => item.coreId === coreId);
        const saved = row?.selection;
        return row?.selectionState === "selected" && saved && saved.selectionRevision === row.selectionRevision
          ? { coreId, providerProfileId: saved.providerProfileId, profileRevision: saved.profileRevision, modelSelectionRevision: saved.modelSelectionRevision, coreSelectionRevision: saved.selectionRevision } : null;
      });
      if (references.every((reference): reference is CoreBindingReference => reference !== null)) {
        const proof = await loadCoreExecutionWitnesses(references);
        if (request !== epoch.current || capturedSignature !== currentProfiles.current.map(profile => `${profile.id}:${profile.revision}`).join("|")) return;
        currentWitnesses.current = proof;
        setWitnesses(proof);
      }
      setErrors(previous => ({ ...previous, ...failures, load: "" }));
    } catch {
      if (request === epoch.current && capturedSignature === currentProfiles.current.map(profile => `${profile.id}:${profile.revision}`).join("|")) { if (!coreProjectionReady) { setCatalogs({}); setCatalogProjections(Object.fromEntries(captured.map(profile => [profile.id, { revision: profile.revision, state: "error" as const }]))); } setErrors(previous => ({ ...previous, load: "세 코어의 저장된 연결을 확인하지 못했습니다." })); }
    }
  }, [enabled]);
  useEffect(() => { void reload(); return () => { epoch.current++; }; }, [reload, signature]);
  const perform = async (key: string, action: () => Promise<void>) => {
    if (pending.current.size) return;
    pending.current.add(key);
    setBusy(previous => [...previous, key]);
    setErrors(previous => ({ ...previous, [key]: "" }));
    const request = epoch.current;
    const capturedSignature = currentProfiles.current.map(profile => `${profile.id}:${profile.revision}`).join("|");
    try { await action(); }
    catch { if (request === epoch.current && capturedSignature === currentProfiles.current.map(profile => `${profile.id}:${profile.revision}`).join("|")) { currentWitnesses.current = null; setWitnesses(null); setErrors(previous => ({ ...previous, [key]: "저장하지 못했습니다. 선택 내용은 유지됩니다. 현재 연결을 다시 확인한 뒤 재시도하십시오." })); } }
    finally { pending.current.delete(key); setBusy(previous => previous.filter(item => item !== key)); }
  };
  const saveModel = async (profileId: string, modelId: string, modeId: string | null) => {
    const state = catalogs[profileId];
    const profile = currentProfiles.current.find(item => item.id === profileId);
    const catalog = state?.catalog;
    if (!profile || !catalogHasExecutionAuthority(catalog) || profile.revision !== state.profileRevision
      || !catalog.models.some(model => model.modelId === modelId)
      || (catalog.negotiatedModes.modes.length ? !catalog.negotiatedModes.modes.some(mode => mode.modeId === modeId) : modeId !== null || catalog.negotiatedModes.currentModeId !== null)) return;
    const request = epoch.current;
    await perform(profileId, async () => {
      setCatalogs(previous => ({ ...previous, [profileId]: { ...state, modelSelection: null, selectionState: "stale" } }));
      const saved = await selectProviderModel({ providerProfileId: profileId, profileRevision: profile.revision, catalogSnapshotId: catalog.catalogSnapshotId, catalogDigest: catalog.catalogDigest, modelId, modeId, expectedSelectionRevision: state.modelSelectionRevision });
      const readback = await loadProviderCatalog(profileId, profile.revision);
      if (request !== epoch.current || !currentProfiles.current.some(item => item.id === profileId && item.revision === profile.revision)) return;
      if (readback.modelSelection?.selectionRevision !== saved.selectionRevision || readback.modelSelection.binding.modelId !== modelId || (readback.modelSelection.binding.modeId ?? null) !== modeId) throw new Error("Model save readback mismatch");
      await reload();
    });
  };
  const saveCore = async (coreId: CoreId) => {
    const profileId = drafts[coreId] ?? cores.find(row => row.coreId === coreId)?.selection?.providerProfileId;
    const profile = currentProfiles.current.find(item => item.id === profileId);
    const state = profileId ? catalogs[profileId] : undefined;
    if (!profile || !savedModelReady(state, profile) || !state?.modelSelection) return;
    const prior = cores.find(row => row.coreId === coreId);
    const request = epoch.current;
    await perform(coreId, async () => {
      const saved = await selectCoreModel({ coreId, providerProfileId: profile.id, profileRevision: profile.revision, modelSelectionRevision: state.modelSelection!.selectionRevision, expectedSelectionRevision: prior?.selectionRevision ?? null });
      const rows = await loadCoreModelSelections();
      if (request !== epoch.current || !currentProfiles.current.some(item => item.id === profile.id && item.revision === profile.revision)) return;
      if (!Array.isArray(rows) || rows.length !== 3 || CORE_IDS.some(id => rows.filter(row => row.coreId === id).length !== 1)) throw new Error("Invalid core selection set");
      const row = rows.find(item => item.coreId === coreId);
      if (row?.selection?.selectionRevision !== saved.selectionRevision || row.selection.providerProfileId !== profile.id || row.selection.modelSelectionRevision !== state.modelSelection!.selectionRevision) throw new Error("Core save readback mismatch");
      await reload();
    });
  };
  const destinations = CORE_IDS.map(coreId => {
    const row = cores.find(item => item.coreId === coreId);
    const saved = row?.selection;
    const profile = profiles.find(item => item.id === saved?.providerProfileId);
    const state = profile ? catalogs[profile.id] : undefined;
    const dirty = drafts[coreId] !== undefined && drafts[coreId] !== (saved?.providerProfileId ?? "");
    const model = state?.modelSelection?.binding;
    const witnessEntry = witnesses?.cores.find(entry => entry.coreBindingReference.coreId === coreId);
    const witness = witnessEntry?.catalogExecutionWitness;
    const refMatches = Boolean(saved && witnessEntry && witnessEntry.coreBindingReference.providerProfileId === saved.providerProfileId
      && witnessEntry.coreBindingReference.profileRevision === saved.profileRevision && witnessEntry.coreBindingReference.modelSelectionRevision === saved.modelSelectionRevision
      && witnessEntry.coreBindingReference.coreSelectionRevision === saved.selectionRevision);
    const originalState = state && witness ? { ...state, catalog: witness.originalCatalog } : undefined;
    const currentAuthority = Boolean(refMatches && witness && model && executionValuesEqual(witness.binding, model)
      && catalogsHaveEquivalentExecutionAuthority(witness.freshCatalog, state?.catalog) && bindingCurrentlyVerified(profile, model));
    const ready = Boolean(connectionReady(profile) && currentAuthority && !dirty && row?.selectionState === "selected" && saved && saved.selectionRevision === row.selectionRevision && saved.profileRevision === profile?.revision && saved.modelSelectionRevision === state?.modelSelection?.selectionRevision && savedModelReady(originalState, profile, true));
    const reference: CoreBindingReference | null = ready && saved ? { coreId, providerProfileId: saved.providerProfileId, profileRevision: saved.profileRevision, modelSelectionRevision: saved.modelSelectionRevision, coreSelectionRevision: saved.selectionRevision } : null;
    return { coreId, profile, model: state?.modelSelection?.binding, dirty, ready, reference, connectionState: profile && connectionChecks[profile.id]?.profileRevision === profile.revision ? connectionChecks[profile.id].state : "not_checked" };
  });
  return { catalogs, cores, drafts, busy, errors, destinations, ready: destinations.every(item => item.ready) && busy.length === 0, canStart: () => currentWitnesses.current === witnesses && destinations.every(item => item.ready && connectionReady(item.profile) && bindingCurrentlyVerified(item.profile, item.model)) && pending.current.size === 0, beginConnectionCheck, finishConnectionCheck, invalidateAuthentication, verifiedAuthentication, profileConnectionState, profileCatalogProjection, reload, saveModel, saveCore, setDraft: (coreId: CoreId, profileId: string) => setDrafts(previous => ({ ...previous, [coreId]: profileId })) };
}
export type CoreBindingsController = ReturnType<typeof useCoreBindings>;

export function CoreBindingControls({ controller, profiles }: { controller: CoreBindingsController; profiles: AcpProfile[] }) {
  return <><button className="button" disabled={controller.busy.length > 0} onClick={() => { void controller.reload(); }}>저장 상태 다시 확인</button>{controller.errors.load && <p role="alert">{controller.errors.load}</p>}{controller.destinations.map(destination => {
    const { coreId } = destination;
    const draft = controller.drafts[coreId] ?? destination.profile?.id ?? "";
    const profile = profiles.find(item => item.id === draft);
    const canSave = savedModelReady(controller.catalogs[draft], profile);
    return <div className="field" key={coreId}><label className="field-label" htmlFor={`core-${coreId}`}>{coreId}</label><p>저장된 연결 · {destination.profile && destination.model ? `${destination.profile.displayName} · ${destination.model.modelId}` : "확인 필요"}</p>{destination.connectionState !== "ready" && <p className="field-help" role="status">{destination.connectionState === "checking" ? "연결 확인 중 · 심의를 시작할 수 없습니다." : destination.connectionState === "failed" ? "연결 확인 실패 · 저장된 선택은 유지됩니다. 연결을 다시 확인하십시오." : "저장된 선택 · 현재 연결을 자동으로 확인합니다."}</p>}{destination.dirty && <p className="field-help" role="status">편집 중인 선택 · {profile?.displayName ?? "미선택"}. 코어 연결을 저장해야 심의를 시작할 수 있습니다.</p>}<select id={`core-${coreId}`} className="text-field" value={draft} disabled={controller.busy.length > 0} onChange={event => controller.setDraft(coreId, event.target.value)}><option value="">이 코어에 사용할 프로필 선택</option>{profiles.filter(item => item.authenticationMethod === "local_subscription").map(item => <option key={item.id} value={item.id}>{item.displayName}</option>)}</select><button className="button" disabled={!canSave || controller.busy.length > 0} onClick={() => { void controller.saveCore(coreId); }}>{controller.busy.length > 0 ? "저장 중…" : "코어 연결 저장"}</button>{controller.errors[coreId] && <p role="alert">{controller.errors[coreId]}</p>}</div>;
  })}{controller.ready && new Set(controller.destinations.map(item => `${item.model?.providerId}:${item.model?.modelId}:${item.model?.modeId ?? ""}`)).size === 1 && <p className="field-help">세 코어가 같은 모델을 사용합니다. 역할이 달라도 모델의 판단 경향은 겹칠 수 있습니다.</p>}</>;
}
