import { describe, expect, it } from 'vitest';
import { L, dir, i18n, normalizeLanguage, setLanguage } from './i18n.svelte';

describe('language', () => {
  it('is English unless Persian is chosen', () => {
    expect(normalizeLanguage('fa')).toBe('fa');
    expect(normalizeLanguage('en')).toBe('en');
    expect(normalizeLanguage(undefined)).toBe('en');
    expect(normalizeLanguage('de')).toBe('en');
  });

  it('picks the text and the direction of the current language', () => {
    setLanguage('en', null);
    expect(L('Stop', 'توقف')).toBe('Stop');
    expect(dir()).toBe('ltr');
    setLanguage('fa', null);
    expect(i18n.lang).toBe('fa');
    expect(L('Stop', 'توقف')).toBe('توقف');
    expect(dir()).toBe('rtl');
    setLanguage('en', null);
  });
});
