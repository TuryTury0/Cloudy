// The island window is click-through except over its glass surfaces. This reports where those surfaces are
// (every element with a data-hit attribute) to the native side whenever they move or resize.
import { invoke } from './ipc';

export interface Region {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Stable text key for a set of regions, rounded to whole pixels, used to skip unchanged updates. */
export function regionsKey(regions: Region[]): string {
  return regions.map((r) => [r.x, r.y, r.width, r.height].map(Math.round).join(',')).join(';');
}

export function toRegions(rects: Array<Pick<DOMRect, 'left' | 'top' | 'width' | 'height'>>): Region[] {
  return rects
    .filter((r) => r.width > 0 && r.height > 0)
    .map((r) => ({ x: r.left, y: r.top, width: r.width, height: r.height }));
}

/** Starts reporting; about 30 times a second while anything moves, nothing when it does not. */
export function trackHitRegions(send: (regions: Region[]) => void = (regions) => void invoke('set_hit_regions', { regions })): () => void {
  let lastKey = '';
  let lastCheck = 0;
  let frame = 0;
  const tick = (now: number) => {
    if (now - lastCheck >= 33) {
      lastCheck = now;
      const regions = toRegions([...document.querySelectorAll<HTMLElement>('[data-hit]')].map((el) => el.getBoundingClientRect()));
      const key = regionsKey(regions);
      if (key !== lastKey) {
        lastKey = key;
        send(regions);
      }
    }
    frame = requestAnimationFrame(tick);
  };
  frame = requestAnimationFrame(tick);
  return () => cancelAnimationFrame(frame);
}
