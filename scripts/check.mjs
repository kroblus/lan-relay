import { readdir, readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { resolve, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
async function walk(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (['node_modules', '.git', '.lan-relay-data', 'output', '.gradle', '.idea', 'build', 'bin', 'obj'].includes(entry.name)) continue;
    const path = resolve(dir, entry.name);
    if (entry.isDirectory()) files.push(...await walk(path));
    else files.push(path);
  }
  return files;
}

let failed = false;
const files = await walk(root);
for (const path of files.filter(path => /\.(mjs|js)$/.test(path))) {
  const result = spawnSync(process.execPath, ['--check', path], { encoding: 'utf8' });
  if (result.status !== 0) {
    failed = true;
    process.stderr.write(result.stderr || `Syntax check failed: ${relative(root, path)}\n`);
  }
}
for (const required of ['src/cli.mjs', 'src/server.mjs', 'public/index.html', 'public/app.mjs', 'public/styles.css', 'public/sha256.mjs', 'public/hash-worker.mjs', 'public/qr.mjs']) {
  if (!files.includes(resolve(root, required))) { failed = true; console.error(`Missing: ${required}`); }
}
const tracked = spawnSync('git', ['ls-files', '-z'], { cwd: root, encoding: 'utf8' });
if (tracked.status === 0) {
  for (const name of tracked.stdout.split('\0').filter(Boolean)) {
    if (/^(?:\.lan-relay-data|node_modules|output)\//.test(name) || /\.(?:sqlite(?:-[\w]+)?|pem|key|jks|keystore|pfx|p12|apk|aab|exe)$/.test(name) || /(?:^|\/)local\.properties$/.test(name) || (name.startsWith('.env') && name !== '.env.example')) {
      failed = true; console.error(`Runtime/private material must not be tracked: ${name}`);
    }
  }
}
const pkg = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8'));
if (Object.keys(pkg.dependencies || {}).length !== 0) {
  failed = true; console.error('V1 must stay dependency-free at runtime.');
}
if (failed) process.exitCode = 1;
else console.log(`Check passed: ${files.filter(path => /\.(mjs|js)$/.test(path)).length} JavaScript modules, required entrypoints and tracked-file rules.`);
