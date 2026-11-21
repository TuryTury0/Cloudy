import { describe, expect, it } from 'vitest';
import { ASSISTANT_STATES, answerLines, answerMaxHeight, autoReturnMs, isAssistantState, islandShape } from './island-layout';

describe('island layout', () => {
  it('has a shape for every state', () => {
    for (const state of ASSISTANT_STATES) {
      const shape = islandShape(state);
      expect(shape.width).toBeGreaterThan(100);
      expect(shape.height).toBeGreaterThanOrEqual(30);
      expect(shape.radius).toBeGreaterThan(0);
    }
  });

  it('is a small notch when idle and a taller band when active', () => {
    expect(islandShape('idle').height).toBeLessThan(islandShape('listening').height);
    expect(islandShape('idle').width).toBeLessThan(islandShape('thinking').width);
  });

  it('widens the idle notch on hover only', () => {
    expect(islandShape('idle', { hovered: true }).width).toBeGreaterThan(islandShape('idle').width);
    expect(islandShape('listening', { hovered: true }).width).toBe(islandShape('listening').width);
  });

  it('becomes a wider, rounder panel when it has a body', () => {
    const band = islandShape('working');
    const panel = islandShape('working', { expanded: true });
    expect(panel.width).toBeGreaterThan(band.width);
    expect(panel.radius).toBeGreaterThan(band.radius);
    expect(panel.height).toBe(band.height);
    expect(islandShape('idle', { expanded: true }).width).toBe(panel.width);
  });

  it('limits the answer text by its length', () => {
    expect(answerLines('salam')).toBe(1);
    expect(answerLines('a'.repeat(120))).toBe(3);
    expect(answerLines('one\ntwo\n\nthree')).toBe(3);
    expect(answerLines('x'.repeat(5000))).toBe(8);
    expect(answerMaxHeight('a'.repeat(200))).toBeGreaterThan(answerMaxHeight('ok'));
  });

  it('a confirmation question takes over the shape', () => {
    expect(islandShape('working', { expanded: true, confirm: true })).toEqual(islandShape('idle', { confirm: true }));
  });

  it('recognizes states and transient ones', () => {
    expect(isAssistantState('text-input')).toBe(true);
    expect(isAssistantState('answer')).toBe(true);
    expect(isAssistantState('sleeping')).toBe(false);
    expect(autoReturnMs('success')).toBeGreaterThan(0);
    expect(autoReturnMs('listening')).toBeNull();
    expect(autoReturnMs('answer', 'x'.repeat(300))!).toBeGreaterThan(autoReturnMs('answer', 'x')!);
    // A short answer: typed in, then about 1.5 s to read.
    expect(autoReturnMs('answer', 'x'.repeat(40))!).toBeLessThan(2500);
    expect(autoReturnMs('answer', 'x'.repeat(5000))!).toBeLessThanOrEqual(8000);
    expect(autoReturnMs('error', 'x'.repeat(10000))).toBe(9000);
  });
});
