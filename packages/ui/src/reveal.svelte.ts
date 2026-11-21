// Typewriter reveal for answers: the text appears quickly and smoothly, and long answers never take more than
// about two seconds. With "Reduce motion" (see applyMotion) the text appears at once.

export function revealSpeed(length: number): number {
  return Math.max(70, length / 2);
}

export class Reveal {
  shown = $state('');
  done = $state(true);
  #frame = 0;

  start(text: string, onDone?: () => void) {
    this.stop();
    const reduced = typeof document !== 'undefined' && document.documentElement.classList.contains('reduce-motion');
    if (reduced || typeof requestAnimationFrame !== 'function' || !text) {
      this.shown = text;
      this.done = true;
      onDone?.();
      return;
    }
    const chars = [...text];
    const speed = revealSpeed(chars.length);
    const started = performance.now();
    this.shown = '';
    this.done = false;
    const tick = (now: number) => {
      const count = Math.min(chars.length, Math.floor(((now - started) / 1000) * speed) + 1);
      this.shown = chars.slice(0, count).join('');
      if (count >= chars.length) {
        this.done = true;
        onDone?.();
        return;
      }
      this.#frame = requestAnimationFrame(tick);
    };
    this.#frame = requestAnimationFrame(tick);
  }

  /** Shows the whole text now. */
  finish(text: string) {
    this.stop();
    this.shown = text;
    this.done = true;
  }

  stop() {
    if (this.#frame) cancelAnimationFrame(this.#frame);
    this.#frame = 0;
  }
}
