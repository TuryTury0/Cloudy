# AnyTask — releases

Each version has its own folder here (`v0.1.0/`, ...) with:

| File | What it is |
| --- | --- |
| `AnyTask_<version>_x64-setup.exe` | Windows installer (recommended) |
| `latest.json` | What the app's updater reads (the newest version, its installer and signature) |
| `AnyTask-phone-<version>.apk` | The Android phone app |
| `SHA256SUMS.txt` | Checksums to verify the downloads |

Persian guide: [README.fa.md](README.fa.md)

## Install on Windows

1. Run `AnyTask_<version>_x64-setup.exe`.
2. Windows may show "Windows protected your PC" (the installer is not code-signed yet): choose **More info → Run anyway**.
3. AnyTask starts with a small island at the top of the screen and an icon in the tray.

## First steps

1. **Settings → Models** (`Ctrl+Alt+S`): add a model and paste its API key (Gemini, OpenAI, OpenRouter, GapGPT, or a local Ollama). Keys are kept only in Windows Credential Manager.
2. Optional: pick a **backup model** for when the chosen one is busy.
   Optional: **Second check** (same page): add a decision model such as Liquid d1-ultra, press **Test**, and turn it on if the test says it is good. It looks again before a screen job says "done" and asks you before clicks that are hard to undo.
3. **Type** a request with `Ctrl+Alt+T`, or **talk** with `Ctrl+Alt+Space` (voice uses Gemini Live; it can have its own key in Settings → General → Voice).
4. **Stop** anything with `Ctrl+Alt+X`. Open the chat with `Ctrl+Alt+C`.
5. Coming from AnyTask v1? **Settings → About → Bring over from v1** copies your models and keys, memory, MCP servers and wake word.

## Wake word

Settings → General → Wake word: say your phrase six times, talk freely for a few seconds, then say it once more. Training and listening run only on this PC; no audio leaves it.

## Phone app (Android)

1. Copy `AnyTask-phone-<version>.apk` to the phone and open it; allow installing from this source.
2. On the PC: **Settings → Phone**, turn the phone connection on. Windows asks once for network access: allow **Private networks**.
3. Press **Add phone** and scan the QR code with the phone app. Phone and PC must be on the same Wi-Fi.
4. In the phone app, allow **notifications** and **SMS** if you want the PC's assistant to read them when you ask ("what's new on my phone?"). It can also open apps and links on the phone.

The connection is encrypted (Noise IK) and only paired phones can connect.

## Where things are

- Settings: `%APPDATA%\com.anytask.next\settings.json`
- Memory, usage, MCP servers, wake word, phone pairing: `%APPDATA%\com.anytask.next\`
- Logs (14 days): `%LOCALAPPDATA%\com.anytask.next\logs\` — also in **Settings → Logs**
- API keys: Windows Credential Manager, entries starting with `AnyTask Next`

## Uninstall

Windows Settings → Apps → AnyTask → Uninstall. Your data folder stays; delete `%APPDATA%\com.anytask.next` to remove it too.

## Building a release

`node scripts/release.mjs` (or `--desktop` / `--phone`) builds into `release/v<version>/`. Then upload every file
of that folder to a GitHub release tagged `v<version>` in github.com/TuryTury0/AnyTask (notes: `release/notes/`).
Installed apps read `releases/latest/download/latest.json` from there and update themselves.

Both keys are in `%USERPROFILE%\.anytask-signing\` (made once, never in git): the phone app's keystore and the
updater's `updater.key` with `updater.password`. **Back that folder up**: Android installs an update only when it is
signed with the same key, and installed desktop apps accept only updates signed with `updater.key`.
