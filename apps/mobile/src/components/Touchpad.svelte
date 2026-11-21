<script lang="ts">
  // The remote control tab: a touchpad that moves the PC's pointer (tap = click, two fingers = scroll or right
  // click, hold = drag), big left/right buttons, a keyboard that types on the PC, special keys, and an optional live
  // view of the PC's screen (tap it to click there).
  import { onMount } from 'svelte';
  import { fade, slide } from 'svelte/transition';
  import { invoke } from '../lib/ipc';
  import type { MouseButton, Screen } from '../lib/types';
  import Icon, { type IconName } from './Icon.svelte';
  import PageHead from './PageHead.svelte';
  import { haptic, sendInput, ui } from './store.svelte';
  import { faError } from './text';

  let { active }: { active: boolean } = $props();

  /** Pointer pixels per finger pixel, before acceleration. */
  const SENSITIVITY = 1.8;
  /** How far (px) a finger may wander and still tap. */
  const SLOP = 9;
  /** Movement below this (px) is a resting finger, not a move (keeps taps on target). */
  const DEAD = 3;
  const TAP_MS = 280;
  const LONG_MS = 450;
  /** Two-finger travel (px) before a wheel step is sent (as 3 wheel units per px: 40 px = one notch). */
  const WHEEL_STEP = 40;
  const SCREEN_EVERY = 800;
  const SAVED = 'anytask.screenView';

  // ---------- touchpad ----------

  interface Finger {
    x: number;
    y: number;
    sx: number;
    sy: number;
    t: number;
  }

  let pad = $state<HTMLDivElement>();
  const fingers = new Map<number, Finger>();
  let gesture: 'none' | 'one' | 'two' | 'drag' = 'none';
  let started = 0;
  let travel = 0;
  let most = 0;
  let longTimer: ReturnType<typeof setTimeout> | undefined;
  let accX = 0;
  let accY = 0;
  let wheelX = 0;
  let wheelY = 0;
  let frame = 0;
  let dragging = $state(false);
  let touches = $state<Array<{ id: number; x: number; y: number }>>([]);
  let ripples = $state<Array<{ id: number; x: number; y: number; right: boolean }>>([]);
  let rippleId = 0;

  function flush() {
    frame = 0;
    const dx = Math.trunc(accX);
    const dy = Math.trunc(accY);
    accX -= dx;
    accY -= dy;
    if (dx || dy) sendInput({ op: 'move', dx, dy });
    const wy = Math.abs(wheelY) >= WHEEL_STEP ? Math.round(wheelY * 3) : 0;
    // Natural scrolling: the page follows the fingers (fingers right shows what is on the left).
    const wx = Math.abs(wheelX) >= WHEEL_STEP ? Math.round(-wheelX * 3) : 0;
    if (wy) wheelY = 0;
    if (wx) wheelX = 0;
    if (wy || wx) sendInput(wx ? { op: 'wheel', dy: wy, dx: wx } : { op: 'wheel', dy: wy });
  }

  function schedule() {
    if (!frame) frame = requestAnimationFrame(flush);
  }

  function local(e: PointerEvent) {
    const r = pad!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  function ripple(e: PointerEvent, right = false) {
    const id = ++rippleId;
    ripples = [...ripples, { id, ...local(e), right }];
    setTimeout(() => (ripples = ripples.filter((r) => r.id !== id)), 500);
  }

  function showTouches() {
    const r = pad!.getBoundingClientRect();
    touches = [...fingers.entries()].map(([id, f]) => ({ id, x: f.x - r.left, y: f.y - r.top }));
  }

  function click(button: MouseButton) {
    sendInput({ op: 'click', button, count: 1 });
    haptic(button === 'right' ? 18 : 10);
  }

  function onDown(e: PointerEvent) {
    if (!ui.link.connected || !pad) return;
    e.preventDefault();
    pad.setPointerCapture(e.pointerId);
    fingers.set(e.pointerId, { x: e.clientX, y: e.clientY, sx: e.clientX, sy: e.clientY, t: e.timeStamp });
    most = Math.max(most, fingers.size);
    if (fingers.size === 1) {
      gesture = 'one';
      started = e.timeStamp;
      travel = 0;
      clearTimeout(longTimer);
      longTimer = setTimeout(() => {
        if (gesture === 'one' && travel < SLOP && fingers.size === 1) {
          gesture = 'drag';
          dragging = true;
          sendInput({ op: 'button', button: 'left', down: true });
          haptic(30);
        }
      }, LONG_MS);
    } else if (fingers.size === 2 && gesture === 'one') {
      clearTimeout(longTimer);
      gesture = 'two';
      wheelX = 0;
      wheelY = 0;
    }
    showTouches();
  }

  function onMove(e: PointerEvent) {
    const f = fingers.get(e.pointerId);
    if (!f) return;
    const dx = e.clientX - f.x;
    const dy = e.clientY - f.y;
    const dt = Math.max(1, e.timeStamp - f.t);
    f.x = e.clientX;
    f.y = e.clientY;
    f.t = e.timeStamp;
    travel = Math.max(travel, Math.hypot(f.x - f.sx, f.y - f.sy));
    showTouches();
    if (gesture === 'one' || gesture === 'drag') {
      if (gesture === 'one' && travel >= SLOP) clearTimeout(longTimer);
      if (gesture === 'one' && travel < DEAD) return;
      // Mild acceleration: slow moves are precise, fast flicks cross the screen.
      const speed = Math.hypot(dx, dy) / dt;
      const gain = SENSITIVITY * (1 + Math.min(1.6, Math.max(0, speed - 0.3) * 0.9));
      accX += dx * gain;
      accY += dy * gain;
      schedule();
    } else if (gesture === 'two') {
      // Each finger reports its own moves: half of each makes the pair's travel.
      wheelX += dx / 2;
      wheelY += dy / 2;
      schedule();
    }
  }

  function onUp(e: PointerEvent) {
    if (!fingers.delete(e.pointerId)) return;
    showTouches();
    if (fingers.size > 0) return;
    clearTimeout(longTimer);
    const quick = e.timeStamp - started < TAP_MS + (most > 1 ? 60 : 0);
    if (gesture === 'drag') {
      sendInput({ op: 'button', button: 'left', down: false });
      dragging = false;
      haptic(12);
    } else if (e.type === 'pointerup' && quick && travel < SLOP) {
      if (most > 1) {
        click('right');
        ripple(e, true);
      } else {
        click('left');
        ripple(e);
      }
    }
    gesture = 'none';
    most = 0;
  }

  onMount(() => {
    const el = pad!;
    el.addEventListener('pointerdown', onDown);
    el.addEventListener('pointermove', onMove);
    el.addEventListener('pointerup', onUp);
    el.addEventListener('pointercancel', onUp);
    const noMenu = (e: Event) => e.preventDefault();
    el.addEventListener('contextmenu', noMenu);
    return () => {
      el.removeEventListener('pointerdown', onDown);
      el.removeEventListener('pointermove', onMove);
      el.removeEventListener('pointerup', onUp);
      el.removeEventListener('pointercancel', onUp);
      el.removeEventListener('contextmenu', noMenu);
      cancelAnimationFrame(frame);
      clearTimeout(longTimer);
      if (dragging) sendInput({ op: 'button', button: 'left', down: false });
    };
  });

  // The link dropped mid-drag: forget the gesture and stop any repeating key (the PC lets go of its buttons itself).
  $effect(() => {
    if (ui.link.connected) return;
    fingers.clear();
    gesture = 'none';
    dragging = false;
    held = null;
    touches = [];
    clearTimeout(repeatTimer);
    pressed = null;
    repeating = false;
  });

  // ---------- left / right buttons (hold one and move on the pad to drag) ----------

  let held = $state<MouseButton | null>(null);

  function buttonDown(button: MouseButton, e: PointerEvent) {
    if (!ui.link.connected) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    held = button;
    sendInput({ op: 'button', button, down: true });
    haptic(button === 'right' ? 18 : 10);
  }

  function buttonUp(button: MouseButton) {
    if (held !== button) return;
    held = null;
    sendInput({ op: 'button', button, down: false });
  }

  /** Enter or Space on a focused button (pointer presses are handled above). */
  function buttonKey(button: MouseButton, e: MouseEvent) {
    if (e.detail === 0) click(button);
  }

  // ---------- keyboard ----------

  /** The field always keeps one space before the caret, so Backspace on an "empty" field still reaches the PC. */
  const SENTINEL = ' ';
  let field = $state<HTMLInputElement>();
  let last = SENTINEL;
  /** Nothing typed but the space: the field shows its own placeholder (the real one hides behind the space). */
  let empty = $state(true);
  let composing = false;
  let resetTimer: ReturnType<typeof setTimeout> | undefined;

  function keepShort() {
    clearTimeout(resetTimer);
    resetTimer = setTimeout(() => {
      if (composing || !field) return;
      field.value = SENTINEL;
      last = SENTINEL;
      empty = true;
    }, 1400);
  }

  /** Sends what changed in the field: deleted letters as Backspace, new ones as text (works with any IME). */
  function onType(e: Event & { currentTarget: HTMLInputElement }) {
    const el = e.currentTarget;
    const before = Array.from(last);
    const after = Array.from(el.value);
    let same = 0;
    while (same < before.length && same < after.length && before[same] === after[same]) same++;
    for (let i = same; i < before.length; i++) sendInput({ op: 'key', combo: 'backspace' });
    const added = after.slice(same).join('');
    if (added) sendInput({ op: 'text', text: added });
    last = el.value;
    if (!el.value) {
      el.value = SENTINEL;
      last = SENTINEL;
    }
    empty = el.value === SENTINEL;
    keepShort();
  }

  function onTypeKey(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      key('enter');
    }
  }

  function key(combo: string) {
    sendInput({ op: 'key', combo });
    haptic(8);
  }

  interface Key {
    combo: string;
    label: string;
    icon?: IconName;
    repeat?: boolean;
  }

  const KEYS: Key[] = [
    { combo: 'escape', label: 'Esc' },
    { combo: 'tab', label: 'Tab' },
    { combo: 'enter', label: 'Enter' },
    { combo: 'backspace', label: '⌫', repeat: true },
    { combo: 'left', label: '←', repeat: true },
    { combo: 'up', label: '↑', repeat: true },
    { combo: 'down', label: '↓', repeat: true },
    { combo: 'right', label: '→', repeat: true },
    { combo: 'win', label: 'Win' },
    { combo: 'ctrl+c', label: 'Ctrl+C' },
    { combo: 'ctrl+v', label: 'Ctrl+V' },
    { combo: 'ctrl+z', label: 'Ctrl+Z' },
    { combo: 'alt+tab', label: 'Alt+Tab' }
  ];
  const MEDIA: Key[] = [
    { combo: 'volume_down', label: 'کم کردن صدا', icon: 'volDown', repeat: true },
    { combo: 'volume_mute', label: 'بی‌صدا', icon: 'mute' },
    { combo: 'volume_up', label: 'زیاد کردن صدا', icon: 'volUp', repeat: true },
    { combo: 'media_play_pause', label: 'پخش / مکث', icon: 'playPause' }
  ];

  let repeatTimer: ReturnType<typeof setTimeout> | undefined;
  let pressed: Key | null = null;
  let repeating = false;

  // A key goes out when the finger lifts (a swipe that scrolls the row or the page cancels it and sends nothing);
  // held, a repeating key (arrows, Backspace, volume) starts repeating.
  function keyDown(k: Key) {
    if (!ui.link.connected) return;
    clearTimeout(repeatTimer);
    pressed = k;
    repeating = false;
    if (!k.repeat) return;
    const again = (wait: number) =>
      (repeatTimer = setTimeout(() => {
        repeating = true;
        key(k.combo);
        again(80);
      }, wait));
    again(420);
  }

  function keyUp(k: Key) {
    clearTimeout(repeatTimer);
    if (pressed === k && !repeating) key(k.combo);
    pressed = null;
    repeating = false;
  }

  function keyCancel() {
    clearTimeout(repeatTimer);
    pressed = null;
    repeating = false;
  }

  function keyClick(k: Key, e: MouseEvent) {
    if (e.detail === 0) key(k.combo);
  }

  /** Lets go of everything held: a drag on the pad, a held left/right button, a repeating key. Called when the tab
   * is left, the app goes to the background, the window loses focus or the link drops (no pointerup may come). */
  function release() {
    if (dragging) sendInput({ op: 'button', button: 'left', down: false });
    if (held) sendInput({ op: 'button', button: held, down: false });
    dragging = false;
    held = null;
    fingers.clear();
    gesture = 'none';
    touches = [];
    clearTimeout(longTimer);
    keyCancel();
  }

  $effect(() => {
    if (!active) release();
  });

  onMount(() => {
    const away = () => document.visibilityState !== 'visible' && release();
    window.addEventListener('blur', release);
    document.addEventListener('visibilitychange', away);
    return () => {
      window.removeEventListener('blur', release);
      document.removeEventListener('visibilitychange', away);
    };
  });

  // ---------- screen view ----------

  function saved() {
    try {
      return localStorage.getItem(SAVED) === '1';
    } catch {
      return false;
    }
  }

  let showScreen = $state(saved());
  let shot = $state<{ src: string; w: number; h: number; cursor: [number, number] | null } | null>(null);
  let screenError = $state('');
  let visible = $state(document.visibilityState === 'visible');
  let img = $state<HTMLImageElement>();

  function toggleScreen() {
    showScreen = !showScreen;
    haptic();
    try {
      localStorage.setItem(SAVED, showScreen ? '1' : '0');
    } catch {
      // not kept
    }
  }

  onMount(() => {
    const onVisible = () => (visible = document.visibilityState === 'visible');
    document.addEventListener('visibilitychange', onVisible);
    return () => document.removeEventListener('visibilitychange', onVisible);
  });

  // Polls the PC's screen only while it is seen: this tab open, the view on, the app in front, the link up.
  // One request at a time; the next one waits for the answer.
  $effect(() => {
    if (!(active && showScreen && visible && ui.link.connected)) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const next = async () => {
      let wait = SCREEN_EVERY;
      try {
        const s = await invoke<Screen>('screen', { width: 960 });
        if (stopped) return;
        if (s) shot = { src: `data:image/jpeg;base64,${s.jpeg}`, w: s.w, h: s.h, cursor: s.cursor };
        screenError = '';
      } catch (e) {
        if (stopped) return;
        screenError = faError(e);
        wait = 2500;
      }
      timer = setTimeout(() => void next(), wait);
    };
    void next();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  });

  function tapScreen(e: MouseEvent, button: MouseButton = 'left') {
    if (!img || !ui.link.connected) return;
    if (e.detail === 0 && button === 'left') return;
    const r = img.getBoundingClientRect();
    const x = Math.round(((e.clientX - r.left) / r.width) * 1000);
    const y = Math.round(((e.clientY - r.top) / r.height) * 1000);
    if (x < 0 || y < 0 || x > 1000 || y > 1000) return;
    sendInput({ op: 'tap', x, y, button });
    haptic(button === 'right' ? 18 : 10);
    if (shot) shot.cursor = [x, y];
  }
</script>

<div class="page-scroll">
  <PageHead title="کنترل">
    {#snippet actions()}
      <button class="toggle" class:on={showScreen} onclick={toggleScreen} aria-pressed={showScreen}>
        <Icon name="eye" size={17} />دیدن صفحه
      </button>
    {/snippet}
  </PageHead>

  <div class="body">
    {#if showScreen}
      <div class="screen glass" transition:slide={{ duration: 260 }}>
        {#if shot}
          <button
            class="shot"
            style="aspect-ratio: {shot.w} / {shot.h}"
            onclick={(e) => tapScreen(e)}
            oncontextmenu={(e) => (e.preventDefault(), tapScreen(e, 'right'))}
            aria-label="صفحه کامپیوتر؛ لمس کن تا همان‌جا کلیک شود"
          >
            <img bind:this={img} src={shot.src} alt="" draggable="false" />
            {#if shot.cursor}
              <i class="cursor" style="left: {shot.cursor[0] / 10}%; top: {shot.cursor[1] / 10}%"></i>
            {/if}
          </button>
        {:else}
          <div class="shot wait"><span class="spin"></span></div>
        {/if}
        {#if screenError}<p class="small-error" transition:fade>{screenError}</p>{/if}
      </div>
    {/if}

    <div class="pad glass" class:dragging bind:this={pad}>
      <span class="grid" aria-hidden="true"></span>
      {#each touches as t (t.id)}
        <i class="touch" style="left: {t.x}px; top: {t.y}px"></i>
      {/each}
      {#each ripples as r (r.id)}
        <i class="ripple" class:right={r.right} style="left: {r.x}px; top: {r.y}px"></i>
      {/each}
      <p class="hint" aria-hidden="true">
        {#if dragging}در حال کشیدن… رها کن تا تمام شود{:else}ضربه: کلیک · دو انگشت: اسکرول یا کلیک راست · نگه داشتن: کشیدن{/if}
      </p>
      {#if !ui.link.connected}
        <div class="offline" transition:fade={{ duration: 200 }}>
          <span class="spin"></span>
          <span>در حال اتصال…</span>
        </div>
      {/if}
    </div>

    <div class="buttons">
      <button
        class="mouse glass"
        class:down={held === 'left'}
        disabled={!ui.link.connected}
        onpointerdown={(e) => buttonDown('left', e)}
        onpointerup={() => buttonUp('left')}
        onpointercancel={() => buttonUp('left')}
        onclick={(e) => buttonKey('left', e)}
        oncontextmenu={(e) => e.preventDefault()}
      >
        چپ
      </button>
      <button
        class="mouse glass"
        class:down={held === 'right'}
        disabled={!ui.link.connected}
        onpointerdown={(e) => buttonDown('right', e)}
        onpointerup={() => buttonUp('right')}
        onpointercancel={() => buttonUp('right')}
        onclick={(e) => buttonKey('right', e)}
        oncontextmenu={(e) => e.preventDefault()}
      >
        راست
      </button>
    </div>

    <div class="type glass">
      <Icon name="keyboard" size={20} />
      {#if empty}<span class="placeholder" aria-hidden="true">روی کامپیوتر تایپ کن</span>{/if}
      <input
        bind:this={field}
        value={SENTINEL}
        oninput={onType}
        onkeydown={onTypeKey}
        oncompositionstart={() => (composing = true)}
        oncompositionend={() => ((composing = false), keepShort())}
        onfocus={(e) => e.currentTarget.setSelectionRange(e.currentTarget.value.length, e.currentTarget.value.length)}
        aria-label="روی کامپیوتر تایپ کن"
        dir="auto"
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        enterkeyhint="enter"
        disabled={!ui.link.connected}
      />
    </div>

    <div class="keys" dir="ltr">
      {#each KEYS as k (k.combo)}
        <button
          class="key"
          disabled={!ui.link.connected}
          onpointerdown={() => keyDown(k)}
          onpointerup={() => keyUp(k)}
          onpointercancel={keyCancel}
          onpointerleave={keyCancel}
          onclick={(e) => keyClick(k, e)}
          oncontextmenu={(e) => e.preventDefault()}
        >
          {k.label}
        </button>
      {/each}
    </div>
    <div class="media" dir="ltr">
      {#each MEDIA as k (k.combo)}
        <button
          class="key media-key"
          disabled={!ui.link.connected}
          aria-label={k.label}
          onpointerdown={() => keyDown(k)}
          onpointerup={() => keyUp(k)}
          onpointercancel={keyCancel}
          onpointerleave={keyCancel}
          onclick={(e) => keyClick(k, e)}
          oncontextmenu={(e) => e.preventDefault()}
        >
          {#if k.icon}<Icon name={k.icon} size={20} />{/if}
        </button>
      {/each}
    </div>
  </div>
</div>

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 4px 12px 12px;
    flex: 1;
    min-height: 0;
  }
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 7px 12px;
    border-radius: 999px;
    border: 1px solid rgba(255, 255, 255, 0.12);
    background: rgba(255, 255, 255, 0.06);
    color: var(--text-muted);
    font: inherit;
    font-size: 12.5px;
    font-weight: 650;
    transition:
      background 200ms ease,
      color 200ms ease;
  }
  .toggle.on {
    background: #fff;
    color: #000;
    border-color: transparent;
  }
  .screen {
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .shot {
    position: relative;
    display: block;
    width: 100%;
    padding: 0;
    border: none;
    border-radius: 14px;
    overflow: hidden;
    background: #000;
    -webkit-touch-callout: none;
  }
  .shot:active {
    /* No press shrink here: the tap point must match the picture. */
    transform: none;
  }
  .shot img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
    pointer-events: none;
  }
  .shot.wait {
    aspect-ratio: 16 / 9;
    display: grid;
    place-items: center;
  }
  .cursor {
    position: absolute;
    width: 14px;
    height: 14px;
    margin: -7px 0 0 -7px;
    border-radius: 50%;
    background: var(--accent);
    border: 2px solid #fff;
    box-shadow: 0 0 12px var(--accent);
    transition:
      left 300ms var(--ease-out),
      top 300ms var(--ease-out);
    pointer-events: none;
  }
  .small-error {
    margin: 0;
    font-size: 12px;
    color: #fecaca;
    text-align: center;
  }
  .pad {
    position: relative;
    flex: 1;
    min-height: 210px;
    overflow: hidden;
    touch-action: none;
    -webkit-touch-callout: none;
    border-radius: 26px;
    transition: box-shadow 250ms ease;
  }
  .pad.dragging {
    box-shadow:
      inset 0 0 0 2px var(--accent),
      0 0 30px -8px var(--glow);
  }
  .grid {
    position: absolute;
    inset: 0;
    background-image: radial-gradient(rgba(255, 255, 255, 0.09) 1px, transparent 1.4px);
    background-size: 22px 22px;
    mask-image: radial-gradient(ellipse at center, #000 30%, transparent 85%);
    -webkit-mask-image: radial-gradient(ellipse at center, #000 30%, transparent 85%);
    pointer-events: none;
  }
  .touch {
    position: absolute;
    width: 64px;
    height: 64px;
    margin: -32px 0 0 -32px;
    border-radius: 50%;
    background: radial-gradient(circle, color-mix(in srgb, var(--accent) 45%, transparent), transparent 70%);
    pointer-events: none;
  }
  .ripple {
    position: absolute;
    width: 80px;
    height: 80px;
    margin: -40px 0 0 -40px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.7);
    animation: ripple 480ms var(--ease-out) forwards;
    pointer-events: none;
  }
  .ripple.right {
    border-color: var(--accent);
  }
  .hint {
    position: absolute;
    inset-inline: 0;
    bottom: 12px;
    margin: 0;
    padding: 0 16px;
    text-align: center;
    font-size: 11.5px;
    color: var(--text-dim);
    pointer-events: none;
  }
  .offline {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    background: rgba(7, 7, 12, 0.55);
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);
    color: var(--text-muted);
    font-size: 14px;
  }
  .buttons {
    display: flex;
    gap: 10px;
  }
  .mouse {
    flex: 1;
    height: 58px;
    color: var(--text);
    font: inherit;
    font-size: 16px;
    font-weight: 750;
    border-radius: 20px;
    touch-action: none;
    -webkit-touch-callout: none;
    transition:
      transform 120ms ease,
      background 120ms ease;
  }
  .mouse.down {
    transform: scale(0.97);
    background: rgba(255, 255, 255, 0.18);
  }
  .mouse:disabled {
    opacity: 0.5;
  }
  .type {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 14px;
    border-radius: 20px;
    color: var(--text-dim);
  }
  .placeholder {
    position: absolute;
    inset-inline-start: 42px;
    top: 50%;
    transform: translateY(-50%);
    font-size: 15px;
    color: var(--text-dim);
    pointer-events: none;
  }
  .type input {
    flex: 1;
    min-width: 0;
    padding: 12px 0;
    border: none;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 16px;
    outline: none;
  }
  .keys {
    display: flex;
    gap: 8px;
    overflow-x: auto;
    padding: 2px 0 4px;
    scrollbar-width: none;
  }
  .keys::-webkit-scrollbar {
    display: none;
  }
  .key {
    flex-shrink: 0;
    min-width: 50px;
    height: 44px;
    padding: 0 12px;
    border-radius: 14px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    background: rgba(255, 255, 255, 0.07);
    color: var(--text);
    font-family: var(--font-latin);
    font-size: 14px;
    font-weight: 650;
    touch-action: manipulation;
    -webkit-touch-callout: none;
  }
  .key:active:not(:disabled) {
    background: rgba(255, 255, 255, 0.2);
  }
  .key:disabled {
    opacity: 0.45;
  }
  .media {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 8px;
  }
  .media-key {
    display: grid;
    place-items: center;
  }
  @keyframes ripple {
    from {
      transform: scale(0.3);
      opacity: 1;
    }
    to {
      transform: scale(1.4);
      opacity: 0;
    }
  }
</style>
