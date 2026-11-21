// The phone app's shared state: the link to the PC, the chat, file transfers and toasts. Events from the Rust side
// change it here; every tab reads it, so a tab keeps what it shows while another tab is open.
import { friendlyError, type AssistantState } from '@anytask/ui';
import { invoke } from '../lib/ipc';
import type { PhoneUpdate } from '../lib/update';
import type { Access, CoreEvent, InputOp, LinkStatus, Message, RemoteMessage, Transfer, TransferEvent, Turn } from '../lib/types';
import { faError } from './text';

export type Tab = 'chat' | 'control' | 'files' | 'pc' | 'settings';

export interface Toast {
  id: number;
  text: string;
  kind: 'info' | 'ok' | 'error';
}

export const ui = $state({
  link: { paired: false, connected: false, pc: '', canWake: false } as LinkStatus,
  loaded: false,
  tab: 'chat' as Tab,
  assistant: 'Cloudy',
  mode: 'idle' as AssistantState,
  detail: '',
  busy: false,
  messages: [] as Message[],
  sendError: '',
  /** The PC's addresses, from its welcome. */
  addresses: [] as string[],
  access: null as Access | null,
  /** Why the app crashed last time, if it did. */
  crash: null as string | null,
  /** A newer phone app on GitHub (checked once at start). */
  update: null as PhoneUpdate | null,
  version: '',
  transfers: [] as Transfer[],
  toasts: [] as Toast[],
  /** Bumped when the chat should scroll to its end. */
  scroll: 0,
  /** A text field has the keyboard (the tab bar steps aside). */
  typing: false,
  /** Read the assistant's answers aloud with the phone's voice (kept on this phone). */
  speakAnswers: readFlag('anytask.speakAnswers')
});

function readFlag(key: string): boolean {
  try {
    return localStorage.getItem(key) === '1';
  } catch {
    return false;
  }
}

/** Turns reading answers aloud on or off (remembered on this phone). */
export function setSpeakAnswers(on: boolean) {
  ui.speakAnswers = on;
  try {
    localStorage.setItem('anytask.speakAnswers', on ? '1' : '0');
  } catch {
    // not kept
  }
  if (!on) void invoke('speak', { text: '' }).catch(() => {});
}

/** A short buzz on key taps (does nothing where the phone does not allow it). */
export function haptic(ms = 10) {
  try {
    navigator.vibrate?.(ms);
  } catch {
    // no vibration here
  }
}

let toastId = 0;

export function toast(text: string, kind: Toast['kind'] = 'info', ms = 3200) {
  const id = ++toastId;
  ui.toasts = [...ui.toasts.slice(-2), { id, text, kind }];
  setTimeout(() => (ui.toasts = ui.toasts.filter((t) => t.id !== id)), ms);
}

/** Opens a tab. Leaving the chat adds one history step, so Android's back button returns to the chat. */
export function goTab(tab: Tab) {
  if (tab === ui.tab) return;
  haptic();
  if (tab === 'chat') {
    if (history.state?.tab) history.back();
    ui.tab = 'chat';
    return;
  }
  if (history.state?.tab) history.replaceState({ tab }, '');
  else history.pushState({ tab }, '');
  ui.tab = tab;
}

/** Android's back button (or a history step): the tab in that step, or the chat. */
export function onHistory(state: unknown) {
  const tab = (state as { tab?: Tab } | null)?.tab;
  ui.tab = tab ?? 'chat';
}

// Input goes out one message at a time, in order (keys after the text before them).
let inputQueue: Promise<unknown> = Promise.resolve();
let inputErrorShown = 0;

/** Sends mouse or keyboard input to the PC (fire and forget). */
export function sendInput(input: InputOp) {
  if (!ui.link.connected) return;
  inputQueue = inputQueue.then(() =>
    invoke('send', { message: { type: 'input', ...input } }).catch((e) => {
      if (Date.now() - inputErrorShown > 4000) {
        inputErrorShown = Date.now();
        toast(faError(e), 'error');
      }
    })
  );
}

export async function loadAccess() {
  ui.access = (await invoke<Access>('access').catch(() => undefined)) ?? null;
}

function fromTurns(turns: Turn[]): Message[] {
  return turns.flatMap((t) => [
    { key: `u${t.id}`, role: 'user' as const, text: t.user, status: 'done' as const, steps: [] },
    { key: `a${t.id}`, role: 'assistant' as const, text: t.assistant, status: 'done' as const, steps: [] }
  ]);
}

function pendingFor(turnId: number) {
  return ui.messages.find((m) => m.role === 'assistant' && m.turnId === turnId && m.status === 'pending');
}

function scrollDown() {
  ui.scroll++;
}

function onEvent(event: CoreEvent) {
  switch (event.kind) {
    case 'state':
      ui.mode = event.state;
      ui.detail = event.state === 'error' ? friendlyError(event.detail ?? '').text : (event.detail ?? '');
      if (event.state === 'idle' || event.state === 'answer' || event.state === 'error') ui.busy = false;
      break;
    case 'turnStarted':
      ui.busy = true;
      ui.messages.push(
        { key: `lu${event.turnId}`, role: 'user', text: event.user, status: 'done', steps: [] },
        { key: `la${event.turnId}`, role: 'assistant', text: '', status: 'pending', steps: [], turnId: event.turnId }
      );
      scrollDown();
      break;
    case 'toolStarted':
      pendingFor(event.turnId)?.steps.push({ id: event.callId, label: event.label, status: 'running' });
      scrollDown();
      break;
    case 'toolProgress': {
      const m = pendingFor(event.turnId);
      if (m && !event.text.endsWith('…')) m.steps.push({ id: `${event.callId}:${m.steps.length}`, label: event.text, status: 'running' });
      break;
    }
    case 'toolFinished': {
      const m = pendingFor(event.turnId);
      if (m) for (const s of m.steps) if (s.status === 'running') s.status = event.ok ? 'done' : 'failed';
      break;
    }
    case 'answer': {
      const m = pendingFor(event.turnId);
      if (m) {
        m.text = event.text;
        m.status = 'done';
        for (const s of m.steps) if (s.status === 'running') s.status = 'done';
        // Spoken answers come from the PC as sound ("voice"); nothing to do here.
      }
      scrollDown();
      break;
    }
    case 'error': {
      const m = event.turnId === null ? undefined : pendingFor(event.turnId);
      if (m) {
        m.text = friendlyError(event.message).text;
        m.status = 'error';
      }
      break;
    }
    case 'cancelled': {
      const m = pendingFor(event.turnId);
      if (m) m.status = 'stopped';
      break;
    }
  }
}

export function onRemote(message: RemoteMessage) {
  switch (message.type) {
    case 'welcome':
      ui.assistant = message.assistant || 'Cloudy';
      ui.busy = message.busy;
      ui.messages = fromTurns(message.turns);
      if (message.addresses?.length) ui.addresses = message.addresses;
      scrollDown();
      break;
    case 'event':
      onEvent(message.event);
      break;
    case 'error':
      // Screen, folder and file requests get their errors as answers; a late one is not news.
      if (message.about === 'input') toast(faError(message.message), 'error');
      else if (!message.about) ui.sendError = faError(message.message);
      break;
    case 'unpaired':
      ui.messages = [];
      break;
    case 'offer':
      toast(`کامپیوتر «${message.name}» را فرستاد؛ در حال دریافت…`);
      haptic(15);
      break;
    case 'voice-error': {
      // The PC could not speak it: the phone's own voice reads the last answer instead.
      const last = [...ui.messages].reverse().find((m) => m.role === 'assistant' && m.text);
      if (ui.speakAnswers && last && document.visibilityState === 'visible') {
        const plain = last.text.replace(/[*_`#>|[\]()]/g, ' ').replace(/https?:\/\/\S+/g, '').trim();
        void invoke('speak', { text: plain }).catch((e) => toast(faError(e), 'error'));
      }
      break;
    }
  }
}

export function onTransfer(event: TransferEvent) {
  const now = Date.now();
  const old = ui.transfers.find((t) => t.id === event.id);
  if (!old) {
    ui.transfers = [{ ...event, rate: 0, at: now }, ...ui.transfers].slice(0, 30);
  } else {
    const seconds = (now - old.at) / 1000;
    const moved = (event.done ?? old.done ?? 0) - (old.done ?? 0);
    if (seconds > 0.05 && moved >= 0) old.rate = old.rate ? old.rate * 0.6 + (moved / seconds) * 0.4 : moved / seconds;
    Object.assign(old, event, { at: now });
  }
  if (event.finished && (!old || !old.finished)) {
    haptic(20);
    toast(event.direction === 'down' ? `«${event.name}» در گوشی ذخیره شد.` : `«${event.name}» به کامپیوتر رسید.`, 'ok');
  } else if (event.error && (!old || !old.error)) {
    toast(`«${event.name}»: ${faError(event.error)}`, 'error');
  }
}

/** Transfers still moving. */
export function moving() {
  return ui.transfers.filter((t) => !t.finished && !t.error).length;
}