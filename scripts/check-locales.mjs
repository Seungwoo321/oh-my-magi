import fs from 'node:fs';
import ts from 'typescript-parser';
const messages = JSON.parse(fs.readFileSync('src/locales/messages.json', 'utf8'));
const english = JSON.parse(fs.readFileSync('src/locales/en.json', 'utf8'));
const errors = [];
const sourceKeys = new Set();
for (const file of ['src/App.tsx', 'src/screens.tsx', 'src/records.tsx', 'src/core-bindings.tsx', 'src/native-settings.tsx', 'src/context-preview.tsx', 'src/role-transfer.tsx', 'src/source-freshness.tsx', 'src/record-actions.tsx', 'src/budget-preview.tsx', 'src/pdf-range-capture.tsx', 'src/store-selection.tsx']) {
  const tree = ts.createSourceFile(file, fs.readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const visit = node => {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 't' && node.arguments[0] && ts.isStringLiteral(node.arguments[0])) sourceKeys.add(node.arguments[0].text);
    if (ts.isJsxText(node) && /[가-힣]/.test(node.text)) errors.push(`${file}: untranslated JSX text`);
    ts.forEachChild(node, visit);
  };
  visit(tree);
}
const placeholders = text => [...text.matchAll(/\{\d+\}/g)].map(match => match[0]).sort().join(',');
for (const key of sourceKeys) if (!messages.includes(key)) errors.push(`Missing source inventory: ${key}`);
for (const key of messages) {
  const value = english[key];
  if (typeof value !== 'string' || value.trim() === '') errors.push(`Missing English translation: ${key}`);
  else {
    if (/[가-힣]/.test(value)) errors.push(`Korean remains in English translation: ${key}`);
    if (placeholders(key) !== placeholders(value)) errors.push(`Placeholder mismatch: ${key}`);
  }
}
if (errors.length) {
  process.stderr.write(`${errors.length} locale contract violations\n${errors.slice(0, 12).join('\n')}\n`);
  process.exitCode = 1;
} else process.stdout.write(`${messages.length} English messages: complete coverage, exact placeholders, no untranslated JSX\n`);
