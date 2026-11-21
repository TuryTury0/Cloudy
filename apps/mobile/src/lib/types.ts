// What the phone gets from the PC (see apps/desktop/src-tauri/src/phone.rs) and from its own Rust side.
import type { AssistantState } from '@anytask/ui';

export interface LinkStatus {
  paired: boolean;
  connected: boolean;
  pc: string;
  error?: string;
  /** The PC can be woken from here (its wake addresses are known). */
  canWake: boolean;
}

export interface Turn {
  id: number;
  at: number;
  user: string;
  assistant: string;
  actions: string[];
}

export type CoreEvent =
  | { kind: 'state'; state: AssistantState; title?: string; detail?: string }
  | { kind: 'turnStarted'; turnId: number; user: string }
  | { kind: 'toolStarted'; turnId: number; callId: string; name: string; label: string }
  | { kind: 'toolProgress'; turnId: number; callId: string; text: string }
  | { kind: 'toolFinished'; turnId: number; callId: string; name: string; ok: boolean; summary: string }
  | { kind: 'answer'; turnId: number; text: string }
  | { kind: 'error'; turnId: number | null; message: string }
  | { kind: 'cancelled'; turnId: number }
  | { kind: 'usage' | 'turnUsage' | 'modelUsed' };

export type RemoteMessage =
  | { type: 'welcome'; assistant: string; busy: boolean; turns: Turn[]; addresses?: string[]; macs?: string[] }
  | { type: 'event'; event: CoreEvent }
  /** `about` names the request that failed ("input", "screen", "files", "fetch"); none: a chat request. */
  | { type: 'error'; message: string; about?: string }
  | { type: 'unpaired' }
  /** The PC is sending a file; the phone fetches it by itself (progress comes as "transfer" events). */
  | { type: 'offer'; path: string; name: string; size: number }
  /** The answer could not be spoken on the PC (or played here). */
  | { type: 'voice-error'; message: string; turnId?: number };

export interface Step {
  id: string;
  label: string;
  status: 'running' | 'done' | 'failed';
}

export interface Message {
  key: string;
  role: 'user' | 'assistant';
  text: string;
  status: 'pending' | 'done' | 'error' | 'stopped';
  steps: Step[];
  turnId?: number;
}

export type MouseButton = 'left' | 'right';

/** Remote mouse and keyboard input, sent as {"type": "input", ...} (fire and forget). */
export type InputOp =
  | { op: 'move'; dx: number; dy: number }
  | { op: 'button'; button: MouseButton; down: boolean }
  | { op: 'click'; button: MouseButton; count: number }
  /** x and y are 0..1000 of the PC screen. */
  | { op: 'tap'; x: number; y: number; button?: MouseButton; count?: number }
  /** 120 is one notch; positive is up (or right). */
  | { op: 'wheel'; dy: number; dx?: number }
  | { op: 'text'; text: string }
  | { op: 'key'; combo: string };

/** The PC's screen for the screen view. */
export interface Screen {
  /** JPEG, base64. */
  jpeg: string;
  w: number;
  h: number;
  /** The pointer's place, 0..1000 of the screen. */
  cursor: [number, number] | null;
}

export interface FileEntry {
  name: string;
  path: string;
  dir: boolean;
  size?: number;
  /** Milliseconds since 1970. */
  modified?: number;
}

/** A PC folder; path "" with no parent is the list of starting places (Desktop, Downloads, drives). */
export interface Listing {
  path: string;
  parent: string | null;
  entries: FileEntry[];
}

/** A file going to the PC (up) or coming from it (down), as the "transfer" events report it. */
export interface TransferEvent {
  id: string;
  direction: 'up' | 'down';
  name: string;
  size: number;
  done?: number;
  finished?: boolean;
  error?: string;
  /** Where a downloaded file is on the phone (for open_file). */
  uri?: string;
  /** Where an uploaded file was saved on the PC. */
  path?: string;
}

export interface Transfer extends TransferEvent {
  /** Bytes per second, smoothed. */
  rate: number;
  at: number;
}

/** A computer on this Wi-Fi waiting for a pairing code (find_code_pc). */
export interface CodePc {
  name: string;
  address: string;
  key: string;
  /** Short code of the PC's key ("A1B2-C3D4-E5F6"); the PC shows the same one next to the 6 digits. */
  fingerprint: string;
}

export type PowerAction ='lock' | 'sleep' | 'restart' | 'shutdown' | 'cancel';

export interface Access {
  battery: number;
  charging: boolean;
  notificationAccess: boolean;
  smsAccess: boolean;
}
