import { describe, expect, it } from 'vitest';
import { shortcutFromKey, type KeyLike } from './shortcut';
import { regionsKey, toRegions } from './hit-regions';

const key = (code: string, mods: Partial<KeyLike> = {}): KeyLike => ({
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods
});

describe('shortcutFromKey', () => {
  it('builds Ctrl+Alt shortcuts', () => {
    expect(shortcutFromKey(key('Space', { ctrlKey: true, altKey: true }))).toEqual({ ok: true, shortcut: 'Ctrl+Alt+Space' });
    expect(shortcutFromKey(key('KeyT', { ctrlKey: true, shiftKey: true }))).toEqual({ ok: true, shortcut: 'Ctrl+Shift+T' });
    expect(shortcutFromKey(key('Digit5', { altKey: true }))).toEqual({ ok: true, shortcut: 'Alt+5' });
  });

  it('waits while only a modifier is held', () => {
    expect(shortcutFromKey(key('ControlLeft', { ctrlKey: true }))).toEqual({ ok: false, reason: 'modifier-only' });
  });

  it('refuses plain keys that other programs need', () => {
    expect(shortcutFromKey(key('F5'))).toEqual({ ok: false, reason: 'needs-modifier' });
    expect(shortcutFromKey(key('KeyA', { shiftKey: true }))).toEqual({ ok: false, reason: 'needs-modifier' });
    expect(shortcutFromKey(key('F13'))).toEqual({ ok: true, shortcut: 'F13' });
  });

  it('maps arrows and refuses unknown keys', () => {
    expect(shortcutFromKey(key('ArrowUp', { ctrlKey: true }))).toEqual({ ok: true, shortcut: 'Ctrl+Up' });
    expect(shortcutFromKey(key('IntlRo', { ctrlKey: true }))).toEqual({ ok: false, reason: 'unsupported' });
  });
});

describe('hit regions', () => {
  it('drops empty rectangles and keys by rounded values', () => {
    const regions = toRegions([
      { left: 10.2, top: 5.6, width: 168, height: 44 },
      { left: 0, top: 0, width: 0, height: 10 }
    ]);
    expect(regions).toHaveLength(1);
    expect(regionsKey(regions)).toBe('10,6,168,44');
    expect(regionsKey([{ x: 10.4, y: 5.5, width: 168, height: 44 }])).toBe(regionsKey(regions));
  });
});
