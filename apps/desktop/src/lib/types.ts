// Mirrors the Rust types in src-tauri/src (settings.rs, commands.rs, hotkeys.rs, mcp.rs, mcp_catalog.rs, mcp_setup.rs)
// and crates (core events, llm models, mcp).
import type { AssistantState } from '@anytask/ui';

export interface Hotkeys {
  talk: string;
  typeText: string;
  toggleIsland: string;
  stop: string;
  openSettings: string;
  openChat: string;
}

export interface ThemeSettings {
  name: string;
  colors: string[];
  weights: number[];
}

export interface Settings {
  /** The app's language: 'en' (default) or 'fa'. */
  language: 'en' | 'fa';
  assistantName: string;
  userName: string;
  hotkeys: Hotkeys;
  theme: ThemeSettings;
  showIsland: boolean;
  /** Start Cloudy when signing in to Windows. */
  startWithWindows: boolean;
  /** Download a new version from GitHub and install it when no job is running. */
  autoUpdate: boolean;
  reduceMotion: boolean;
  models: ModelConfig[];
  activeModel: string;
  backupModel: string;
  voice: VoiceSettings;
  proxy: ProxySettings;
  /** The second check (Settings > Models): a decision model looks again before "done" and risky clicks. */
  decision: DecisionSettings;
}

/** An app tip file for the screen agent (Settings > Learning). */
export interface Skill {
  name: string;
  keys: string[];
  text: string;
  builtin: boolean;
}

/** A decision model (Liquid's decisions API, e.g. d1-ultra). */
export interface DecisionModel {
  id: string;
  name: string;
  baseUrl: string;
  modelId: string;
}

export interface DecisionSettings {
  enabled: boolean;
  models: DecisionModel[];
  active: string;
}

/** One question of the second check's test and the model's answer. */
export interface DecisionTestCase {
  group: 'words' | 'clicks' | 'screen';
  lang: 'en' | 'fa' | 'fin' | 'image';
  text: string;
  want: boolean;
  probability: number | null;
  ok: boolean;
  ms: number;
  error: string | null;
}

export interface DecisionTestReport {
  cases: DecisionTestCase[];
  passed: number;
  total: number;
  imagesWork: boolean;
  medianMs: number;
  verdict: 'good' | 'weak' | 'failed';
  error: string | null;
}

/** One MCP server as written in mcp-servers.json (Claude Desktop format). */
export interface McpServerConfig {
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  url?: string;
  headers?: Record<string, string>;
  tools?: string[];
  disabled?: boolean;
}

export type McpStatus = 'connecting' | 'connected' | 'error' | 'disabled' | 'stopped';

export interface McpToolInfo {
  name: string;
  description: string;
  schema: Record<string, unknown>;
  enabled: boolean;
}

export interface McpEnvStatus {
  node: boolean;
  npx: boolean;
  uv: boolean;
  uvx: boolean;
  python: boolean;
  git: boolean;
  docker: boolean;
}

export interface McpServerStatus {
  name: string;
  status: McpStatus;
  error?: string;
  tools: string[];
  allTools: McpToolInfo[];
  runs: string;
  logs: string[];
  latencyMs?: number;
}

export interface McpReport {
  servers: McpServerStatus[];
  error?: string;
  path: string;
  secrets: Array<{ name: string; set: boolean }>;
  builtin: string[];
  config: { mcpServers: Record<string, McpServerConfig> };
  maxTools: number;
  activeToolCount: number;
  env: McpEnvStatus;
  exclusiveSecrets: Record<string, string[]>;
}

export interface McpServerTestReport {
  name: string;
  ok: boolean;
  status: McpStatus;
  ms: number;
  tools: string[];
  error?: string;
  logs: string[];
}

export interface McpToolTestReport {
  server: string;
  tool: string;
  ok: boolean;
  ms: number;
  text: string;
  error?: string;
}

export interface McpCatalogItem {
  id: string;
  serverName: string;
  titleEn: string;
  titleFa: string;
  summaryEn: string;
  summaryFa: string;
  requires: string[];
}

export interface McpSetupField {
  key: string;
  labelEn: string;
  labelFa: string;
  helpEn: string;
  helpFa: string;
  helpUrl?: string;
  kind: 'secret' | 'text' | 'file_json';
  secretName?: string;
  target: 'env' | 'header' | 'arg_replace';
  placeholder?: string;
  defaultValue?: string;
  required: boolean;
  alreadySet: boolean;
}

export interface McpSetupPlan {
  presetId?: string;
  serverName: string;
  titleEn: string;
  titleFa: string;
  summaryEn: string;
  summaryFa: string;
  kind: 'command' | 'url';
  command?: string;
  args: string[];
  url?: string;
  staticEnv: Record<string, string>;
  staticHeaders: Record<string, string>;
  tools: string[];
  requires: string[];
  missingRequires: string[];
  fields: McpSetupField[];
}

export interface McpSetupMessage {
  id: number;
  role: 'user' | 'assistant';
  text: string;
  plan?: McpSetupPlan;
  steps: string[];
  at: number;
  error?: string;
}

export interface McpSetupState {
  messages: McpSetupMessage[];
  busy: boolean;
  progress: string;
}

export interface McpUploadedFile {
  fieldKey: string;
  filename: string;
  content: string;
}

export interface McpApplyOutcome {
  report: McpReport;
  setup: McpSetupState;
  server?: McpServerStatus;
}

/** Train mode: what the assistant learned from use. */
export interface LearnSummary {
  enabled: boolean;
  requests: number;
  worked: number;
  playbooks: Array<{ goal: string; steps: string[]; window: string; seconds: number; uses: number; at: number }>;
  lessons: Array<{ id: number; text: string; about: string; uses: number; at: number }>;
}

/** What the assistant knows about this PC. */
export interface PcView {
  scannedAt: number;
  profile: string;
  appCount: number;
  preferences: Array<{ key: string; value: string }>;
}

/** What AnyTask v1 left on this PC (never keys). */
export interface V1Report {
  summary: {
    found: boolean;
    assistantName?: string;
    userName?: string;
    voice?: string;
    theme?: string;
    models: string[];
    keys: number;
    search?: string;
    wakeWord?: string;
    turns: number;
    facts: number;
    mcpServers: string[];
    problems: string[];
  };
  importedAt?: number;
}

/** A paired phone. */
export interface PhoneDevice {
  name: string;
  key: string;
  pairedAt: number;
  lastSeen: number;
  connected: boolean;
}

export interface PhoneStatus {
  enabled: boolean;
  error?: string;
  address: string;
  devices: PhoneDevice[];
}

export interface PairingView {
  svg: string;
  text: string;
  minutes: number;
  /** 6-digit code to type on a phone on the same network. */
  code: string;
  /** This PC's key in short, shown on the phone before it sends the code. */
  fingerprint: string;
}

/** The wake word as Settings shows it. */
export interface WakeStatus {
  hasModel: boolean;
  enabled: boolean;
  listening: boolean;
  training: boolean;
  label: string;
  quality: string;
  tested: boolean;
  createdAt: number;
}

/** One step of wake word training. */
export type WakeTraining =
  | { kind: 'started' | 'cancelled' }
  | { kind: 'prompt'; stage: 'enroll'; index: number; total: number }
  | { kind: 'prompt'; stage: 'negatives'; seconds: number }
  | { kind: 'prompt'; stage: 'test' | 'test_retry' }
  | { kind: 'accepted'; index: number; total: number }
  | { kind: 'rejected'; reason: string }
  | { kind: 'tick'; remaining: number }
  | { kind: 'finished'; quality: string; tested: boolean }
  | { kind: 'failed'; reason: string };

/** Voice conversations (Gemini Live). */
export interface VoiceSettings {
  model: string;
  voice: string;
  quietSeconds: number;
  speakAnswers: boolean;
}

/** The user's proxy (a VPN app such as v2rayN). */
export interface ProxySettings {
  /** "127.0.0.1:10808" (SOCKS5) or a full address such as http://127.0.0.1:10809; empty = none. */
  address: string;
  /** Ids of the models whose requests go through the proxy. */
  models: string[];
  /** The voice (Gemini Live) connects through the proxy. */
  voice: boolean;
  /** Web pages and searches that fail straight out are tried once more through the proxy. */
  web: boolean;
}

export type Provider = 'gemini' | 'openai';
export type Thinking = 'default' | 'low' | 'medium' | 'high';

/** A model the user added; its API key lives in the OS keychain, never here. */
export interface ModelConfig {
  id: string;
  name: string;
  provider: Provider;
  baseUrl: string;
  modelId: string;
  thinking: Thinking;
}

export interface ModelTest {
  ms: number;
  reply: string;
}

export interface SearchKeys {
  tavily: boolean;
  brave: boolean;
}

export interface Turn {
  id: number;
  at: number;
  user: string;
  assistant: string;
  actions: string[];
}

export interface TurnOutcome {
  turnId: number;
  status: 'answered' | 'failed' | 'cancelled';
  text: string;
  actions: string[];
}

export interface ConfirmRequest {
  id: number;
  title: string;
  detail: string;
}

/** Everything the assistant does arrives as one of these ('core-event'). */
export type CoreEvent =
  | { kind: 'state'; state: AssistantState; title?: string; detail?: string }
  | { kind: 'turnStarted'; turnId: number; user: string }
  | { kind: 'toolStarted'; turnId: number; callId: string; name: string; label: string; args: unknown }
  | { kind: 'toolProgress'; turnId: number; callId: string; text: string }
  | { kind: 'toolFinished'; turnId: number; callId: string; name: string; ok: boolean; summary: string }
  | { kind: 'answer'; turnId: number; text: string }
  | { kind: 'modelUsed'; label: string; ok: boolean; reason?: string }
  | { kind: 'error'; turnId: number | null; message: string }
  | { kind: 'cancelled'; turnId: number };

export interface HotkeyProblem {
  action: string;
  shortcut: string;
  reason: string;
}

export interface SaveReport {
  saved: boolean;
  problems: HotkeyProblem[];
  error: string | null;
}

/** The updater's state (updater.rs); `state`: idle, checking, latest, downloading, ready, installing, failed. */
export interface UpdateStatus {
  state: string;
  current: string;
  version?: string | null;
  notes?: string | null;
  progress?: number | null;
  error?: string | null;
}

export interface AppInfo {
  version: string;
  settingsPath: string;
}

export const DEFAULT_SETTINGS: Settings = {
  language: 'en',
  assistantName: 'Cloudy',
  userName: '',
  hotkeys: {
    talk: 'Ctrl+Alt+Space',
    typeText: 'Ctrl+Alt+T',
    toggleIsland: 'Ctrl+Alt+H',
    stop: 'Ctrl+Alt+X',
    openSettings: 'Ctrl+Alt+S',
    openChat: 'Ctrl+Alt+C'
  },
  theme: { name: 'Cloudy', colors: ['#38bdf8', '#ffffff', '#bae6fd'], weights: [45, 35, 20] },
  showIsland: true,
  startWithWindows: false,
  autoUpdate: true,
  reduceMotion: false,
  models: [],
  activeModel: '',
  backupModel: '',
  voice: { model: 'gemini-3.8-live', voice: 'Aoede', quietSeconds: 20, speakAnswers: true },
  proxy: { address: '127.0.0.1:10808', models: [], voice: false, web: true },
  decision: { enabled: false, models: [], active: '' }
};

/** How a chat message looks: waiting for the answer, answered, failed or stopped. */
export type BubbleStatus = 'pending' | 'done' | 'error' | 'stopped';

export interface LogLine {
  at: string;
  kind: string;
  text: string;
}

export interface Tally {
  requests: number;
  calls: number;
  input: number;
  output: number;
  thinking: number;
}

export interface UsageSummary {
  last: Tally | null;
  today: Tally;
  total: Tally;
}