import { describe, expect, it } from 'vitest';
import { normalizeHex, normalizeTheme, readableAccent, luminance, parseHex, themeVariables, DEFAULT_THEME } from './theme';

describe('theme', () => {
  it('normalizes hex colours', () => {
    expect(normalizeHex('#ABC')).toBe('#aabbcc');
    expect(normalizeHex('a855f7')).toBe('#a855f7');
    expect(normalizeHex('purple')).toBeNull();
    expect(normalizeHex(42)).toBeNull();
  });

  it('falls back to the default theme when no colour is valid', () => {
    expect(normalizeTheme({ colors: ['nope'] }).colors).toEqual(DEFAULT_THEME.colors);
    expect(normalizeTheme(null).name).toBe(DEFAULT_THEME.name);
  });

  it('normalizes weights to percentages and keeps at most three colours', () => {
    const theme = normalizeTheme({ name: 'X', colors: ['#f00', '#0f0', '#00f', '#fff'], weights: [2, 1, 1] });
    expect(theme.colors).toHaveLength(3);
    expect(theme.weights).toEqual([50, 25, 25]);
  });

  it('uses equal weights when the given ones are unusable', () => {
    expect(normalizeTheme({ colors: ['#f00', '#0f0'], weights: [1, -3] }).weights).toEqual([50, 50]);
  });

  it('lifts dark accents so they stay readable', () => {
    const lifted = readableAccent('#1e1b4b');
    expect(luminance(parseHex(lifted))).toBeGreaterThan(0.3);
    expect(readableAccent('#ffffff')).toBe('#ffffff');
  });

  it('produces css variables', () => {
    const vars = themeVariables({ name: 'A', colors: ['#22d3ee'], weights: [100] });
    expect(vars['--accent']).toBe('#22d3ee');
    expect(vars['--accent-3']).toBe('#22d3ee');
    expect(vars['--glow']).toMatch(/^rgba\(/);
  });
});
