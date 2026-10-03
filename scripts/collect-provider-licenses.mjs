import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { pathToFileURL } from 'node:url';

export function collectProviderLicenses(source) {
  const root = fs.realpathSync(source);
  const inventory = [];
  const sections = [];
  const seen = new Map();
  let total = 0;
  const read = file => {
    const stat = fs.lstatSync(file);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 2 * 1024 * 1024) throw new Error('Provider license input is not a bounded regular file.');
    const bytes = fs.readFileSync(file);
    total += bytes.length;
    if (total > 32 * 1024 * 1024) throw new Error('Provider license inventory exceeds its bound.');
    return bytes;
  };
  const visit = directory => {
    if (!fs.lstatSync(directory).isDirectory()) throw new Error('Provider package directory is not regular.');
    const packageFile = path.join(directory, 'package.json');
    if (!fs.existsSync(packageFile)) throw new Error('Provider package metadata is missing.');
    const metadata = JSON.parse(read(packageFile));
    if (typeof metadata.name !== 'string' || typeof metadata.version !== 'string') throw new Error('Provider package identity is invalid.');
    const files = [];
    for (const entry of fs.readdirSync(directory).sort()) {
      if (!/^(license|licence|notice|copying)([._-].*)?$/i.test(entry)) continue;
      const bytes = read(path.join(directory, entry));
      files.push({ file: entry, sha256: crypto.createHash('sha256').update(bytes).digest('hex'), text: bytes.toString('utf8') });
    }
    const key = `${metadata.name}@${metadata.version}`;
    const record = { package: metadata.name, version: metadata.version, license: metadata.license ?? null, files };
    const encoded = JSON.stringify(record);
    if (seen.has(key) && seen.get(key) !== encoded) throw new Error('Conflicting provider package license inventory.');
    if (!seen.has(key)) {
      if (seen.size >= 5000) throw new Error('Provider package count exceeds its bound.');
      seen.set(key, encoded); inventory.push(record);
    }
    const dependencies = path.join(directory, 'node_modules');
    if (!fs.existsSync(dependencies)) return;
    if (!fs.lstatSync(dependencies).isDirectory()) throw new Error('Provider dependency directory is unsafe.');
    for (const name of fs.readdirSync(dependencies).sort()) {
      if (name.startsWith('.')) continue;
      const child = path.join(dependencies, name);
      if (fs.lstatSync(child).isSymbolicLink()) throw new Error('Provider dependency links are unsupported.');
      if (name.startsWith('@')) {
        for (const packageName of fs.readdirSync(child).sort()) visit(path.join(child, packageName));
      } else visit(child);
    }
  };
  visit(root);
  inventory.sort((a, b) => `${a.package}@${a.version}`.localeCompare(`${b.package}@${b.version}`));
  for (const record of inventory) {
    sections.push(`\nPackage: ${record.package}@${record.version}\nDeclared license: ${JSON.stringify(record.license)}\n`);
    for (const file of record.files) sections.push(`File: ${file.file}\nSHA256: ${file.sha256}\n${file.text}\n`);
  }
  return `\nBundled provider package licenses\n${sections.join('')}`;
}
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  if (process.argv.length !== 3) throw new Error('Pass the verified provider source directory.');
  process.stdout.write(collectProviderLicenses(process.argv[2]));
}
