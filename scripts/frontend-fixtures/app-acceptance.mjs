import assert from 'node:assert/strict';
export async function runAppAcceptance(browser,base){
const page = await browser.newPage({viewport:{width:1440,height:720}});
const pageErrors=[]; page.on('pageerror',error=>{pageErrors.push(error.message);console.log('PAGEERROR',error.message)});
await page.addInitScript(()=>{
 window.isTauri=true;
 let callback=0; const callbacks={}; const events={};
 window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(){}};
 const profiles=[{providerProfileId:'fixture-profile',revision:1,providerId:'codex-acp',displayName:'Saved profile',accountAlias:'Saved profile',authenticationMethod:'local_subscription',credentialConfigured:true,credentialHome:{displayPath:'~/.codex'},digest:'fixture-digest',updatedAt:'2026-10-01T00:00:00Z'}];
 window.fixture={profiles,saveMode:'success',saveCalls:0, calls:[], modes:{}, pending:{},validationReady:new URLSearchParams(location.search).has('terminal')||new URLSearchParams(location.search).has('manual-initial')};
 const catalog={schemaVersion:3,artifactSetDigest:'d'.repeat(64),catalogSnapshotId:'cat1',catalogDigest:'f'.repeat(64),providerId:'codex-acp',acpMode:'acp',providerProfileId:'fixture-profile',profileRevision:1,adapterId:'codex-acp',adapterVersion:'1',adapterDigest:'e'.repeat(64),fetchedAt:'2026-10-01T00:00:00Z',models:[{modelId:'model-a',name:'Model A'},{modelId:'model-b',name:'Model B'}],negotiatedModes:{currentModeId:'default',modes:[{modeId:'default',name:'Default',description:null}]}};
 window.fixture.catalog={providerProfileId:'fixture-profile',profileRevision:1,catalog,modelSelection:null,modelSelectionRevision:null,selectionState:'unselected'};
 window.fixture.cores=['MELCHIOR-1','BALTHASAR-2','CASPER-3'].map(coreId=>({coreId,selection:null,selectionRevision:null,selectionState:'unselected'}));
 window.__TAURI_INTERNALS__={transformCallback(fn){callbacks[++callback]=fn;return callback},async invoke(cmd,args){
  window.fixture.calls.push({cmd,args});
  if(['validate_provider_profile','authenticate_provider_profile','refresh_provider_catalog'].includes(cmd)){
   const id=args.profileId, rev=args.expectedRevision??profiles.find(p=>p.providerProfileId===id).revision;
   const mode=window.fixture.modes[id];
   if(mode==='pending' && cmd==='authenticate_provider_profile') await new Promise((resolve,reject)=>window.fixture.pending[id]={resolve,reject});
   if(mode==='fail' && cmd==='authenticate_provider_profile') throw {code:'authentication_required',message:'Fixture existing subscription unavailable',retryable:true};
   if(cmd==='validate_provider_profile' && window.fixture.validationReady)return {state:'ready',profileId:id,profileRevision:rev,adapterId:'codex-acp',rootBinding:'verified',checkedAt:'2026-10-03T00:00:00Z'};
   if(cmd==='validate_provider_profile')return {state:'blocked',profileId:id,profileRevision:rev,adapterId:'codex-acp',rootBinding:'unverified',reason:'other',checkedAt:'2026-10-03T00:00:00Z'};
   if(cmd==='authenticate_provider_profile')return {providerId:'codex-acp',profileId:id,profileRevision:rev,state:'authenticated',method:'chat_gpt',checkedAt:'fixture'};
   return structuredClone(window.fixture.catalogs[id]);
  }
  if(cmd==='preview_deliberation_budget'){
   const snapshot=window.fixture.preferences;
   if(args.input.expectedCommonContextBudgetRevision!==snapshot.fieldRevisions.commonContextTokenLimit)throw Error('controlled budget revision conflict');
   const preview={commonContextBudget:{schemaVersion:1,tokenLimit:snapshot.preferences.commonContextTokenLimit,settingsFieldRevision:snapshot.fieldRevisions.commonContextTokenLimit},inputFingerprint:String(snapshot.fieldRevisions.commonContextTokenLimit).repeat(64),ready:snapshot.fieldRevisions.commonContextTokenLimit!==window.fixture.deferBudgetRevision,futureArtifactsKnown:false,scope:'serialized_base_inputs_conservative',warnings:snapshot.fieldRevisions.commonContextTokenLimit===window.fixture.deferBudgetRevision?['CONTROLLED OLD POLICY MUST NOT PUBLISH']:[],slots:Array.from({length:10},(_,i)=>({slot:i+1,stage:'independent_review',coreId:'MELCHIOR-1',baseInputBytes:1,budget:{},blockedReason:null}))};
   if(window.fixture.deferBudgetRevision===args.input.expectedCommonContextBudgetRevision){window.fixture.budgetResolvers??=[];return new Promise(resolve=>{window.fixture.budgetResolvers.push(()=>resolve(preview));window.fixture.resolveBudget=()=>{for(const release of window.fixture.budgetResolvers.splice(0))release();};});}
   return preview;
  }
  if(cmd==='get_console_preferences'){window.fixture.preferences??={schemaVersion:1,revision:0,preferences:{motion:'full',sound:false,theme:'command',fontScale:100,language:'ko',commonContextTokenLimit:32000},fieldRevisions:{motion:0,sound:0,theme:0,fontScale:0,language:0,commonContextTokenLimit:0}};return structuredClone(window.fixture.preferences);}
  if(cmd==='save_console_preferences'){
   const input=args.input,snapshot=window.fixture.preferences;
   const receipts=window.fixture.preferenceReceipts??={};
   let receipt=receipts[input.idempotencyKey];
   if(!receipt){for(const field of Object.keys(input.patch)){if(snapshot.fieldRevisions[field]!==input.expectedFieldRevisions[field])throw Error('revision conflict');snapshot.preferences[field]=input.patch[field];snapshot.fieldRevisions[field]++;}snapshot.revision++;receipt={schemaVersion:1,commandId:input.commandId,idempotencyKey:input.idempotencyKey,intentDigest:'a'.repeat(64),committedAt:'server',snapshot:structuredClone(snapshot)};receipts[input.idempotencyKey]=receipt;}
   if(window.fixture.preferenceReplyLost && 'commonContextTokenLimit' in input.patch)throw Error('controlled response lost after commit');
   return {schemaVersion:1,receipt:structuredClone(receipt),notification:{state:'delivered'}};
  }
  if(cmd==='plugin:event|listen'){events[args.event]=args.handler;window.fixture.emit=(event,payload)=>callbacks[events[event]]?.({event,id:1,payload});return 1;}
  if(cmd.startsWith('plugin:event|'))return 1;
  if(cmd==='shell_context')return {windowLabel:location.search.includes('companion')?'companion':'main',platform:'macos'};
  if(cmd==='get_console_snapshot'){const terminal=new URLSearchParams(location.search).get('terminal');return {schemaVersion:1,connection:'blocked',storage:'ready',...(terminal||location.search.includes('queue')?{activeRun:{id:'queue-run',question:'Queue question',stage:terminal??'independent_review',status:terminal??'independent_review',sourceCount:0,roles:[],ballotState:'none'}}:{})};}
  if(cmd==='load_run_dossier'){if(window.fixture.deferDossier){window.fixture.deferDossier=false;return new Promise(resolve=>window.fixture.resolveDossier=resolve);}return {...structuredClone(window.fixture.dossier),runId:args.runId};}
  if(cmd==='list_record_evidence')return {runId:args.runId,revision:window.fixture.dossier.revision,sources:[]};
  if(cmd==='list_records')return {items:window.fixture.rows,nextCursor:null};
  if(cmd==='load_run_core_dispatches'){if(window.fixture.deferQueue){window.fixture.deferQueue=false;return new Promise(resolve=>window.fixture.resolveQueue=resolve);}return structuredClone(window.fixture.queue);}
  if(cmd==='list_acp_adapters')return [{id:'codex-acp',displayName:'Codex ACP',state:'supported'}];
  if(cmd==='list_provider_profiles')return structuredClone(profiles);
  if(cmd==='list_recent_runs')return location.search.includes('queue')?window.fixture.rows:[];
  if(cmd==='list_provider_source_scopes')return [];
  if(cmd==='list_role_presets')return [{presetId:'factory.magi.default',revision:1,displayName:'Fixture roles',digest:'roles-digest',source:'factory',roles:['MELCHIOR-1','BALTHASAR-2','CASPER-3'].map(coreId=>({coreId,profileId:'role-'+coreId,displayName:coreId,reviewPurpose:'Independent review',evaluationCriteria:['Evidence'],falsificationQuestions:['Disproof?'],responseLanguage:'ko'}))}];
  if(cmd==='load_active_role_preset_selection')return {presetId:'factory.magi.default',selectionRevision:0,updatedAt:'server'};
  if(cmd==='register_deliberation_request')return {kind:'registered',admissionAuthority:{token:'11111111-1111-4111-8111-111111111111',processEpoch:'22222222-2222-4222-8222-222222222222',expiresAt:'2099-01-01T00:00:00Z'}};
  if(cmd==='start_deliberation'){window.fixture.started=args;throw new Error('Independent capture only');}
  if(cmd==='load_active_provider_profile_selection') return window.fixture.active??null;
  if(cmd==='set_active_provider_profile'){window.fixture.active={providerId:args.providerId,providerProfileId:args.profileId,selectionRevision:(window.fixture.active?.selectionRevision??-1)+1,updatedAt:'fixture'};return structuredClone(window.fixture.active);}
  if(cmd==='load_provider_catalog') return structuredClone(window.fixture.catalogs[args.profileId]);
  if(cmd==='load_core_model_selections') return structuredClone(window.fixture.cores);
  if(cmd==='load_core_execution_witnesses')return {schemaVersion:1,cores:args.input.coreBindings.map(coreBindingReference=>{const state=window.fixture.catalogs[coreBindingReference.providerProfileId];return {coreBindingReference:structuredClone(coreBindingReference),catalogExecutionWitness:{binding:structuredClone(state.modelSelection.binding),originalCatalog:structuredClone(state.catalog),freshCatalog:structuredClone(state.catalog)}};})};
  if(cmd==='select_provider_model') {
    const input=args.input; window.fixture.lastModelInput=input;
    if(window.fixture.modelFail) throw new Error('save failed');
    const state=window.fixture.catalogs[input.providerProfileId];const rev=state.modelSelectionRevision;
    if(input.expectedSelectionRevision!==rev) throw new Error('CAS mismatch');
    const {fetchedAt,models,negotiatedModes,...identity}=state.catalog;const saved={binding:{...identity,modelId:input.modelId,modeId:input.modeId,bindingDigest:'9'.repeat(64)},selectionRevision:rev===null?0:rev+1,updatedAt:'server'};
    state.modelSelection=saved;state.modelSelectionRevision=saved.selectionRevision;state.selectionState='selected';return structuredClone(saved);
  }
  if(cmd==='select_core_model') {
    window.fixture.lastCoreInput=args.input;
    const row=window.fixture.cores.find(row=>row.coreId===args.input.coreId);
    if(args.input.expectedSelectionRevision!==row.selectionRevision)throw new Error('CAS mismatch');
    row.selectionRevision=row.selectionRevision===null?0:row.selectionRevision+1;row.selectionState='selected';row.selection={...args.input,selectionRevision:row.selectionRevision,updatedAt:'server'};return structuredClone(row.selection);
  }
  if(cmd==='save_provider_profile'){
   window.fixture.saveCalls++; window.fixture.lastDraft=args.draft;
   if(window.fixture.saveMode==='error')throw new Error('fixture save failure');
   if(window.fixture.saveMode==='pending')await new Promise(resolve=>window.fixture.resolveSave=resolve);
   const d=args.draft,p={...profiles[0],providerProfileId:d.profileId??'created-fixture',revision:(d.expectedRevision??0)+1,displayName:d.displayName,accountAlias:d.displayName,credentialHome:{displayPath:d.credentialHomePath}};
   const i=profiles.findIndex(x=>x.providerProfileId===p.providerProfileId);if(i<0)profiles.push(p);else profiles[i]=p;if(location.search.includes('manual-initial')){const state=window.fixture.catalogs[p.providerProfileId];state.profileRevision=p.revision;state.catalog.profileRevision=p.revision;state.selectionState='stale';}return structuredClone(p);
  }
  return null;
 }};
 profiles.splice(0,profiles.length,...[1,2,3].map(n=>({...profiles[0],providerProfileId:'p'+n,displayName:'Profile '+n,accountAlias:'Profile '+n})));
 window.fixture.rows=['queue-run','other-run'].map((runId,index)=>({runId,revision:2,question:index?'Other question':'Queue question',status:'independent_review',createdAt:'2026-10-03T00:00:00Z',updatedAt:'2026-10-03T00:00:00Z'}));
 window.fixture.dossier={runId:'queue-run',revision:2,generation:2,question:'Queue question',stage:'independent_review',status:'independent_review',proposal:{kind:'answer',body:'BASELINE DOSSIER',conditions:[],alternatives:[],openObjections:[]},votes:[],outcome:null};
 const terminalStatus=new URLSearchParams(location.search).get('terminal');if(terminalStatus){window.fixture.dossier.status=terminalStatus;window.fixture.dossier.stage=terminalStatus;}
 window.fixture.queue={runId:'queue-run',runRevision:2,generation:2,inputDigest:'a'.repeat(64),projectionDigest:'b'.repeat(64),status:'independent_review',stage:'independent_review',coreDispatches:Array.from({length:10},(_,index)=>({slotOrdinal:index+1,stage:index<3?'independent_review':index<6?'cross_review':index===6?'synthesis':'balloting',bindingCoreId:['MELCHIOR-1','BALTHASAR-2','CASPER-3'][index%3],coreId:index===6?null:['MELCHIOR-1','BALTHASAR-2','CASPER-3'][index%3],state:index===0?'settled':index===1?'active':'reserved',resultRef:null}))};
 window.fixture.catalogs={};
 profiles.forEach((p,i)=>{const c={...catalog,providerProfileId:p.providerProfileId,catalogSnapshotId:'cat'+i,catalogDigest:String(i+1).repeat(64),models:[{modelId:'model'+i,name:'Model '+i,description:null,contextWindowTokens:128000,maxOutputTokens:8192}]};const {fetchedAt,models,negotiatedModes,...identity}=c;window.fixture.catalogs[p.providerProfileId]={providerProfileId:p.providerProfileId,profileRevision:1,catalog:c,modelSelection:{binding:{...identity,modelId:'model'+i,modeId:'default',bindingDigest:String(i+4).repeat(64)},selectionRevision:0,updatedAt:'server'},modelSelectionRevision:0,selectionState:'selected'};});
 window.fixture.cores.forEach((row,i)=>{row.selectionState='selected';row.selectionRevision=0;row.selection={coreId:row.coreId,providerProfileId:'p'+(i+1),profileRevision:1,modelSelectionRevision:0,selectionRevision:0,updatedAt:'server'};});
if(location.search.includes('manual-initial'))window.fixture.cores.forEach(row=>{row.selection=null;row.selectionRevision=null;row.selectionState='unselected';});
});




try {
await page.goto(base);
await page.getByRole('button',{name:'새 심의 시작',exact:true}).click();
await page.locator('#question-draft').fill('Merged source retained question');
await page.evaluate(()=>window.scrollTo(0,160));
const priorScroll=await page.evaluate(()=>window.scrollY);assert.ok(priorScroll>0,'real product document must have scrollable draft');
await page.locator('.connection-state').click();await page.getByRole('heading',{name:'모델 연결',exact:true}).waitFor();
await page.getByRole('button',{name:'원래 화면으로 돌아가기',exact:true}).click();await page.waitForFunction(top=>Math.abs(window.scrollY-top)<2,priorScroll);assert.equal(await page.locator('#question-draft').inputValue(),'Merged source retained question');
console.log('PASS blocked connection-status direct entry and real document scroll restoration preserve draft');
await page.getByRole('button',{name:'설정',exact:true}).first().click();
const cap=page.getByRole('spinbutton',{name:'공통 문맥 토큰 상한',exact:true});
await cap.waitFor();assert.equal(await cap.inputValue(),'32000');
const capCalls=()=>page.evaluate(()=>fixture.calls.filter(c=>c.cmd==='save_console_preferences' && 'commonContextTokenLimit' in c.args.input.patch).length);
const beforeCap=await capCalls();
await cap.fill('');await page.waitForFunction(el=>el.value==='',await cap.elementHandle());assert.equal(await cap.inputValue(),'');await cap.blur();await page.getByRole('alert').filter({hasText:'정수를 입력하세요'}).waitFor();assert.equal(await capCalls(),beforeCap);
await cap.fill('1.5');await cap.blur();assert.equal(await capCalls(),beforeCap);
await cap.fill('128001');await cap.blur();assert.equal(await capCalls(),beforeCap);
await cap.fill('64000');assert.equal(await capCalls(),beforeCap,'typing must not save each digit');await cap.press('Enter');
await page.waitForFunction(()=>fixture.preferences.preferences.commonContextTokenLimit===64000);
assert.equal(await capCalls(),beforeCap+1);
const capCommand=await page.evaluate(()=>fixture.calls.filter(c=>c.cmd==='save_console_preferences' && 'commonContextTokenLimit' in c.args.input.patch).at(-1).args.input);
assert.deepEqual(capCommand.patch,{commonContextTokenLimit:64000});assert.deepEqual(capCommand.expectedFieldRevisions,{commonContextTokenLimit:0});assert.equal(capCommand.schemaVersion,1);assert.equal(capCommand.target,'console_preferences');assert.equal(typeof capCommand.commandId,'string');assert.equal(typeof capCommand.idempotencyKey,'string');assert.ok(capCommand.commandId.length>0 && capCommand.idempotencyKey.length>0);
console.log('PASS controlled mounted budget draft invalid edits send no patch and Enter saves exact field CAS once');
await page.evaluate(()=>fixture.preferenceReplyLost=true);await cap.fill('76000');await cap.press('Enter');
await page.waitForFunction(()=>fixture.calls.filter(c=>c.cmd==='save_console_preferences' && c.args.input.patch.commonContextTokenLimit===76000).length===2);
await page.getByText('콘솔 설정을 저장하지 못했습니다. 입력은 유지했습니다. 충돌한 설정은 다시 확인해 주세요.',{exact:true}).waitFor();assert.equal(await cap.inputValue(),'76000');
await page.evaluate(()=>fixture.preferenceReplyLost=false);await cap.focus();await cap.press('Enter');
await page.waitForFunction(()=>fixture.calls.filter(c=>c.cmd==='save_console_preferences' && c.args.input.patch.commonContextTokenLimit===76000).length===3);
const recovered=await page.evaluate(()=>fixture.calls.filter(c=>c.cmd==='save_console_preferences' && c.args.input.patch.commonContextTokenLimit===76000).map(c=>c.args.input));
assert.deepEqual(recovered[1],recovered[0]);assert.deepEqual(recovered[2],recovered[0]);assert.deepEqual(recovered[0].expectedFieldRevisions,{commonContextTokenLimit:1});
assert.equal(await page.evaluate(()=>fixture.preferences.fieldRevisions.commonContextTokenLimit),2);
console.log('PASS controlled lost settings receipt keeps draft and explicit same-value retry reuses identical operation without duplicate commit');

await page.getByRole('button',{name:'모델 연결 관리',exact:true}).click();
await page.getByRole('button',{name:'원래 화면으로 돌아가기',exact:true}).click();
await page.getByRole('heading',{name:'콘솔 설정',exact:true}).waitFor();
await page.getByRole('button',{name:'원래 화면으로 돌아가기',exact:true}).click();
assert.equal(await page.locator('#question-draft').inputValue(),'Merged source retained question');
console.log('PASS settings direct connections entry and nested return preserve draft');
await page.getByRole('button',{name:'설정',exact:true}).first().click();assert.equal(await page.getByRole('spinbutton',{name:'공통 문맥 토큰 상한',exact:true}).inputValue(),'76000');await page.getByRole('button',{name:'원래 화면으로 돌아가기',exact:true}).click();console.log('PASS controlled saved common budget survives mounted settings reopen');

await page.evaluate(()=>fixture.validationReady=true);
await page.getByRole('button',{name:'모델 연결',exact:true}).click();
for(const [index,profileId] of ['p1','p2','p3'].entries()) {
 await page.getByRole('button',{name:'연결 확인',exact:true}).nth(index).click();
 await page.waitForFunction(id=>fixture.calls.some(c=>c.cmd==='refresh_provider_catalog' && c.args.profileId===id),profileId);
 await page.waitForFunction(()=>{const buttons=[...document.querySelectorAll('button')].filter(b=>b.textContent.trim()==='연결 확인');return buttons.length===3 && buttons.every(b=>!b.disabled);});
}
for(const [i,id] of ['MELCHIOR-1','BALTHASAR-2','CASPER-3'].entries()) {
 const row=page.locator('#core-'+id).locator('..');
 assert.equal(await row.locator('select').inputValue(),'p'+(i+1));
 await row.getByText('저장된 연결 ·Profile '+(i+1)+' · model'+i+' · default',{exact:true}).waitFor();
 assert.equal(await row.getByRole('status').count(),0);
}
const selectionsBefore=await page.evaluate(()=>fixture.calls.filter(c=>['select_provider_model','select_core_model'].includes(c.cmd)).length);
await page.evaluate(()=>{fixture.catalogs.p1.selectionState='stale';fixture.cores[0].selectionState='stale';});
await page.getByRole('button',{name:'저장 상태 다시 확인',exact:true}).click();
const staleRow=page.locator('#core-MELCHIOR-1').locator('..');
await staleRow.getByText('저장된 모델 선택을 다시 확인하십시오.',{exact:true}).waitFor();
await staleRow.getByText('저장된 연결 ·Profile 1 · model0 · default',{exact:true}).waitFor();
assert.equal(await staleRow.locator('select').inputValue(),'p1');
assert.equal(await page.evaluate(()=>fixture.calls.filter(c=>['select_provider_model','select_core_model'].includes(c.cmd)).length),selectionsBefore);
await page.getByRole('button',{name:'원래 화면으로 돌아가기',exact:true}).click();await page.getByRole('button',{name:'입력 확인',exact:true}).first().click();
assert.equal(await page.getByRole('button',{name:'이 동의로 심의 시작',exact:false}).isEnabled(),false);
await page.getByRole('button',{name:'ACP 프로필 확인',exact:true}).click();
await page.getByRole('button',{name:'Profile 1',exact:false}).first().click();
await page.locator('#live-acp-model').selectOption('model0');await page.locator('#connection-model-mode').selectOption('default');
await page.getByRole('button',{name:'모델 연결 저장',exact:true}).click();
await page.waitForFunction(()=>fixture.catalogs.p1.modelSelectionRevision===1);
await staleRow.getByRole('button',{name:'코어 연결 저장',exact:true}).click();
await page.waitForFunction(()=>fixture.cores[0].selectionRevision===1);
await page.waitForFunction(()=>!document.querySelector('#core-MELCHIOR-1').disabled);
await page.getByRole('button',{name:'원래 화면으로 돌아가기',exact:true}).click();
await page.getByRole('button',{name:'이 동의로 심의 시작',exact:false}).waitFor();
await page.getByText('모든 단계의 사전 예산 확인됨',{exact:true}).waitFor();
const explicit=await page.evaluate(()=>fixture.calls.filter(c=>['select_provider_model','select_core_model'].includes(c.cmd)).slice(-2));
assert.equal(explicit[0].args.input.modelId,'model0');assert.equal(explicit[0].args.input.modeId,'default');assert.equal(explicit[0].args.input.expectedSelectionRevision,0);assert.equal(explicit[1].args.input.expectedSelectionRevision,0);assert.equal(explicit[1].args.input.modelSelectionRevision,1);
console.log('PASS stale saved intent stays visible and blocked without automatic writes; explicit same model/mode and core CAS restore fresh proof');
await page.getByRole('button',{name:'MAGI COMMAND CONSOLE 홈으로 이동',exact:true}).click();await page.getByRole('button',{name:'새 심의 시작',exact:true}).click();
await page.evaluate(()=>fixture.deferBudgetRevision=fixture.preferences.fieldRevisions.commonContextTokenLimit);
await page.getByRole('button',{name:'입력 확인',exact:true}).first().click();
try { await page.waitForFunction(()=>typeof fixture.resolveBudget==='function'); }
catch(error) { console.log('BUDGET STAGE CONTROLLED STATE',await page.evaluate(()=>({heading:document.querySelector('#screen-title')?.textContent,status:[...document.querySelectorAll('[role=status],[role=alert],.field-help')].map(e=>e.textContent),commands:fixture.calls.filter(c=>['validate_provider_profile','authenticate_provider_profile','refresh_provider_catalog','load_core_execution_witnesses','preview_deliberation_budget'].includes(c.cmd)).map(c=>({cmd:c.cmd,profileId:c.args?.profileId,expectedBudgetRevision:c.args?.input?.expectedCommonContextBudgetRevision})),settingsRevision:fixture.preferences.fieldRevisions.commonContextTokenLimit})));throw error; }
console.log('CONTROLLED HELD POLICY',await page.evaluate(()=>({heldRevision:fixture.deferBudgetRevision,previewRequests:fixture.calls.filter(c=>c.cmd==='preview_deliberation_budget').map(c=>({revision:c.args.input.expectedCommonContextBudgetRevision,bindings:c.args.input.coreBindings.map(b=>({coreId:b.coreId,profileRevision:b.profileRevision,modelSelectionRevision:b.modelSelectionRevision,coreSelectionRevision:b.coreSelectionRevision}))})),pendingResponses:fixture.budgetResolvers?.length,status:[...document.querySelectorAll('[role=status]')].map(e=>e.textContent)})));
await page.getByText('단계별 전송 예산 확인 중',{exact:true}).waitFor();
assert.equal(await page.getByRole('button',{name:'이 동의로 심의 시작',exact:false}).isEnabled(),false);
await page.getByRole('button',{name:'설정',exact:true}).first().click();
await page.getByRole('spinbutton',{name:'공통 문맥 토큰 상한',exact:true}).fill('84000');await page.getByRole('spinbutton',{name:'공통 문맥 토큰 상한',exact:true}).press('Enter');
await page.waitForFunction(()=>fixture.preferences.fieldRevisions.commonContextTokenLimit===3);
await page.getByRole('button',{name:'원래 화면으로 돌아가기',exact:true}).click();
await page.getByText('모든 단계의 사전 예산 확인됨',{exact:true}).waitFor();
await page.evaluate(()=>fixture.resolveBudget());
await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
await page.locator('code').filter({hasText:'3'.repeat(64)}).waitFor();
assert.equal(await page.getByText('CONTROLLED OLD POLICY MUST NOT PUBLISH',{exact:true}).count(),0);
assert.equal(await page.locator('code').filter({hasText:'2'.repeat(64)}).count(),0);
const previews=await page.evaluate(()=>fixture.calls.filter(c=>c.cmd==='preview_deliberation_budget').map(c=>c.args.input));
const firstCurrent=previews.findIndex(p=>p.expectedCommonContextBudgetRevision===3);assert.ok(firstCurrent>0,'held old policy must precede new policy');assert.ok(previews.slice(0,firstCurrent).every(p=>p.expectedCommonContextBudgetRevision===2));assert.ok(previews.slice(firstCurrent).every(p=>p.expectedCommonContextBudgetRevision===3));
assert.equal(previews.some(p=>'commonContextTokenLimit' in p || 'tokenLimit' in p),false);
await page.getByText('모든 단계의 사전 예산 확인됨',{exact:true}).waitFor();
console.log('PASS native-confirmed budget CAS changes preview identity; late prior policy cannot publish and numeric cap is never request authority');
await page.getByRole('button',{name:'MAGI COMMAND CONSOLE 홈으로 이동',exact:true}).click();await page.getByRole('button',{name:'새 심의 시작',exact:true}).click();

await page.getByRole('button',{name:'모델 연결',exact:true}).click();
for(const [i,id] of ['MELCHIOR-1','BALTHASAR-2','CASPER-3'].entries()) await page.waitForFunction(({id,value})=>document.getElementById(id)?.value===value,{id:'core-'+id,value:'p'+(i+1)});
const edit=page.getByRole('button',{name:'편집',exact:true}).first();await edit.click();
const modal=page.getByRole('dialog');await modal.waitFor();await page.waitForFunction(()=>document.activeElement?.id==='acp-profile-alias');assert.equal(await modal.locator('#acp-profile-credential-home').inputValue(),'~/.codex');
await page.keyboard.press('Escape');await modal.waitFor({state:'hidden'});assert.equal(await edit.evaluate(e=>e===document.activeElement),true);assert.equal(await page.evaluate(()=>fixture.saveCalls),0);
console.log('PASS edit uses actual modal, populated manual home, initial focus, Escape and opener restoration without saving');
await edit.click();await modal.locator('#acp-profile-alias').fill('Edited manual profile');await page.evaluate(()=>fixture.saveMode='error');await modal.getByRole('button',{name:'프로필 저장',exact:true}).click();await modal.getByText('프로필을 저장하지 못했습니다. 입력 내용은 유지했습니다.',{exact:true}).waitFor();assert.equal(await modal.locator('#acp-profile-alias').inputValue(),'Edited manual profile');assert.equal(await modal.locator('#acp-profile-credential-home').inputValue(),'~/.codex');
console.log('PASS failed native-shaped save retains name and manual path');
await page.evaluate(()=>fixture.saveMode='success');await modal.getByRole('button',{name:'프로필 저장',exact:true}).click();await modal.waitFor({state:'hidden'});assert.equal(await page.evaluate(()=>fixture.profiles.find(p=>p.providerProfileId==='p1').revision),2);await page.getByRole('button',{name:'Edited manual profile',exact:false}).first().waitFor();
console.log('PASS successful edit revision update appears in saved list and closes modal');
await page.getByRole('button',{name:'새 ACP 프로필',exact:true}).click();await modal.locator('#acp-profile-alias').fill('Additional manual profile');await modal.locator('#acp-profile-credential-home').fill('~/.codex-extra');await modal.getByRole('button',{name:'취소',exact:true}).click();await modal.getByRole('button',{name:'변경 버리기',exact:true}).click();await modal.waitFor({state:'hidden'});assert.equal(await page.evaluate(()=>fixture.profiles.length),3);
await page.getByRole('button',{name:'새 ACP 프로필',exact:true}).click();await modal.locator('#acp-profile-alias').fill('Additional manual profile');await modal.locator('#acp-profile-credential-home').fill('~/.codex-extra');await modal.getByRole('button',{name:'프로필 저장',exact:true}).click();await modal.waitFor({state:'hidden'});assert.equal(await page.evaluate(()=>fixture.profiles.length),4);assert.equal(await page.evaluate(()=>fixture.lastDraft.credentialHomePath),'~/.codex-extra');
console.log('PASS add modal cancel discards; explicit manually entered home saves fourth independent profile');
for(const [i,id] of ['MELCHIOR-1','BALTHASAR-2','CASPER-3'].entries())assert.equal(await page.locator('#core-'+id).inputValue(),'p'+(i+1));
await page.getByRole('button',{name:'원래 화면으로 돌아가기',exact:true}).click();assert.equal(await page.locator('#question-draft').inputValue(),'Merged source retained question');assert.equal(await page.evaluate(()=>fixture.calls.filter(c=>c.cmd==='start_deliberation').length),0);
console.log('PASS connections return preserves question and independent saved core assignments; no inference submitted');
await page.getByRole('button',{name:'설정',exact:true}).click();await page.getByRole('combobox',{name:'콘솔 언어',exact:true}).selectOption('en');await page.waitForFunction(()=>document.documentElement.lang==='en');await page.waitForFunction(()=>fixture.preferences.preferences.language==='en');assert.equal(await page.getByRole('button',{name:'Manage model connections',exact:true}).count(),1);const saved=await page.evaluate(()=>fixture.calls.filter(c=>c.cmd==='save_console_preferences').at(-1).args.input);assert.deepEqual(saved.patch,{language:'en'});assert.deepEqual(saved.expectedFieldRevisions,{language:0});assert.equal(await page.locator('#question-draft').inputValue(),'Merged source retained question');console.log('PASS English display language saves field-specific authoritative CAS while preserving draft');
await page.goto(base+'?queue');await page.locator('.core-status strong').filter({hasText:'요청 종료 · 산출물 미확인'}).first().waitFor();assert.equal(await page.getByText('공개 산출물 저장됨',{exact:false}).count(),0);console.log('PASS native settled slot without result reference never renders accepted completion');
await page.evaluate(()=>{fixture.deferQueue=true;fixture.emit('magi:run-update',{runId:'queue-run',stage:'independent_review',state:'streaming',dispatchProjection:fixture.queue});});await page.waitForFunction(()=>typeof fixture.resolveQueue==='function');await page.evaluate(()=>{fixture.queue.runRevision=3;fixture.queue.projectionDigest='c'.repeat(64);fixture.queue.coreDispatches[0].resultRef='accepted-result';fixture.emit('magi:run-update',{runId:'queue-run',stage:'independent_review',state:'streaming',dispatchProjection:fixture.queue});});await page.locator('.core-status strong').filter({hasText:'공개 산출물 저장됨'}).first().waitFor();await page.evaluate(()=>{const old=structuredClone(fixture.queue);old.runRevision=1;old.coreDispatches[0].resultRef=null;fixture.resolveQueue(old);fixture.emit('magi:run-update',{runId:'other-run',stage:'independent_review',state:'streaming',dispatchProjection:{...old,runId:'other-run'}});});await page.waitForTimeout(50);assert.ok(await page.getByText('공개 산출물 저장됨',{exact:false}).count()>0);assert.equal(await page.getByText('요청 종료 · 산출물 미확인',{exact:false}).count(),0);console.log('PASS late older projection and different-run event cannot overwrite current accepted custody projection');
await page.evaluate(()=>fixture.emit('magi:run-update',{runId:'queue-run',stage:'independent_review',state:'streaming',dispatchProjection:{...fixture.queue,coreDispatches:[null]}}));await page.locator('.core-status strong').filter({hasText:'요청 상태 미확인'}).first().waitFor();assert.equal(await page.getByText('공개 산출물 저장됨',{exact:false}).count(),0);console.log('PASS malformed native slot payload fails closed to unknown without retaining fake completion');
await page.getByRole('button',{name:'진행 심의 기록 열기',exact:false}).click();await page.getByText('BASELINE DOSSIER',{exact:true}).waitFor();await page.evaluate(()=>fixture.deferDossier=true);await page.getByRole('button',{name:'저장 상태 새로고침',exact:true}).click();await page.waitForFunction(()=>typeof fixture.resolveDossier==='function');await page.evaluate(()=>{fixture.dossier.revision=3;fixture.dossier.generation=3;fixture.dossier.proposal.body='LATEST AUTHORITATIVE DOSSIER';fixture.emit('magi:run-update',{runId:'queue-run',stage:'independent_review',state:'completed',result:structuredClone(fixture.dossier)});});await page.getByText('LATEST AUTHORITATIVE DOSSIER',{exact:true}).waitFor();await page.evaluate(()=>{const old={...structuredClone(fixture.dossier),revision:2,generation:2,proposal:{...fixture.dossier.proposal,body:'STALE SAME RUN DOSSIER'}};fixture.resolveDossier(old);fixture.emit('magi:run-update',{runId:'queue-run',stage:'independent_review',state:'completed',result:{...old,revision:4,generation:1}});});await page.waitForTimeout(50);assert.equal(await page.getByText('STALE SAME RUN DOSSIER',{exact:true}).count(),0);await page.getByText('LATEST AUTHORITATIVE DOSSIER',{exact:true}).waitFor();console.log('PASS late same-run readonly response and lower-generation result cannot overwrite latest dossier publication');
await page.evaluate(()=>{fixture.deferDossier=true;fixture.resolveDossier=null;});await page.getByRole('button',{name:/Other question/}).click();await page.waitForFunction(()=>typeof fixture.resolveDossier==='function');await page.getByRole('button',{name:/Queue question/}).click();await page.getByText('LATEST AUTHORITATIVE DOSSIER',{exact:true}).waitFor();await page.evaluate(()=>fixture.resolveDossier({...structuredClone(fixture.dossier),runId:'other-run',revision:9,generation:9,proposal:{...fixture.dossier.proposal,body:'STALE OTHER RUN DOSSIER'}}));await page.waitForTimeout(50);assert.equal(await page.getByText('STALE OTHER RUN DOSSIER',{exact:true}).count(),0);await page.getByText('LATEST AUTHORITATIVE DOSSIER',{exact:true}).waitFor();console.log('PASS obsolete different-run history response cannot publish into returned current selection');
await page.goto(base+'?queue&companion');await page.locator('.core-status strong').filter({hasText:'요청 종료 · 산출물 미확인'}).first().waitFor();const subscription=await page.evaluate(()=>fixture.calls.findIndex(call=>call.cmd==='plugin:event|listen'&&call.args.event==='magi:core-dispatches'));const initialRead=await page.evaluate(()=>fixture.calls.findIndex(call=>call.cmd==='load_run_core_dispatches'));assert.ok(subscription>=0&&initialRead>subscription);await page.evaluate(()=>{fixture.queue.coreDispatches[0].resultRef='companion-accepted';fixture.queue.projectionDigest='d'.repeat(64);fixture.emit('magi:core-dispatches',fixture.queue);});await page.locator('.core-status strong').filter({hasText:'공개 산출물 저장됨'}).first().waitFor();const reads=await page.evaluate(()=>fixture.calls.filter(call=>call.cmd==='load_run_core_dispatches').length);await page.evaluate(()=>fixture.emit('magi:core-dispatches',{...fixture.queue,runId:'other-run'}));await page.waitForTimeout(50);assert.equal(await page.evaluate(()=>fixture.calls.filter(call=>call.cmd==='load_run_core_dispatches').length),reads);assert.equal(await page.evaluate(()=>fixture.calls.some(call=>call.cmd==='start_deliberation'||call.cmd==='start_live_run')),false);console.log('PASS companion subscribes before native readonly read, consumes same-run metadata transitions, and ignores unpinned run without inference');
await page.goto(base+'?manual-initial');
await page.getByRole('button',{name:'모델 연결',exact:true}).click();
for(const id of ['p1','p2','p3']) {
 const profileRow=page.locator(`li[data-profile-id="${id}"]`);
 await profileRow.getByRole('button',{name:'연결 확인',exact:true}).click();
 await profileRow.getByText('기존 구독 확인됨',{exact:false}).waitFor();
}
assert.equal(await page.evaluate(()=>fixture.calls.filter(c=>c.cmd==='authenticate_provider_profile').length),3);
const manualCore=page.locator('#core-MELCHIOR-1').locator('..');
await manualCore.locator('select').selectOption('p1');
await manualCore.getByRole('button',{name:'코어 연결 저장',exact:true}).click();
await page.waitForFunction(()=>fixture.cores[0].selection?.providerProfileId==='p1'&&!document.querySelector('#core-MELCHIOR-1').disabled);
await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
assert.equal(await page.evaluate(()=>fixture.calls.filter(c=>c.cmd==='authenticate_provider_profile').length),3);
await page.getByRole('button',{name:'저장 상태 다시 확인',exact:true}).click();
await page.waitForFunction(()=>!document.querySelector('#core-MELCHIOR-1').disabled);
await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
assert.equal(await page.evaluate(()=>fixture.calls.filter(c=>c.cmd==='authenticate_provider_profile').length),3);
const revisionRow=page.locator('li[data-profile-id="p1"]');
await revisionRow.getByRole('button',{name:'편집',exact:true}).click();
await page.locator('#acp-profile-alias').fill('Revised Profile 1');
await page.getByRole('button',{name:'프로필 저장',exact:true}).click();
await page.waitForFunction(()=>fixture.calls.some(c=>c.cmd==='authenticate_provider_profile'&&c.args.profileId==='p1'&&c.args.expectedRevision===2));
await page.locator('li[data-profile-id="p1"]').getByText('기존 구독 확인됨',{exact:false}).waitFor();
assert.equal(await page.evaluate(()=>fixture.calls.filter(c=>c.cmd==='authenticate_provider_profile').length),4);
await page.goto(base+'?terminal=completed');
await page.waitForFunction(()=>fixture.calls.filter(c=>c.cmd==='authenticate_provider_profile').length===3);
await page.getByRole('button',{name:'모델 연결',exact:true}).click();
for(const id of ['p1','p2','p3'])await page.locator(`li[data-profile-id="${id}"]`).getByText('기존 구독 확인됨',{exact:false}).waitFor();
assert.equal(await page.evaluate(()=>fixture.calls.some(c=>['select_provider_model','select_core_model'].includes(c.cmd))),false);
console.log('PASS manual initial checks survive explicit core save and reload without duplicate automatic authentication; new profile revision and restart still verify automatically');
for(const status of ['completed','cancelled','failed']) {
 await page.goto(base+'?terminal='+status);
 await page.getByRole('button',{name:'새 심의 시작',exact:true}).waitFor();
 const previous=await page.evaluate(()=>JSON.stringify(fixture.dossier));
 await page.getByRole('button',{name:'새 심의 시작',exact:true}).click();
 const draft=page.locator('#question-draft');assert.equal(await draft.getAttribute('readonly'),null);
 await draft.fill('New question after '+status);
 await page.evaluate(()=>{const result={...structuredClone(fixture.dossier),revision:3,generation:3};fixture.emit('magi:run-update',{runId:result.runId,stage:result.stage,state:result.status,result});});
 await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
 assert.equal(await draft.inputValue(),'New question after '+status);assert.equal(await draft.getAttribute('readonly'),null);
 await page.getByRole('button',{name:'모델 연결',exact:true}).click();
 await page.getByRole('heading',{name:'모델 연결',exact:true}).waitFor();
 assert.equal((await page.locator('.agenda-primary').textContent()).replace('↗','').trim(),'입력 확인');assert.equal(await page.locator('.agenda-primary').isEnabled(),false);
 assert.equal(await draft.inputValue(),'New question after '+status);
 assert.equal(await page.locator('.agenda-source-status span').last().textContent(),'자료 0개');
 await page.evaluate(()=>{const result={...structuredClone(fixture.dossier),revision:4,generation:4};fixture.emit('magi:run-update',{runId:result.runId,stage:result.stage,state:result.status,result});});
 await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
 await page.getByRole('heading',{name:'모델 연결',exact:true}).waitFor();
 assert.equal((await page.locator('.agenda-primary').textContent()).replace('↗','').trim(),'입력 확인');assert.equal(await page.locator('.agenda-primary').isEnabled(),false);
 assert.equal(await draft.inputValue(),'New question after '+status);
 await page.getByRole('button',{name:'원래 화면으로 돌아가기',exact:true}).click();
 assert.equal(await draft.getAttribute('readonly'),null);
 await page.getByRole('button',{name:'설정',exact:true}).click();
 await page.getByRole('heading',{name:'콘솔 설정',exact:true}).waitFor();
 assert.equal((await page.locator('.agenda-primary').textContent()).replace('↗','').trim(),'입력 확인');assert.equal(await page.locator('.agenda-primary').isEnabled(),false);
 assert.equal(await draft.inputValue(),'New question after '+status);
 await page.evaluate(()=>{const result={...structuredClone(fixture.dossier),revision:5,generation:5};fixture.emit('magi:run-update',{runId:result.runId,stage:result.stage,state:result.status,result});});
 await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
 await page.getByRole('heading',{name:'콘솔 설정',exact:true}).waitFor();
 assert.equal((await page.locator('.agenda-primary').textContent()).replace('↗','').trim(),'입력 확인');assert.equal(await page.locator('.agenda-primary').isEnabled(),false);
 assert.equal(await draft.inputValue(),'New question after '+status);
 await page.getByRole('button',{name:'원래 화면으로 돌아가기',exact:true}).click();
 assert.equal(await draft.inputValue(),'New question after '+status);assert.equal(await draft.getAttribute('readonly'),null);
 await page.getByRole('button',{name:'자료 보기',exact:true}).click();
 assert.equal((await page.locator('.agenda-primary').textContent()).replace('↗','').trim(),'입력 확인');assert.equal(await page.locator('.agenda-primary').isEnabled(),false);
 await page.getByRole('button',{name:'입력 확인',exact:false}).last().click();
 await page.getByRole('heading',{name:'입력·전송 확인',exact:true}).waitFor();
 assert.equal((await page.locator('.agenda-primary').textContent()).replace('↗','').trim(),'입력 확인');assert.equal(await page.locator('.agenda-primary').isEnabled(),false);
 await page.evaluate(()=>{const result={...structuredClone(fixture.dossier),revision:6,generation:6};fixture.emit('magi:run-update',{runId:result.runId,stage:result.stage,state:result.status,result});});
 await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
 await page.getByRole('heading',{name:'입력·전송 확인',exact:true}).waitFor();
 assert.equal((await page.locator('.agenda-primary').textContent()).replace('↗','').trim(),'입력 확인');assert.equal(await page.locator('.agenda-primary').isEnabled(),false);
 assert.equal(await page.locator('#question-draft').inputValue(),'New question after '+status);
 assert.equal(await page.evaluate(()=>fixture.calls.some(c=>['register_deliberation_request','start_deliberation','select_provider_model','select_core_model'].includes(c.cmd))),false);
 await page.getByText('모든 단계의 사전 예산 확인됨',{exact:true}).waitFor();
 await page.getByRole('checkbox').check();
 await page.getByRole('button',{name:'이 동의로 심의 시작',exact:false}).click();
 await page.waitForFunction(()=>fixture.started);
 const submitted=await page.evaluate(()=>fixture.started.input);
 assert.equal(submitted.question,'New question after '+status);assert.equal(submitted.coreBindings.length,3);assert.equal(submitted.expectedCommonContextBudgetRevision,0);
 assert.equal(await page.evaluate(()=>JSON.stringify(fixture.dossier)),previous);
 assert.equal(await page.evaluate(()=>fixture.calls.some(c=>['select_provider_model','select_core_model'].includes(c.cmd))),false);
}
await page.goto(base+'?queue');await page.getByRole('button',{name:'진행 심의 기록 열기',exact:false}).waitFor();assert.equal(await page.getByRole('button',{name:'새 심의 시작',exact:true}).count(),0);
await page.getByRole('button',{name:'진행 심의 기록 열기',exact:false}).click();assert.notEqual(await page.locator('#question-draft').getAttribute('readonly'),null);assert.equal((await page.locator('.agenda-primary').textContent()).replace('↗','').trim(),'진행 중');assert.equal(await page.locator('.agenda-primary').isEnabled(),false);assert.equal(await page.evaluate(()=>fixture.calls.some(c=>c.cmd==='register_deliberation_request'||c.cmd==='start_deliberation')),false);
console.log('PASS terminal records preserve immutable dossier while new editable draft reaches exact three-binding admission; ongoing run blocks new entry');
assert.deepEqual(pageErrors,[]);
} finally {try {await page.evaluate(()=>{for(const release of window.fixture?.budgetResolvers?.splice(0)??[])release();});} finally {await page.close();}}
}
