// A newer phone app on GitHub: the latest release of github.com/TuryTury0/AnyTask with an .apk in it. Android does
// not let an app replace itself silently, so the phone opens the download and the user taps the file to install it
// (it installs over this one: every release is signed with the same key).
import { getVersion } from '@tauri-apps/api/app';
import { inTauri } from './ipc';

const LATEST = 'https://api.github.com/repos/TuryTury0/AnyTask/releases/latest';

export interface PhoneUpdate {
  current: string;
  version: string;
  url: string;
}

/** True when version a is newer than b ("0.10.0" > "0.9.3"). */
export function newer(a: string, b: string): boolean {
  const parts = (v: string) => v.replace(/^v/, '').split(/[.-]/).map((p) => Number.parseInt(p, 10) || 0);
  const x = parts(a);
  const y = parts(b);
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    if ((x[i] ?? 0) !== (y[i] ?? 0)) return (x[i] ?? 0) > (y[i] ?? 0);
  }
  return false;
}

export async function currentVersion(): Promise<string> {
  return inTauri ? getVersion() : '0.0.0';
}

/** The newer phone app, or null (none, or GitHub could not be reached). */
export async function findUpdate(): Promise<PhoneUpdate | null> {
  const current = await currentVersion();
  try {
    const response = await fetch(LATEST, { headers: { Accept: 'application/vnd.github+json' } });
    if (!response.ok) return null;
    const release = (await response.json()) as { tag_name?: string; assets?: Array<{ name?: string; browser_download_url?: string }> };
    const version = String(release.tag_name ?? '').replace(/^v/, '');
    const apk = release.assets?.find((a) => a.name?.endsWith('.apk') && a.browser_download_url);
    if (!apk?.browser_download_url || !newer(version, current)) return null;
    return { current, version, url: apk.browser_download_url };
  } catch {
    return null;
  }
}
