<div align="center">

<img src="apps/desktop/assets/icon.svg" width="96" height="96" alt="Cloudy logo" />

# Cloudy

**A fast, privacy-first voice & desktop AI assistant for Windows with an encrypted Android companion app.**

Built with **Tauri 2 (Rust)**, **Svelte 5**, and **TypeScript**.

[![Platform](https://img.shields.io/badge/Platform-Windows%2010%2F11%20%7C%20Android-38bdf8?style=flat-square)](#getting-started)
[![Stack](https://img.shields.io/badge/Stack-Tauri%202%20%C2%B7%20Rust%20%C2%B7%20Svelte%205-7dd3fc?style=flat-square)](#repository-layout)
[![MCP](https://img.shields.io/badge/MCP-AI%20Installer%20%26%20Live%20Tester-bae6fd?style=flat-square)](#ai-mcp-installer--live-tools)
[![License](https://img.shields.io/badge/License-MIT-ffffff?style=flat-square)](Cargo.toml)

[Features](#features) • [Examples](#what-you-can-ask) • [AI MCP Installer](#ai-mcp-installer--live-tools) • [Phone Companion](#encrypted-phone-companion) • [Safety & Privacy](#safety--privacy-by-design) • [Getting Started](#getting-started)

</div>

---

**Cloudy** gets real work done on your PC: it talks naturally, searches and reads the web, reads the browser tab you have open, plays music, manages files, sets executable reminders, connects to apps like Gmail, GitHub, and Notion via **MCP**, and controls desktop UIs with mouse and keyboard only when no background tool can handle the task.

It is built for **global, multilingual use**—supporting **English**, **RTL languages**, and extensible localization across both the interface and voice/text conversations. For detailed reliability notes and benchmarks from real usage logs, see [CAPABILITIES.md](CAPABILITIES.md).

---

## Features

### ☁️ Dynamic Island & Liquid Glass UI
- **Dynamic Island**: A spring-animated floating notch at the top edge of your monitor showing a live audio wave while listening or speaking, thinking shimmer, step-by-step execution progress, answer cards, and interactive permission prompts. It stays click-through outside active surfaces and automatically hides itself from screenshots during screen automation.
- **Liquid Glass Windows**: Translucent Windows Acrylic `Chat` and `Settings` windows with pointer-reactive highlights and an animated cloud character (`BotFace`) that blinks, looks around while thinking, reacts to speech, and smiles on success.
- **Customizable Themes & Multilingual UI**: Ships with the **Cloudy** (white & sky-blue) theme by default, plus `Monochrome Noir`, `Aurora`, `Sunset`, `Ocean`, `Forest`, `Royal`, and custom 3-color pickers with full LTR/RTL layout support (`Settings > General > Language`).

### 🎙️ Voice & Offline Wake Word
- **Gemini Live Voice**: Real-time bidirectional voice conversation with natural interruptions and push-to-talk hotkey (`Ctrl+Alt+Space`), plus optional spoken readback for typed responses.
- **On-Device Wake Word**: Train your own custom wake phrase directly in Settings using local **MFCC + DTW** audio matching—100% offline, nothing leaves your PC.

### 🧩 AI MCP Installer & Live Tools (`Settings > Tools & MCP`)
Connect external apps and services in seconds without editing JSON or hunting for terminal commands:
- **Conversational Setup Assistant**: Tell the assistant inside Settings what you want to connect (e.g., *"Connect my Gmail"* or *"Install Notion MCP"*). It checks your PC runtimes (`Node.js`/`npx`, `uv`/`uvx`, `Python`, `Docker`), uses verified built-in presets or live web search + documentation reading for unknown servers, and builds a step-by-step setup card.
- **Built-in Verified Presets**:
  - **Gmail (Quick App Password)** — Connect in ~60 seconds with a Google App Password (no Google Cloud Console setup required)
  - **Google Workspace (OAuth)** — Gmail, Google Calendar, and Google Drive via `workspace-mcp`
  - **GitHub** (Local stdio or zero-install Cloud HTTP), **Notion**, **Playwright**, **Filesystem**, **Memory**, **Fetch**, **PostgreSQL** (read-safe restricted mode), **Brave Search**, and **Sequential Thinking**
- **Zero-Leak Secrets & Uploads**: Tokens, passwords, and JSON credential files are submitted only through secure card inputs straight into **Windows Credential Manager** (or isolated app data)—never sent to the LLM or saved in plaintext files.
- **Live Server & Tool Tester**: Ping/health-check servers with latency readout, run any individual MCP tool via an auto-generated Schema form (or raw JSON), toggle individual tools on/off (keeping within the 40-tool model budget), inspect redacted per-server logs, or click **Fix with AI** for automatic troubleshooting.
- **One-Click Runtime Setup**: Detects missing `Node.js` or `uv` on Windows and offers guided `winget` installation.
- **Claude Desktop Compatibility**: Uses standard `mcp-servers.json` format so existing configurations can be pasted or edited anytime.

### ⚡ Fast Background Tools (No Mouse/Keyboard)
- **Web Search & Grounding**: Quick or deep web search powered by Google Search grounding (via your Gemini key), Tavily/Brave keys, or multi-engine free fallback (Yahoo, DuckDuckGo, Wikipedia).
- **Page & Active Tab Reader**: Fetches pages in the background with Markdown table formatting, or reads the exact URL open in your front-most browser tab (`read_browser_tab`).
- **Live Markets, Shopping & Weather**: Real-time currency and commodity quotes, multi-product shop price comparison, listing lookups, and global weather forecasts.
- **Files & OS Control**: Search (`find_files`), read, write (with overwrite confirmation), list, and open files/folders (`"here"` resolves to your open File Explorer folder), clipboard read/write, Windows media session control (`Play`/`Pause`/`Next`/`Prev`/now playing), system volume, and app open/close.
- **Executable Reminders**: Schedule alarms or timed actions (*"In 2 minutes play my focus playlist"*).
- **Long-Term Memory**: SQLite-backed conversation history and persistent user facts (`remember_fact` / `recall_memory`).

### 🖥️ Dual-Agent Screen Automation & Train Mode
- **Planner + Executor Screen Agent**: For tasks inside apps without APIs, Cloudy combines clean screenshots, Windows UI Automation, and Windows OCR, clicking controls by name in short verified batches.
- **100+ Built-in App Skills (`crates/core/skills/`)**: Context-aware tips loaded on demand only for the active app/site (zero token overhead otherwise). Custom `.md` skill files can be added or overridden in `Settings > Learning`.
- **Optional Second Check**: Pair with a decision model (e.g., Liquid `d1-ultra`) to double-check completion on screenshots and gate irreversible clicks.
- **Train Mode**: Automatically saves playbooks from successful runs and turns user corrections into reusable lessons.

### 📱 Encrypted Phone Companion (`apps/mobile`)
- **End-to-End Encrypted**: Connects over **Noise IK** on local Wi-Fi or **Tailscale** from outside home (zero router port forwarding).
- **Verified Pairing**: Scan a one-time QR code or enter a 6-digit code after comparing the PC's key fingerprint. Auto-rediscovers your PC if its LAN IP changes.
- **Full Remote Control**: Chat & voice requests, PC touchpad (scroll, drag, right-click) & keyboard, live PC screen viewer (tap to click), two-way file transfers, power menu (Lock, Sleep, Restart, Shut down with 20s cancel countdown), Wake-on-LAN, and assistant access to phone notifications, SMS, alarms, timers, dialer, maps, and ring-to-find.

### 🧠 Multi-Model & Smart Proxy Routing
- Works with **Google Gemini** (including Gemini Live) and any **OpenAI-compatible** endpoint (OpenAI, OpenRouter, Ollama local models, or custom self-hosted endpoints) with automatic **Backup Model** failover.
- Per-model, voice, and web **SOCKS5 / HTTP Proxy** routing with automatic direct/proxy fallback.

---

## What You Can Ask

| Request | What Cloudy Does |
| --- | --- |
| *"What's the current USD to EUR exchange rate?"* | Reads live market rates in the background and answers with the latest price and date. |
| *"Read this tab — how do I download and install it?"* | Reads the URL of your currently open browser tab, parses the page, and summarizes the steps. |
| *"Play Believer by Imagine Dragons on Spotify"* | Opens Spotify, searches, plays the song, and verifies playback via Windows Media Session. |
| *"Pause it"* / *"Next track"* | Pauses or skips tracks directly via Windows media controls and reports the current song. |
| *"Remind me to drink water at 8"* | Clarifies AM/PM if needed and sets a spoken reminder. |
| *"In 2 minutes, play my focus playlist"* | Creates an executable reminder that runs the action automatically when due. |
| *"Write the current exchange rates to a file on my Desktop"* | Fetches the data and writes the file (asks permission before modifying existing files). |
| *"What's the weather forecast for Tokyo tomorrow?"* | Returns the weather forecast for any city worldwide. |
| *"Translate the text I just copied"* | Reads your clipboard and translates it into your target language. |
| *"Connect my Gmail"* *(in Settings > Tools & MCP)* | Checks `npx`, prepares a secure Gmail setup form, saves your App Password in Credential Manager, connects, and lists tools. |

---

## Built-in App Skills (`crates/core/skills/`)

Loaded dynamically into the screen agent only when relevant to the active task:

| Category | Covered Apps & Sites |
| --- | --- |
| **Search & Knowledge** | Google Search, Bing, DuckDuckGo, Wikipedia, Google Translate, Google Maps, Stack Overflow, Quora, Medium, MDN Web Docs |
| **Google & Microsoft Online** | Gmail, Google Drive, Google Docs, Google Sheets, Google Calendar, Outlook, OneDrive, Dropbox |
| **AI Chat** | ChatGPT, Claude, Gemini |
| **Shopping & Travel** | Amazon, eBay, AliExpress, Walmart, Etsy, Booking.com, Tripadvisor |
| **Social** | X (Twitter), Instagram, Facebook, LinkedIn, Pinterest, TikTok, Reddit |
| **Video & Music** | YouTube, YouTube Music, Spotify, Netflix, Twitch, SoundCloud, Apple Music, Deezer, IMDb, VLC, Media Player |
| **Chat & Meetings** | Telegram, WhatsApp (desktop & web), Microsoft Teams, Zoom, Discord, Slack |
| **Office & Notes** | Word, Excel, PowerPoint, OneNote, Notion, Trello, Sticky Notes, Notepad, Notepad++, PDF readers |
| **Creative** | Canva, Figma, Paint, Photoshop, GIMP, Blender, Audacity, OBS Studio, Snipping Tool, Photos, Camera, Sound Recorder |
| **Developer** | GitHub, Visual Studio Code, npm, PyPI |
| **Learning & Gaming** | Coursera, Udemy, Steam, Epic Games Launcher, Xbox app |
| **Browsers & Windows** | Chrome, Edge, Firefox, File Explorer, Windows Settings, Control Panel, Microsoft Store, Calculator, Clock, Weather, 7-Zip, WinRAR |
| **Regional Platforms** | Additional regional e-commerce, streaming, and messaging web apps |
| **Blocked for Safety** | Task Manager, PowerShell/CMD/Terminal, Registry Editor, AnyDesk & TeamViewer |

---

## Safety & Privacy by Design

- **OS Keychain Only**: Model API keys, search keys, remote pairing keys, and MCP secrets (`secret:NAME`) are stored strictly in **Windows Credential Manager**. Secrets are never written to `settings.json` or `mcp-servers.json`, never sent to LLMs during MCP setup, and automatically redacted (`***`) from logs and error messages.
- **Local-First Storage**: Conversation history (SQLite), learned lessons/playbooks, wake-word profiles, and daily logs stay on your machine (`%APPDATA%\com.anytask.next` and `%LOCALAPPDATA%\com.anytask.next\logs`).
- **Hard-Coded Screen Guardrails**: The screen agent is blocked in code from closing windows/tabs (`Alt+F4`, `Ctrl+W`, close buttons), choosing "Don't save", launching terminals/Task Manager/Registry/Run, or clicking Shut down / Restart / Sign out. Loop detection automatically aborts stuck steps, and destructive file operations require explicit confirmation on the Island card.

---

## Repository Layout

```text
.
├── apps/
│   ├── desktop/            # Tauri 2 desktop shell (Island, tray, hotkeys, settings, chat) + Svelte 5 UI
│   └── mobile/             # Android companion app (Tauri 2 + Svelte 5 + Kotlin native plugin)
├── packages/
│   └── ui/                 # Shared Liquid Glass design system, WebGL Aurora, Island, BotFace, i18n
├── crates/
│   ├── audio/              # Microphone & speaker audio capture/playback
│   ├── control/            # Windows screenshots, UI Automation, OCR, mouse/keyboard, media session
│   ├── core/               # Agent loop, voice conversation logic, tools, screen planner/executor, skills
│   ├── llm/                # Gemini (incl. Live), OpenAI-compatible client, thinking fallback, backup
│   ├── mcp/                # MCP client (stdio + Streamable HTTP), per-server logs, runtime checks, tester
│   ├── migrate/            # Importer for v1 data
│   ├── remote/             # Noise IK encrypted pairing, discovery, Wake-on-LAN & PC-phone protocol
│   ├── storage/            # SQLite memory & OS keychain secret store
│   ├── wake/               # On-device MFCC + DTW wake word engine & VAD
│   └── web/                # Grounded Google/Tavily/Brave/free search, page reader, weather, shop prices
└── scripts/                # Build, verification, and test helper scripts
```

---

## Getting Started

### Prerequisites

- **Windows 10 / 11**
- **[Rust](https://www.rust-lang.org/tools/install)** (stable toolchain, Edition 2024)
- **[Node.js](https://nodejs.org/)** (LTS v20+ recommended)
- **WebView2** (pre-installed on Windows 11)
- *(Optional for Android)* Android Studio / SDK / NDK + JDK

### Clone, Build & Run

```bash
git clone https://github.com/TuryTury0/Cloudy.git
cd Cloudy

# Install JS workspace dependencies (once)
npm install

# Run desktop app in development mode with hot reload
npm run dev

# Build a fast standalone runnable binary (target/app/anytask-desktop.exe)
npm run app
npm run app:open

# Build full NSIS Windows installer
npm run build
```

> **Low-RAM Tip:** Limit parallel Rust compiler jobs with `CARGO_BUILD_JOBS=2`.

### Checks & Tests

```bash
# Type-check Svelte/TypeScript workspaces + strict Rust Clippy (-D warnings)
npm run check

# Run Vitest UI tests + entire Rust workspace test suite
npm test
```

---

## Default Global Shortcuts

| Action | Shortcut |
| --- | --- |
| Talk to Cloudy (voice) | `Ctrl+Alt+Space` |
| Type a request on Island | `Ctrl+Alt+T` |
| Show / hide Island | `Ctrl+Alt+H` |
| Stop everything | `Ctrl+Alt+X` |
| Open Settings | `Ctrl+Alt+S` |
| Open Chat window | `Ctrl+Alt+C` |

All shortcuts can be customized in **Settings > Shortcuts**.

---

## License

[MIT](Cargo.toml)