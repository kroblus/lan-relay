import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { readdir, readFile, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';

const [platform, directory] = process.argv.slice(2);
if (!['windows-x64', 'android'].includes(platform) || !directory) {
  throw new Error('Usage: node scripts/artifact-metadata.mjs windows-x64|android OUTPUT_DIRECTORY');
}
const root = path.resolve(directory);
const pkg = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8'));
const names = (await readdir(root, { withFileTypes: true }))
  .filter(entry => entry.isFile() && /\.(exe|zip|apk)$/.test(entry.name)).map(entry => entry.name).sort();
if (!names.length) throw new Error('No native build artifacts found.');
const files = [];
for (const name of names) {
  const file = path.join(root, name);
  const sha = createHash('sha256');
  for await (const chunk of createReadStream(file)) sha.update(chunk);
  files.push({ name, bytes: (await stat(file)).size, sha256: sha.digest('hex'), signing: name.endsWith('.apk')
    ? name.includes('unsigned') ? 'unsigned: not directly installable' : 'CI development key: trial only, not a stable upgrade identity'
    : 'No publisher code-signing certificate supplied' });
}
const info = {
  application: 'LAN Relay / 随手传', version: pkg.version, platform,
  sourceCommit: /^[a-f0-9]{40}$/.test(process.env.GITHUB_SHA || '') ? process.env.GITHUB_SHA : 'local-build',
  createdAt: new Date().toISOString(), buildHostNode: process.version,
  acceptance: 'Built artifacts are not evidence of physical Android/Windows acceptance. See docs/NATIVE_APPS.md.',
  localUserBaseline: 'User reported 53 local tests; that unpublished working copy was not imported.', files,
};
await writeFile(path.join(root, 'BUILD-INFO.json'), JSON.stringify(info, null, 2) + '\n');
await writeFile(path.join(root, 'SHA256SUMS.txt'), files.map(file => `${file.sha256}  ${file.name}`).join('\n') + '\n');
console.log(`Recorded ${files.length} ${platform} artifacts with SHA-256 and source provenance.`);
