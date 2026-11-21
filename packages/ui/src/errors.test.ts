import { describe, expect, it } from 'vitest';
import { friendlyError } from './errors';
import { setLanguage } from './i18n.svelte';

describe('friendly errors', () => {
  it('points to the models page when the fix is there', () => {
    expect(friendlyError('No model: Add one in Settings > Models.').needsModel).toBe(true);
    expect(friendlyError('Gemini (g): No API key is saved for Gemini.').needsModel).toBe(true);
    const bad = friendlyError('Gemini (g): HTTP 401: API key not valid');
    expect(bad.text).toContain('401');
    expect(bad.needsModel).toBe(true);
    expect(bad.detail).toBe('Gemini (g): HTTP 401: API key not valid');
  });

  it('explains limits, server and network problems without blaming the settings', () => {
    expect(friendlyError('X: HTTP 429: quota').needsModel).toBe(false);
    const daily = friendlyError('Gemini: HTTP 429: requests per day limit exceeded. Please retry in 14h');
    expect(daily.text).toContain('daily');
    expect(daily.needsModel).toBe(true);
    expect(friendlyError('X: HTTP 503: down').text).toContain('503');
    expect(friendlyError('X: Network error: dns').text).toContain('internet');
  });

  it('speaks the language of the app', () => {
    setLanguage('fa', null);
    expect(friendlyError('Gemini: HTTP 429: requests per day limit exceeded').text).toContain('روزانه');
    expect(friendlyError('X: Network error: dns').text).toContain('اتصال');
    setLanguage('en', null);
  });

  it('no quota on the key points to another model or billing', () => {
    const none = friendlyError('Gemini (gemini-3.8-flash): HTTP 429: You exceeded your current quota, please check your plan and billing details.');
    expect(none.text).toContain('Flash-Lite');
    expect(none.needsModel).toBe(true);
    expect(friendlyError('X: HTTP 429: You exceeded your current quota. Please retry in 20s.').text).toContain('try again');
  });

  it('a refusal of the location asks for the VPN, not a new key', () => {
    const refused = friendlyError('Gemini (g): HTTP 403: the service refused this address (its 403 page, not an API answer): the location is not supported without a VPN');
    expect(refused.text).toContain('VPN');
    expect(refused.needsModel).toBe(false);
    expect(friendlyError('Gemini (g): HTTP 400: User location is not supported for the API use.').text).toContain('VPN');
  });

  it('keeps unknown messages as they are', () => {
    expect(friendlyError('Something new').text).toBe('Something new');
  });
});
