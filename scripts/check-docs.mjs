#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
if (args.includes('--help')) {
  console.log('Usage: node scripts/check-docs.mjs [--root PATH]');
  process.exit(0);
}
if (args.length !== 0 && (args.length !== 2 || args[0] !== '--root')) {
  console.error('Expected no arguments or --root PATH.');
  process.exit(2);
}
const root = path.resolve(args[1] ?? fileURLToPath(new URL('../', import.meta.url)));
const errors = [];
const documents = new Map();
const counters = { markdown: 0, localLinks: 0, remoteLinks: 0, jsonFences: 0, requirements: 0, flows: 0, scenarios: 0 };
const skipped = new Set(['tmp', 'node_modules', 'target', 'dist', 'build', 'coverage']);
const decoder = new TextDecoder('utf-8', { fatal: true });
const relative = (file) => path.relative(root, file).split(path.sep).join('/');
const fail = (file, line, message) => errors.push(`${relative(file)}:${line}: ${message}`);
const mask = (text) => text.replace(/[^\n]/g, ' ');
const lineAt = (text, index) => text.slice(0, index).split('\n').length;
const identifiers = (text, prefix) => [...text.matchAll(new RegExp(`\\b${prefix}\\d{2,}\\b`, 'g'))].map((match) => match[0]);

function walk(directory) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (entry.name.startsWith('.') || skipped.has(entry.name) || entry.isSymbolicLink()) continue;
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) walk(file);
    else if (entry.isFile() && /\.md$/i.test(entry.name)) readDocument(file);
  }
}

function decodeEntities(text) {
  return text.replace(/&#(x[\da-f]+|\d+);|&(amp|lt|gt|quot|apos);/gi, (whole, number, named) => {
    if (named) return { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'" }[named.toLowerCase()];
    const code = number.toLowerCase().startsWith('x') ? Number.parseInt(number.slice(1), 16) : Number(number);
    return code <= 0x10ffff ? String.fromCodePoint(code) : whole;
  });
}

function headingSlug(text) {
  const plain = decodeEntities(text.replace(/<[^>]*>/g, '').replace(/!?\[([^\]]+)\]\([^)]*\)/g, '$1').replace(/\\([\p{P}\p{S}])/gu, '$1'));
  return plain.toLowerCase().replace(/[^\p{L}\p{M}\p{N}_\-\s]/gu, '').replace(/\s/g, '-');
}

function readDocument(file) {
  let text;
  try { text = decoder.decode(fs.readFileSync(file)).replace(/^\uFEFF/, ''); }
  catch { fail(file, 1, 'Document is unreadable or invalid UTF-8.'); return; }
  const lines = text.split('\n');
  const visible = [...lines];
  let fence = null;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (fence) {
      visible[index] = mask(line);
      const close = line.match(/^ {0,3}(`{3,}|~{3,})\s*$/);
      if (close && close[1][0] === fence.marker[0] && close[1].length >= fence.marker.length) {
        if (fence.language === 'json') {
          counters.jsonFences += 1;
          try { JSON.parse(lines.slice(fence.start + 1, index).join('\n')); }
          catch { fail(file, fence.start + 1, 'Invalid JSON fence.'); }
        }
        fence = null;
      }
    } else {
      const open = line.match(/^ {0,3}(`{3,}|~{3,})([^\n]*)$/);
      if (open) {
        fence = { marker: open[1], language: open[2].trim().split(/\s/)[0].toLowerCase(), start: index };
        visible[index] = mask(line);
      }
    }
  }
  if (fence) fail(file, fence.start + 1, 'Unclosed fenced code block.');
  const prose = visible.join('\n').replace(/<!--[\s\S]*?-->/g, mask);
  const anchors = new Set();
  const explicit = new Set();
  for (const match of prose.matchAll(/<[a-z][^>]*\s(?:id|name)\s*=\s*(["'])(.*?)\1[^>]*>/gi)) {
    const id = decodeEntities(match[2]);
    if (explicit.has(id)) fail(file, lineAt(prose, match.index), `Duplicate explicit anchor: ${id}`);
    explicit.add(id);
    anchors.add(id);
  }
  const headingIds = new Set();
  const headings = [];
  const proseLines = prose.split('\n');
  for (let index = 0; index < proseLines.length; index += 1) {
    const match = proseLines[index].match(/^ {0,3}(#{1,6})(?:\s+|$)(.*?)(?:\s+#+\s*)?$/);
    const setext = index > 0 && /^ {0,3}(?:=+|-+)\s*$/.test(proseLines[index]) && proseLines[index - 1].trim() && !/^\s*[|>#*-]/.test(proseLines[index - 1]);
    if (!match && !setext) continue;
    const heading = match ? match[2] : proseLines[index - 1].trim();
    const level = match ? match[1].length : proseLines[index].trim()[0] === '=' ? 1 : 2;
    const base = headingSlug(heading);
    let slug = base;
    for (let count = 1; headingIds.has(slug); count += 1) slug = `${base}-${count}`;
    headingIds.add(slug);
    anchors.add(slug);
    headings.push({ text: heading, level, line: match ? index + 1 : index });
  }
  const links = prose.replace(/(`+)([\s\S]*?)\1/g, mask);
  documents.set(file, { text, prose, links, anchors, headings });
  counters.markdown += 1;
}

function destination(raw) {
  const text = raw.trim();
  if (text.startsWith('<')) return text.slice(1, text.indexOf('>'));
  return (text.match(/^(?:\\.|[^\s])*/)?.[0] ?? '').replace(/\\([\p{P}\p{S}])/gu, '$1');
}

function checkLink(file, line, raw) {
  const target = decodeEntities(raw);
  if (/^(?:[a-z][a-z\d+.-]*:|\/\/)/i.test(target) && !/^file:/i.test(target)) {
    counters.remoteLinks += 1;
    if (/^https?:/i.test(target)) {
      try { new URL(target); } catch { fail(file, line, `Invalid remote URL: ${target}`); }
    }
    return;
  }
  counters.localLinks += 1;
  const hash = target.indexOf('#');
  let pathname;
  let fragment;
  try {
    pathname = decodeURIComponent((hash < 0 ? target : target.slice(0, hash)).split('?')[0]);
    fragment = hash < 0 ? '' : decodeURIComponent(target.slice(hash + 1));
  } catch { fail(file, line, `Invalid percent encoding in link: ${target}`); return; }
  const resolved = pathname ? path.resolve(path.dirname(file), pathname) : file;
  const inRoot = (value) => value === root || value.startsWith(`${root}${path.sep}`);
  if (/^file:/i.test(target) || path.isAbsolute(pathname) || !inRoot(resolved)) {
    fail(file, line, `Local link must stay inside the repository: ${target}`); return;
  }
  let stat;
  try {
    stat = fs.statSync(resolved);
    if (!inRoot(fs.realpathSync(resolved))) throw new Error('outside root');
  } catch { fail(file, line, `Missing or inaccessible local target: ${target}`); return; }
  if (!fragment) return;
  if (stat.isDirectory()) { fail(file, line, `Cannot resolve fragment on directory: ${target}`); return; }
  if (/\.md$/i.test(resolved)) {
    if (!documents.has(resolved)) readDocument(resolved);
    if (!documents.get(resolved)?.anchors.has(fragment)) fail(file, line, `Missing Markdown anchor: ${target}`);
  } else if (/\.(?:svg|html?)$/i.test(resolved)) {
    const content = fs.readFileSync(resolved, 'utf8');
    const ids = [...content.matchAll(/\bid\s*=\s*(["'])(.*?)\1/g)].map((match) => decodeEntities(match[2]));
    if (!ids.includes(fragment)) fail(file, line, `Missing asset anchor: ${target}`);
  } else fail(file, line, `Fragment target type is not supported: ${target}`);
}

function checkLinks(file, document) {
  const normalizeLabel = (label) => label.trim().replace(/\s+/g, ' ').toLowerCase();
  const references = new Map();
  let prose = document.links;
  prose = prose.replace(/^ {0,3}\[([^\]]+)\]:\s*(<[^>]+>|\S+)(?:[^\n]*)$/gm, (whole, label, target, index) => {
    const key = normalizeLabel(label);
    if (references.has(key)) fail(file, lineAt(prose, index), `Duplicate link definition: ${label}`);
    references.set(key, destination(target));
    checkLink(file, lineAt(prose, index), destination(target));
    return mask(whole);
  });
  for (let index = 0; index < prose.length; index += 1) {
    if (prose[index] !== '[' || prose[index - 1] === '\\') continue;
    let depth = 1;
    let end = index + 1;
    for (; end < prose.length && depth; end += 1) {
      if (prose[end] === '\\') { end += 1; continue; }
      if (prose[end] === '[') depth += 1;
      if (prose[end] === ']') depth -= 1;
    }
    if (depth) continue;
    const label = prose.slice(index + 1, end - 1);
    if (prose[end] === '(') {
      let nested = 1;
      let close = end + 1;
      for (; close < prose.length && nested; close += 1) {
        if (prose[close] === '\\') { close += 1; continue; }
        if (prose[close] === '(') nested += 1;
        if (prose[close] === ')') nested -= 1;
      }
      if (nested) fail(file, lineAt(prose, index), 'Unclosed inline link destination.');
      else checkLink(file, lineAt(prose, index), destination(prose.slice(end + 1, close - 1)));
      index = close - 1;
    } else if (prose[end] === '[') {
      const close = prose.indexOf(']', end + 1);
      if (close < 0) continue;
      const ref = normalizeLabel(prose.slice(end + 1, close) || label);
      if (!references.has(ref)) fail(file, lineAt(prose, index), `Undefined reference link: ${ref}`);
      index = close;
    } else if (references.has(normalizeLabel(label))) index = end - 1;
  }
  for (const match of prose.matchAll(/<(?:a|img)\b[^>]*\b(?:href|src)\s*=\s*(["'])(.*?)\1[^>]*>/gi)) {
    checkLink(file, lineAt(prose, match.index), match[2]);
  }
}

function tableDefinitions(file, prefix) {
  const document = documents.get(file);
  const found = new Map();
  if (!document) { fail(file, 1, 'Required specification document is missing.'); return found; }
  for (const [index, line] of document.prose.split('\n').entries()) {
    const match = line.match(new RegExp(`^\\|\\s*\x60?(${prefix}\\d{2,})\\b`));
    if (!match) continue;
    if (found.has(match[1])) fail(file, index + 1, `Duplicate ${prefix} definition: ${match[1]}`);
    found.set(match[1], index + 1);
  }
  if (!found.size) fail(file, 1, `No ${prefix} definitions found in the first column of a table.`);
  return found;
}

function checkCoverage() {
  const designPath = path.join(root, 'docs', 'DESIGN.md');
  const scenarioPath = path.join(root, 'docs', 'SCENARIOS.md');
  const requirements = tableDefinitions(designPath, 'R');
  const flows = tableDefinitions(designPath, 'F');
  const document = documents.get(scenarioPath);
  counters.requirements = requirements.size;
  counters.flows = flows.size;
  if (!document) { fail(scenarioPath, 1, 'Required scenario document is missing.'); return; }
  const coveredRequirements = new Set();
  const coveredFlows = new Set();
  const scenarioIds = new Set();
  const lines = document.prose.split('\n');
  for (const [index, heading] of document.headings.entries()) {
    const match = heading.text.replace(/[`*]/g, '').match(/^(S\d{2,})\b/);
    if (!match) continue;
    const id = match[1];
    if (scenarioIds.has(id)) fail(scenarioPath, heading.line, `Duplicate scenario: ${id}`);
    scenarioIds.add(id);
    const next = document.headings.slice(index + 1).find((candidate) => candidate.level <= heading.level);
    const body = lines.slice(heading.line, next ? next.line - 1 : lines.length).map((line) => line.replace(/\*\*/g, '').replace(/^\s*[-*+]\s+/, '').trim());
    for (const label of ['요구사항', '흐름', '렌즈', 'Given', 'When', 'Then', '검증']) {
      const values = body.filter((line) => new RegExp(`^${label}\\s*:`).test(line)).map((line) => line.slice(line.indexOf(':') + 1).trim());
      if (values.length !== 1 || !values[0]) { fail(scenarioPath, heading.line, `${id}: exactly one nonempty ${label} line is required.`); continue; }
      if (label === '요구사항' || label === '흐름') {
        const prefix = label === '요구사항' ? 'R' : 'F';
        const ids = identifiers(values[0], prefix);
        if (!ids.length) fail(scenarioPath, heading.line, `${id}: ${label} must reference at least one ${prefix} ID.`);
        for (const ref of ids) (prefix === 'R' ? coveredRequirements : coveredFlows).add(ref);
      }
    }
  }
  counters.scenarios = scenarioIds.size;
  if (!scenarioIds.size) fail(scenarioPath, 1, 'No Sxx scenario headings found.');
  for (const [id, line] of requirements) if (!coveredRequirements.has(id)) fail(designPath, line, `${id} is not covered by scenario metadata.`);
  for (const [id, line] of flows) if (!coveredFlows.has(id)) fail(designPath, line, `${id} is not covered by scenario metadata.`);
  for (const [file, item] of documents) {
    for (const [index, line] of item.prose.split('\n').entries()) {
      for (const id of identifiers(line, 'R')) if (!requirements.has(id)) fail(file, index + 1, `Undefined requirement reference: ${id}`);
      for (const id of identifiers(line, 'F')) if (!flows.has(id)) fail(file, index + 1, `Undefined flow reference: ${id}`);
    }
  }
}

try {
  walk(root);
  for (const [file, document] of documents) checkLinks(file, document);
  checkCoverage();
} catch (error) {
  errors.push(`Checker could not complete: ${error.message}`);
}
for (const error of errors) console.error(error);
console.log(JSON.stringify({ result: errors.length ? 'failed' : 'passed', checks: counters, errors: errors.length, scope: 'Document structure only; remote availability and product behavior are not verified.' }, null, 2));
process.exitCode = errors.length ? 1 : 0;
