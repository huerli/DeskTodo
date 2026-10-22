import { readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const versions = {
  'package.json': JSON.parse(read('package.json')).version,
  'src-tauri/tauri.conf.json': JSON.parse(read('src-tauri/tauri.conf.json')).version,
  'src-tauri/Cargo.toml': read('src-tauri/Cargo.toml').match(/\[package\]([\s\S]*?)(?=\n\[|$)/)?.[1].match(/^version\s*=\s*"([^"]+)"/m)?.[1],
  'src-tauri/Cargo.lock': read('src-tauri/Cargo.lock').match(/name = "desk-todo"\r?\nversion = "([^"]+)"/)?.[1],
};
if (Object.values(versions).some((version) => !version || version !== versions['package.json'])) {
  console.error('版本号不一致，请统一后重新构建：');
  for (const [file, version] of Object.entries(versions)) console.error(`  ${file}: ${version ?? '未找到'}`);
  process.exit(1);
}
console.log(`[version] ${versions['package.json']}（所有版本号一致）`);
