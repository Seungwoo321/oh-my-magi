#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(fileURLToPath(new URL('../', import.meta.url)));
const source = path.join(root, 'docs/visuals/console');
const contract = JSON.parse(fs.readFileSync(path.join(source, 'design-contract.json'), 'utf8'));
const errors = [];
const report = { result: 'unproven', scripts: 0, surfaces: 0, variants: 0, rendered: 0, navigationTargets: 0, journeys: contract.journeys.length, errors };
const document = { addEventListener() {}, querySelector() { return null; }, querySelectorAll() { return []; }, getElementById() { return null; } };
const context = vm.createContext({ window: {}, document, console, URLSearchParams, setTimeout() {}, clearTimeout() {}, setInterval() {}, clearInterval() {}, structuredClone });
const files = ['ui-kit.js', 'runtime.js', 'setup.js', 'records.js'];
for (const file of [...files, 'app.js']) {
  try {
    const code = fs.readFileSync(path.join(source, file), 'utf8');
    const compiled = new vm.Script(code, { filename: file });
    if (files.includes(file)) compiled.runInContext(context, { timeout: 3000 });
    report.scripts++;
  } catch (error) { errors.push({ file, message: error.message }); }
}

const views = context.window.MagiScreens || {};
const kit = context.window.MagiKit;
const rendered = [];
const declared = new Map(contract.surfaces.map(item => [item.id, item]));
for (const entry of contract.surfaces) {
  const view = views[entry.id];
  if (!view) { errors.push({ screen: entry.id, message: 'Required surface is not registered.' }); continue; }
  const variantIds = (view.variants || []).map(item => item.id);
  if (new Set(variantIds).size !== variantIds.length) errors.push({ screen: entry.id, message: 'Duplicate variant IDs.' });
  if (typeof view.render !== 'function' || !view.title || !view.group) errors.push({ screen: entry.id, message: 'Missing render function, title or group.' });
  if ((view.profile || 'desktop') !== entry.profile) errors.push({ screen: entry.id, message: 'Window profile does not match design contract.' });
  for (const variant of entry.variants) if (!variantIds.includes(variant)) errors.push({ screen: entry.id, variant, message: 'Required variant is missing.' });
}
for (const [id, view] of Object.entries(views)) {
  report.surfaces++;
  if (!declared.has(id)) errors.push({ screen: id, message: 'Registered surface has no design contract.' });
  for (const { id: variant } of view.variants || []) {
    report.variants++;
    if (!declared.get(id)?.variants.includes(variant)) errors.push({ screen: id, variant, message: 'Registered variant has no design contract.' });
    try {
      const formState = {};
      const setup = context.window.MagiSetup.state({ formState });
      const sources = context.window.MagiSetup.selectedSources(setup);
      const html = view.render({
        variant, screen: id, question: '새 기능을 제한 공개할까요?',
        escape: kit.esc, topology: kit.topology,
        sources, sourceCount: sources.length, roles: setup.roles, runStage: 'input', isRunActive: false,
        formState, go() {}, notify() {}, openDialog() {}, closeDialog() {},
      });
      if (typeof html !== 'string' || !html.trim()) throw new Error('Renderer produced no HTML.');
      if (/Oh My MAGI/i.test(html)) throw new Error('Public project branding leaked into console.');
      if (/\b(?:undefined|NaN)\b/.test(html)) throw new Error('Unresolved value in rendered output.');
      report.rendered++;
      rendered.push({ id, variant, html });
    } catch (error) { errors.push({ screen: id, variant, message: error.message }); }
  }
}
const attribute = (tag, name) => tag.match(new RegExp(name + '=["\\\']([^"\\\']+)["\\\']'))?.[1];
for (const { id, variant, html } of rendered) {
  for (const match of html.matchAll(/<[^>]+\b(?:data-go|data-submit-screen)=["'][^"']+["'][^>]*>/g)) {
    const target = attribute(match[0], 'data-go') || attribute(match[0], 'data-submit-screen');
    const targetVariant = attribute(match[0], 'data-variant') || attribute(match[0], 'data-submit-variant') || 'default';
    report.navigationTargets++;
    if (!views[target]) errors.push({ screen: id, variant, target, message: 'Navigation target is not registered.' });
    else if (targetVariant !== 'default' && !views[target].variants.some(item => item.id === targetVariant)) errors.push({ screen: id, variant, target, targetVariant, message: 'Navigation variant is not registered.' });
  }
}
for (const journey of contract.journeys) for (const id of journey.routes) {
  if (!declared.has(id) || !views[id]) errors.push({ journey: journey.id, screen: id, message: 'Journey references an undefined surface.' });
}
report.result = errors.length ? 'failed' : 'passed';
report.scope = 'Design registry, render functions and navigation references. Native behavior and visual quality require independent browser and macOS evidence.';
console.log(JSON.stringify(report, null, 2));
process.exitCode = errors.length ? 1 : 0;
