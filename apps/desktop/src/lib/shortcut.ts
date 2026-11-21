// Turns a key press into the shortcut text the native side understands, e.g. "Ctrl+Alt+Space".

export interface KeyLike {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

const MODIFIER_CODES = new Set(['ControlLeft', 'ControlRight', 'AltLeft', 'AltRight', 'ShiftLeft', 'ShiftRight', 'MetaLeft', 'MetaRight']);

const NAMED: Record<string, string> = {
  Space: 'Space',
  Enter: 'Enter',
  Escape: 'Escape',
  Tab: 'Tab',
  Backspace: 'Backspace',
  Delete: 'Delete',
  Insert: 'Insert',
  Home: 'Home',
  End: 'End',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  Backquote: 'Backquote',
  Minus: 'Minus',
  Equal: 'Equal',
  Comma: 'Comma',
  Period: 'Period',
  Slash: 'Slash',
  Semicolon: 'Semicolon',
  Quote: 'Quote',
  BracketLeft: 'BracketLeft',
  BracketRight: 'BracketRight',
  Backslash: 'Backslash'
};

function keyName(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  return NAMED[code] ?? null;
}

export type ShortcutResult = { ok: true; shortcut: string } | { ok: false; reason: 'modifier-only' | 'unsupported' | 'needs-modifier' };

/**
 * Shortcut text for a key press. A plain key without Ctrl/Alt/Win is refused (except F13-F24) because a global
 * shortcut on a normal key would take it away from every other program.
 */
export function shortcutFromKey(event: KeyLike): ShortcutResult {
  if (MODIFIER_CODES.has(event.code)) return { ok: false, reason: 'modifier-only' };
  const key = keyName(event.code);
  if (!key) return { ok: false, reason: 'unsupported' };
  const parts: string[] = [];
  if (event.ctrlKey) parts.push('Ctrl');
  if (event.altKey) parts.push('Alt');
  if (event.shiftKey) parts.push('Shift');
  if (event.metaKey) parts.push('Super');
  const highFunctionKey = /^F(1[3-9]|2[0-4])$/.test(key);
  if (!event.ctrlKey && !event.altKey && !event.metaKey && !highFunctionKey) return { ok: false, reason: 'needs-modifier' };
  parts.push(key);
  return { ok: true, shortcut: parts.join('+') };
}
