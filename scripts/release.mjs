// Builds a release into release/v<version>/: the Windows installer (NSIS) with latest.json for the app's updater, the
// Android phone app (APK) and SHA256SUMS.txt. The version comes from apps/desktop/src-tauri/tauri.conf.json.
// Upload every file of that folder to a GitHub release tagged v<version> in github.com/TuryTury0/AnyTask: the desktop
// app reads releases/latest/download/latest.json, the phone app reads the latest release's .apk.
//
//   node scripts/release.mjs            everything
//   node scripts/release.mjs --desktop  only the Windows installer
//   node scripts/release.mjs --phone    only the APK
//
// The APK is a release build signed with the key in ~/.anytask-signing (see the phone app's build.gradle.kts).
// Keep that folder backed up: phones accept updates only when they are signed with the same key. The installer is
// signed for the updater with ~/.anytask-signing/updater.key (password in updater.password; made with
// `npx tauri signer generate`): installed apps accept only an update signed with it, so losing it ends updates.
// Release notes (shown in Settings > About) come from release/notes/v<version>.md when it exists. Builds are kept
// to two parallel compiler jobs: this PC has 8 GB of memory.
import { execSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const version = JSON.parse(readFileSync(join(root, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8')).version;
const out = join(root, 'release', `v${version}`);
const args = new Set(process.argv.slice(2));
const want = (part) => args.size === 0 || args.has(`--${part}`);
const env = { ...process.env, CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? '2' };
const repo = 'TuryTury0/AnyTask';
const signing = join(homedir(), '.anytask-signing');

function run(command, cwd) {
  console.log(`\n> ${command}`);
  execSync(command, { cwd, stdio: 'inherit', env });
}

function copyAll(dir, pattern, rename = (name) => name) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => pattern.test(name))
    .map((name) => {
      copyFileSync(join(dir, name), join(out, rename(name)));
      return rename(name);
    });
}

mkdirSync(out, { recursive: true });
const files = [];

if (want('desktop')) {
  const key = join(signing, 'updater.key');
  if (!existsSync(key)) throw new Error(`No updater signing key at ${key}: the app could not check its updates.`);
  env.TAURI_SIGNING_PRIVATE_KEY = readFileSync(key, 'utf8');
  env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = readFileSync(join(signing, 'updater.password'), 'utf8').trim();
  run('npx tauri build', join(root, 'apps/desktop'));
  const nsis = join(root, 'target/release/bundle/nsis');
  const setup = `AnyTask_${version}_x64-setup.exe`;
  copyFileSync(join(nsis, setup), join(out, setup));
  const signature = readFileSync(join(nsis, `${setup}.sig`), 'utf8').trim();
  const notesFile = join(root, 'release', 'notes', `v${version}.md`);
  const notes = existsSync(notesFile) ? readFileSync(notesFile, 'utf8').trim() : `AnyTask ${version}`;
  const latest = {
    version,
    notes,
    pub_date: new Date().toISOString(),
    platforms: { 'windows-x86_64': { signature, url: `https://github.com/${repo}/releases/download/v${version}/${setup}` } },
  };
  writeFileSync(join(out, 'latest.json'), JSON.stringify(latest, null, 2) + '\n');
  files.push(setup, 'latest.json');
}

if (want('phone')) {
  const sdk = process.env.ANDROID_HOME ?? join(process.env.LOCALAPPDATA ?? '', 'Android', 'Sdk');
  const ndkDir = join(sdk, 'ndk');
  const ndk = process.env.NDK_HOME ?? (existsSync(ndkDir) ? join(ndkDir, readdirSync(ndkDir).sort().pop()) : '');
  Object.assign(env, { ANDROID_HOME: sdk, NDK_HOME: ndk });
  run('npx tauri android build --apk --target aarch64', join(root, 'apps/mobile'));
  const apkDir = join(root, 'apps/mobile/src-tauri/gen/android/app/build/outputs/apk/universal/release');
  files.push(...copyAll(apkDir, /\.apk$/, () => `AnyTask-phone-${version}.apk`));
}

const sums = readdirSync(out)
  .filter((name) => name !== 'SHA256SUMS.txt')
  .map((name) => `${createHash('sha256').update(readFileSync(join(out, name))).digest('hex')}  ${name}`);
writeFileSync(join(out, 'SHA256SUMS.txt'), sums.join('\n') + '\n');
console.log(`\nRelease v${version} in ${out}:\n  ${[...new Set(files)].join('\n  ')}`);
