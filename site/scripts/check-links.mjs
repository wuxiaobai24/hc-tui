#!/usr/bin/env node
// 检查 dist 下所有 HTML 的相对 href/src 是否能在磁盘上解析。
import { readdir, readFile, stat } from 'node:fs/promises';
import { dirname, join, resolve, sep } from 'node:path';

const dist = resolve('dist');

async function walk(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) files.push(...(await walk(full)));
    else if (entry.isFile() && entry.name.endsWith('.html')) files.push(full);
  }
  return files;
}

async function exists(path) {
  try {
    const info = await stat(path);
    if (info.isFile()) return true;
    return false;
  } catch {
    return false;
  }
}

const files = await walk(dist);
let checked = 0;
const broken = [];

for (const file of files) {
  const html = await readFile(file, 'utf8');
  for (const match of html.matchAll(/(?:href|src)="([^"]*)"/g)) {
    const raw = match[1];
    if (!raw) continue;
    if (/^(?:[a-z]+:|\/\/|#)/i.test(raw)) continue;
    const target = raw.split('#')[0].split('?')[0];
    if (!target) continue;
    checked += 1;
    const base = dirname(file);
    let candidate = resolve(base, target);
    if (!candidate.startsWith(dist + sep) && candidate !== dist) {
      broken.push(`${file}: ${raw} (escapes dist)`);
      continue;
    }
    if (target.endsWith('/')) candidate = join(candidate, 'index.html');
    if (await exists(candidate)) continue;
    if (await exists(join(candidate, 'index.html'))) continue;
    broken.push(`${file}: ${raw}`);
  }
}

for (const item of broken) console.log(`broken ${item}`);
console.log(`${broken.length} broken (${checked} relative links checked in ${files.length} files)`);
process.exit(broken.length === 0 ? 0 : 1);
