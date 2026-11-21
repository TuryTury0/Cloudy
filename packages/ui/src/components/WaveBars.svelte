<script lang="ts">
  // Voice meter: bars follow the microphone (or speaker) level. Each bar has its own rhythm and the middle ones
  // rise highest, so the row looks like speech; a soft ripple keeps it alive in silence.
  import { onMount } from 'svelte';

  interface Props {
    level?: number;
    bars?: number;
    /** Tallest bar, in pixels. */
    height?: number;
  }

  let { level = 0, bars = 7, height = 22 }: Props = $props();

  let heights = $state<number[]>([]);

  onMount(() => {
    const current = Array.from({ length: bars }, () => 0.12);
    let frame = 0;
    let loud = 0;
    const tick = (now: number) => {
      // The level rises at once and falls slowly, like a real meter.
      loud = level > loud ? level : loud + (level - loud) * 0.12;
      for (let i = 0; i < bars; i++) {
        const middle = 1 - Math.abs(i - (bars - 1) / 2) / (bars / 2 + 1);
        const ripple = 0.1 + 0.06 * Math.sin(now / 260 + i * 0.8);
        const voice = loud * (0.35 + 0.65 * middle) * (0.62 + 0.38 * Math.sin(now / (70 + ((i * 37) % 50)) + i * 1.7));
        const target = Math.min(1, ripple + voice * 1.25);
        current[i] += (target - current[i]) * 0.4;
      }
      heights = [...current];
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  });
</script>

<div class="wave" style="height: {height}px" aria-hidden="true">
  {#each heights as h, i (i)}
    <span style="height: {Math.max(3, Math.round(h * height))}px; opacity: {0.55 + h * 0.45}"></span>
  {/each}
</div>

<style>
  .wave {
    display: flex;
    align-items: center;
    gap: 2.5px;
  }
  span {
    width: 3px;
    border-radius: 3px;
    background: linear-gradient(180deg, #fff, var(--accent-text));
    box-shadow: 0 0 7px var(--glow);
  }
</style>
