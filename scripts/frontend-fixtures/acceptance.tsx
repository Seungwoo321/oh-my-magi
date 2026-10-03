import {StoredReplay,RecordEvidenceBrowser} from "../../src/records";
import {createRoot} from 'react-dom/client';
import {useLayoutEffect,useState} from 'react';
import {ContextPreview} from '../../src/context-preview';
import {SourceFreshness} from '../../src/source-freshness';
import {RoleTransfer} from '../../src/role-transfer';
import {SignedUpdatePanel} from '../../src/native-settings';
import {ReviewedSharePanel,RestoreDataPanel} from '../../src/record-actions';
import {BudgetPreview} from '../../src/budget-preview';
import type {ContextSelectionSummary,RunDossierView} from '../../src/lib/desktop-api';
import {validateSettingsSnapshot} from '../../src/lib/desktop-api';
const validSettings={schemaVersion:1,revision:0,preferences:{motion:'full',sound:false,theme:'command',fontScale:100,language:'ko',commonContextTokenLimit:32000},fieldRevisions:{motion:0,sound:0,theme:0,fontScale:0,language:0,commonContextTokenLimit:0}};
validateSettingsSnapshot(validSettings);
for(const limit of [0,128001,1.5,NaN,'32000']) {
  let rejected=false;
  try {validateSettingsSnapshot({...validSettings,preferences:{...validSettings.preferences,commonContextTokenLimit:limit}});} catch {rejected=true;}
  if(!rejected)throw Error('invalid_common_budget_snapshot_accepted');
}
for(const limit of [1,128000])validateSettingsSnapshot({...validSettings,preferences:{...validSettings.preferences,commonContextTokenLimit:limit}});
const foreignSettings={...validSettings,preferences:{...validSettings.preferences,unexpectedPolicy:1}};
let foreignRejected=false;try{validateSettingsSnapshot(foreignSettings);}catch{foreignRejected=true;}if(!foreignRejected)throw Error('foreign_settings_key_accepted');
const loc={source_id:'source-1',object_digest:'digest',start_line:1,end_line:2,total_lines:2};
const initial:ContextSelectionSummary={draftId:'draft-1',manifestId:'manifest',manifestDigest:'digest',revision:1,sources:[{sourceId:'source-1',displayName:'Captured source',status:'captured',byteLength:12,representation:'utf8_text',issueCodes:[],includedLocators:[loc]}]};
const dossier:RunDossierView={runId:'run-1',generation:1,revision:7,question:'Saved question',status:'completed',stage:'completed',assessments:['MELCHIOR-1','BALTHASAR-2','CASPER-3'].flatMap(coreId=>[1,2].map(n=>({assessmentId:coreId+'-'+n,coreId:coreId as any,positionSummary:'Actual saved opinion '+n}))),proposal:{kind:'recommendation',body:'Saved proposal',conditions:[],alternatives:[],openObjections:[]},votes:['MELCHIOR-1','BALTHASAR-2','CASPER-3'].map(coreId=>({coreId:coreId as any,choice:'support',rationale:'Actual rationale'})),outcome:'unanimous'};
const bindings=['MELCHIOR-1','BALTHASAR-2','CASPER-3'].map(coreId=>({coreId,providerProfileId:coreId,profileRevision:1,modelSelectionRevision:0,coreSelectionRevision:0}));
function Fixture(){const [revision,setRevision]=useState(7);const [selection,setSelection]=useState(initial);useLayoutEffect(()=>{if(selection.revision>1){const old=document.querySelector('.evidence-source[aria-label="전송 전 캡처 원문"]');if(old)throw new Error("stale_context_first_commit");(window as any).contextFirstCommitChecked=selection.revision;}},[selection]);const [budgetQuestion,setBudgetQuestion]=useState("Question");const [budget,setBudget]=useState({key:'',ready:false});return <><ContextPreview selection={selection} onSelectionChange={setSelection}/><button id="revise-context" onClick={()=>setSelection({...initial,draftId:"draft-2",revision:2})}>Replace captured draft</button><SourceFreshness runId="run-1" sourceId="source-1"/><RoleTransfer selected={{id:'user-role',kind:'user',revision:3} as any} onChanged={()=>{(window as any).changed=true}}/><SignedUpdatePanel/><ReviewedSharePanel dossier={dossier}/><RestoreDataPanel/><BudgetPreview requestKey={JSON.stringify({expectedCommonContextBudgetRevision:0,question:budgetQuestion,contextDraftId:'draft-1',contextRevision:1,coreBindings:bindings,rolePresetId:'role',roleRevision:3})} enabled onResult={setBudget}/><button id="block-budget" onClick={()=>setBudgetQuestion("Blocked")}>Invalidate budget</button><div id="replay"><StoredReplay runId="run-1" dossier={dossier}/></div><div id="evidence"><RecordEvidenceBrowser runId="run-1" revision={revision}/></div><button id="revise-evidence" onClick={()=>{(window as any).evidenceRevision=8;setRevision(8)}}>Revise evidence</button><output id="budget-result">{String(budget.ready)}</output></>};createRoot(document.getElementById('root')!).render(<Fixture/>);
