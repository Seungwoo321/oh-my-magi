const fs = require('node:fs');
const path = require('node:path');

const root = fs.realpathSync(path.resolve(process.argv[2]));
const isWithin = (parent, candidate) => {
  const relative = path.relative(parent, candidate);
  return relative === '' || (relative !== '..'
    && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative));
};

for (const requestedPath of process.argv.slice(3)) {
  const target = path.resolve(requestedPath);
  if (!isWithin(root, target)) {
    throw new Error('Provider cache or staging path escapes the repository root.');
  }
  let current = root;
  const components = path.relative(root, target).split(path.sep).filter(Boolean);
  for (let index = 0; index < components.length; index += 1) {
    current = path.join(current, components[index]);
    let metadata;
    try {
      metadata = fs.lstatSync(current);
    } catch (error) {
      if (error.code === 'ENOENT') break;
      throw error;
    }
    if (metadata.isSymbolicLink()) {
      throw new Error('Provider cache or staging path traverses a symlink.');
    }
    if (index < components.length - 1 && !metadata.isDirectory()) {
      throw new Error('Provider cache or staging path has a non-directory parent.');
    }
    if (!isWithin(root, fs.realpathSync(current))) {
      throw new Error('Resolved provider cache or staging path escapes the repository root.');
    }
  }
}
