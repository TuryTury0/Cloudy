<script lang="ts">
  // A button that acts only after it is held for a moment while a ring fills around its icon: for actions that are
  // hard to undo, like shutting the PC down. Letting go early does nothing.
  import Icon, { type IconName } from './Icon.svelte';
  import { haptic } from './store.svelte';

  interface Props {
    icon: IconName;
    label: string;
    hold?: number;
    tone?: 'normal' | 'warn' | 'danger';
    disabled?: boolean;
    onconfirm: () => void;
  }

  let { icon, label, hold = 1200, tone = 'normal', disabled = false, onconfirm }: Props = $props();

  const RING = 2 * Math.PI * 21;
  let progress = $state(0);
  let frame = 0;
  let start = 0;
  let hint = $state(false);
  let hintTimer: ReturnType<typeof setTimeout> | undefined;

  function step(now: number) {
    progress = Math.min(1, (now - start) / hold);
    if (progress >= 1) {
      frame = 0;
      haptic(35);
      onconfirm();
      setTimeout(() => (progress = 0), 250);
      return;
    }
    frame = requestAnimationFrame(step);
  }

  function begin() {
    if (disabled || frame) return;
    hint = false;
    haptic(8);
    start = performance.now();
    frame = requestAnimationFrame(step);
  }

  function end() {
    if (!frame) return;
    cancelAnimationFrame(frame);
    frame = 0;
    if (progress < 1) {
      // A quick tap: say how it works.
      if (progress < 0.35) {
        hint = true;
        clearTimeout(hintTimer);
        hintTimer = setTimeout(() => (hint = false), 1600);
      }
      progress = 0;
    }
  }

  // Turned off mid-hold (the link dropped, another action started) or the app left the screen: the hold is void.
  $effect(() => {
    if (disabled) end();
  });

  $effect(() => {
    const away = () => document.visibilityState !== 'visible' && end();
    document.addEventListener('visibilitychange', away);
    return () => document.removeEventListener('visibilitychange', away);
  });

  function down(e: PointerEvent) {
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    begin();
  }

  function key(e: KeyboardEvent) {
    if ((e.key === ' ' || e.key === 'Enter') && !e.repeat) {
      e.preventDefault();
      begin();
    }
  }
</script>

<button
  class="hold {tone}"
  class:holding={progress > 0}
  {disabled}
  aria-label="{label} (نگه دار)"
  onpointerdown={down}
  onpointerup={end}
  onpointercancel={end}
  onlostpointercapture={end}
  onkeydown={key}
  onkeyup={end}
  oncontextmenu={(e) => e.preventDefault()}
>
  <span class="ring">
    <svg viewBox="0 0 48 48" aria-hidden="true">
      <circle class="track" cx="24" cy="24" r="21" />
      <circle class="fill" cx="24" cy="24" r="21" style="stroke-dasharray: {RING}; stroke-dashoffset: {(1 - progress) * RING}" />
    </svg>
    <Icon name={icon} size={24} />
  </span>
  <b>{label}</b>
  <small>{hint ? 'کمی نگه دار' : progress > 0 ? 'نگه دار…' : 'نگه دار'}</small>
</button>

<style>
  .hold {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 16px 8px 12px;
    border-radius: 22px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.09), rgba(255, 255, 255, 0.03));
    backdrop-filter: blur(20px) saturate(160%);
    -webkit-backdrop-filter: blur(20px) saturate(160%);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.14);
    color: var(--text);
    font: inherit;
    touch-action: manipulation;
    -webkit-touch-callout: none;
    transition:
      transform 200ms var(--ease-out),
      background 200ms ease;
  }
  .hold:disabled {
    opacity: 0.45;
  }
  .hold.holding {
    transform: scale(0.96);
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.14), rgba(255, 255, 255, 0.05));
  }
  .ring {
    position: relative;
    width: 52px;
    height: 52px;
    display: grid;
    place-items: center;
  }
  svg {
    position: absolute;
    inset: 0;
    transform: rotate(-90deg);
  }
  circle {
    fill: none;
    stroke-width: 3;
  }
  .track {
    stroke: rgba(255, 255, 255, 0.1);
  }
  .fill {
    stroke: var(--accent);
    stroke-linecap: round;
  }
  .warn .fill {
    stroke: #fbbf24;
  }
  .danger .fill {
    stroke: #fb7185;
  }
  .hold:not(.holding) .fill {
    transition: stroke-dashoffset 260ms ease;
  }
  b {
    font-size: 15px;
    font-weight: 750;
  }
  small {
    font-size: 11px;
    color: var(--text-dim);
  }
</style>
