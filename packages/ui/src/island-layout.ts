import { revealSpeed } from './reveal.svelte';
// Size and shape of the island for each assistant state. The island hangs from the top edge of the screen like a
// notch and morphs between these shapes with spring physics. Its body (steps, answer, text box) is measured by
// the component, so only the top band and the width are decided here.

export type AssistantState =
  | 'idle'
  | 'listening'
  | 'thinking'
  | 'speaking'
  | 'working'
  | 'success'
  | 'error'
  | 'text-input'
  | 'answer';

export const ASSISTANT_STATES: readonly AssistantState[] = [
  'idle',
  'listening',
  'thinking',
  'speaking',
  'working',
  'success',
  'error',
  'text-input',
  'answer'
];

export interface IslandShape {
  width: number;
  /** Height of the top band; the body under it is measured and added by the component. */
  height: number;
  /** Radius of the bottom corners and of the concave "shoulders" at the top corners. */
  radius: number;
}

/** One live step of a task, shown under the island while it works. */
export interface IslandStep {
  id: string;
  label: string;
  status: 'running' | 'done' | 'failed';
}

/** A question the island asks before a risky step. */
export interface ConfirmQuestion {
  title: string;
  detail: string;
}

export interface ShapeOptions {
  /** The island shows a body under its band (steps, answer, text box, question). */
  expanded?: boolean;
  /** The pointer is on the island. */
  hovered?: boolean;
  /** A confirmation question is open. */
  confirm?: boolean;
}

const IDLE: IslandShape = { width: 152, height: 30, radius: 13 };
const IDLE_HOVER: IslandShape = { width: 208, height: 36, radius: 15 };
const BAND = 40;

/** Band-only shapes (nothing under the band). */
const COMPACT: Record<AssistantState, IslandShape> = {
  idle: IDLE,
  listening: { width: 320, height: BAND, radius: 17 },
  thinking: { width: 250, height: BAND, radius: 17 },
  speaking: { width: 320, height: BAND, radius: 17 },
  working: { width: 290, height: BAND, radius: 17 },
  success: { width: 210, height: BAND, radius: 17 },
  error: { width: 300, height: BAND, radius: 17 },
  'text-input': { width: 480, height: BAND, radius: 17 },
  answer: { width: 300, height: BAND, radius: 17 }
};

/** Widths of the panel that hangs under the band. */
const EXPANDED_WIDTH: Partial<Record<AssistantState, number>> = { 'text-input': 480, answer: 470 };
const PANEL: IslandShape = { width: 440, height: BAND, radius: 28 };

const ANSWER_LINE = 21;
const MAX_ANSWER_LINES = 8;
const ANSWER_CHARS_PER_LINE = 54;

export function isAssistantState(value: unknown): value is AssistantState {
  return typeof value === 'string' && (ASSISTANT_STATES as readonly string[]).includes(value);
}

/** Lines the answer card shows before it scrolls (rough: paragraphs wrap every ~54 characters). */
export function answerLines(text: string): number {
  const lines = text
    .split('\n')
    .filter((p) => p.trim())
    .reduce((sum, p) => sum + Math.max(1, Math.ceil(p.trim().length / ANSWER_CHARS_PER_LINE)), 0);
  return Math.min(MAX_ANSWER_LINES, Math.max(1, lines));
}

/** Height the answer text may take before it scrolls. */
export function answerMaxHeight(text: string): number {
  return answerLines(text) * ANSWER_LINE;
}

/** Shape of the island's band for a state; a panel (expanded) is wider and has rounder corners. */
export function islandShape(state: AssistantState, options: ShapeOptions = {}): IslandShape {
  if (options.confirm) return PANEL;
  if (options.expanded) return { ...PANEL, width: EXPANDED_WIDTH[state] ?? PANEL.width };
  if (state === 'idle' && options.hovered) return IDLE_HOVER;
  return COMPACT[state] ?? IDLE;
}

/** States that end by themselves and fall back to idle after `ms`; longer texts stay longer. */
export function autoReturnMs(state: AssistantState, text = ''): number | null {
  if (state === 'success') return 1800;
  if (state === 'error') return Math.min(9000, 3200 + text.length * 35);
  // The answer types itself in, then stays just long enough to read (short ones about 1.5 s); the pointer on the
  // island keeps it, and the text can be selected and copied by hand.
  if (state === 'answer') {
    const typing = (text.length / revealSpeed(text.length)) * 1000;
    return Math.round(typing + Math.min(6000, Math.max(1500, text.length * 35)));
  }
  return null;
}
