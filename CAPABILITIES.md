# What Cloudy can do today

Reliability metrics below are measured from the app's own execution logs (433 requests, 2026-10-27 to 2026-11-11) and live/unit test runs on 2026-11-12. `"ok"` counts tool calls that completed without an error, prior to human evaluation.

## Works well (direct tools, no mouse or keyboard)

| Area | What you can ask | Evidence |
| --- | --- | --- |
| Reminders | Set, list, and cancel; reminders can also trigger an action at the scheduled time (*"In 2 minutes play song X"*) | 41 ok, 1 failed |
| Music | Play, pause, next, previous; identifies the current track | 48 ok, 6 failed (all 6: no player was open) |
| Web pages | Read a page or the active browser tab, summarize content, extract exact values | 54 + 4 ok, 2 failed |
| Weather | Forecast for any city worldwide across supported languages; remembers your default city | 16 ok, 1 failed |
| Apps and sites | Open an application or web page, close an application | 32 + 2 ok, 3 failed |
| Volume | Volume up, down, mute, or set to a specific level | 7 ok |
| Files | Write, read, list, and find files across local folders | 41 ok, 4 failed (model omitted filename) |
| Clipboard and memory | Read/write clipboard; remember and recall user facts | 10 ok |
| Search (new) | Quick answers or deep research across languages via Google Search grounding (through your Gemini key) with sources and dates | Live tests 11-12: quick 2 s, deep 5 s; live market rates verified with sources |
| Track playback in Spotify (new) | *"Open Spotify and play X"*: searches Spotify and clicks the track's own Play button by name; if no song is specified, resumes or opens Spotify and plays | Unit tests passing (`cargo run -p anytask-control --example spotify_play -- "Believer"`) |
| Now playing (new) | *"What is playing?"* / *"Name this song"*: reads the active Windows media session without clicking | Unit tests passing |
| Shop prices (new) | Fetches prices for one or multiple products in parallel (prioritizing new items, filtering sponsored ads) and optionally saves results to a file | Live 11-12: 1–2.7 s per product in batched queries |
| Screenshot to phone (new) | *"Take a screenshot and send it to my phone"*: saves to `Pictures\Screenshots` and transfers to the paired phone | Implemented, pending live end-to-end app run |
| `"Save it here"` (new) | `write_text_file here/name.txt`: resolves automatically to the currently open File Explorer folder, or falls back to Desktop | Explorer path resolved live in 0.25 s |

**Auto-Updates (new):** The desktop app checks `github.com/TuryTury0/Cloudy` one minute after startup and every 6 hours thereafter, downloads newer signed installers, applies the update when idle, and relaunches (`Settings > General > Auto-update`; `Settings > About > Check for updates`). Applies only to installed builds (never local dev/target builds). The mobile app checks the latest release once on startup and prompts to install newer APKs.

**Zero-token local fast-path commands (new):** Short commands—pause, next, previous, what is playing, volume up/down/mute/level, screenshot to phone, play music, or open an app—match directly in any supported language and execute immediately without calling the LLM. Longer requests or failed tools fall back to the model seamlessly.

Search without a Gemini key falls back to free engines (which can have lower coverage for non-English queries: 19 of 53 pre-change fallback searches returned empty). If a Tavily or Brave API key is configured, it takes priority.

## The phone app (`apps/mobile`)

| Feature | How it works |
| --- | --- |
| Pair | Scan the QR code or enter the 6-digit code displayed on the PC after verifying the PC's key fingerprint |
| Auto-reconnect | Automatically rediscovers your PC when its LAN IP changes (hotspot restart, Wi-Fi change) by probing all known addresses |
| Voice & text assistant | Send requests by text or voice (mic button supports multilingual modes), stream step progress, and cancel running jobs |
| Touchpad and keyboard | Move cursor, tap to click, two-finger tap for right-click, two-finger scroll, hold to drag, full phone keyboard input, special & media keys |
| Live screen viewer | Real-time view of the PC screen; tap anywhere on the image to click at that coordinate |
| Two-way file transfer | Browse PC folders and drives, download files to the phone, or send phone files to the PC (`Settings > Phone > Send a file` or via the assistant) |
| Power controls | Lock, sleep, restart, or shut down (restart and shut down include a 20 s cancelable countdown) |
| Wake-on-LAN | Wake the PC from the phone on the same network when supported by BIOS and NIC settings |
| Phone tools for the assistant | Read notifications and SMS (with explicit permission), list/open apps, open links, ring phone to locate it (`Settings > Phone > Ring`), set alarms/timers, pre-fill dialer/SMS, and open maps |
| Remote access outside home | Install Tailscale on both the phone and PC; Cloudy automatically routes over Tailscale with zero router port forwarding |

Benchmarked on 2026-11-12 against the desktop app with a test client: discovered PC in 2 s, paired by code in 0.4 s, streamed 51 KB screen frames in 0.3 s, and transferred a 761 KB file each way in 0.6 s with verified checksums.

Security: All traffic uses **Noise IK** end-to-end encryption and restricts access to paired devices; new pairing requires a QR token or one-time 6-digit code (5 failed attempts lock the code). Devices can be revoked anytime in `Settings > Phone`.

## Screen automation agent (UI fallback)

Used only when direct background tools cannot handle a task: 63 jobs marked `"ok"` by the model, 22 failed, and 13 stopped by the user. Recent improvements address misclicks and post-completion continuation: full-screen Windows OCR on every step, preserving list rows in dense UI trees, larger scroll increments, naming target controls per step, loop prevention, and playback state feedback. Controls are clicked by exact UI element name (`click : "Play"`) on clean screenshots without bounding-box overlays.

A dual-agent architecture separates planning from execution: the **Planner** (text-only) generates steps from the goal, app skills, and successful playbooks; the **Executor** follows strict batching rules and re-consults the Planner if blocked. Redundant row cells, duplicate labels, and window chrome buttons are filtered out of the Executor's view.

## Pending real-device verification

- Live third-party MCP servers (full client & setup UI implemented; compatible with Claude Desktop `mcp-servers.json`).
- Android companion on physical hardware across Android versions (touch gestures, camera QR scan, storage permissions).
- Wake-on-LAN across different motherboard BIOS and NIC configurations.