<script lang="ts">
  // The island: a black notch that hangs from the top edge of the screen (its top corners flare outward like
  // shoulders) and morphs with spring physics between the assistant states: small notch -> voice band with a
  // live wave -> thinking shimmer -> panel with the request, live steps and the answer card. A glow in the theme
  // colours flows behind its edge and breathes with the voice. Before a risky step it becomes a question panel.
  import { Spring } from 'svelte/motion';
  import { blur, fade, fly, scale } from 'svelte/transition';
  import { flip } from 'svelte/animate';
  import { backOut, cubicOut } from 'svelte/easing';
  import Aurora from './Aurora.svelte';
  import BotFace from './BotFace.svelte';
  import WaveBars from './WaveBars.svelte';
  import { answerMaxHeight, islandShape, type AssistantState, type ConfirmQuestion, type IslandStep } from '../island-layout';
  import { L, i18n } from '../i18n.svelte';
  import { plainText } from '../markdown';
  import { Reveal } from '../reveal.svelte';

  interface Props {
    mode?: AssistantState;
    name?: string;
    detail?: string;
    steps?: IslandStep[];
    level?: number;
    colors?: string[];
    placeholder?: string;
    /** What the user asked, shown above the steps and the answer. */
    request?: string;
    /** The written answer (Markdown) for the answer card. */
    answer?: string;
    confirm?: ConfirmQuestion | null;
    onsubmit?: (text: string) => void;
    oncancel?: () => void;
    onactivate?: () => void;
    onconfirm?: (allow: boolean) => void;
    ondismiss?: () => void;
    onhover?: (hovered: boolean) => void;
    /** The user wants to ask something after an answer. */
    onfollowup?: () => void;
  }

  let {
    mode = 'idle',
    name = 'Cloudy',
    detail = '',
    steps = [],
    level = 0,
    colors = ['#38bdf8', '#ffffff', '#bae6fd'],
    placeholder = '',
    request = '',
    answer = '',
    confirm = null,
    onsubmit,
    oncancel,
    onactivate,
    onconfirm,
    ondismiss,
    onhover,
    onfollowup
  }: Props = $props();

  /** The core asks in English; these are the Persian wordings. */
  const CONFIRM_TITLES_FA: Record<string, string> = {
    'Overwrite this file?': 'فایل قبلی جایگزین شود؟',
    'Add to this file?': 'به این فایل اضافه شود؟',
    'Write a script file?': 'فایل اسکریپت نوشته شود؟'
  };

  let hovered = $state(false);
  let text = $state('');
  let input = $state<HTMLInputElement>();
  let bodyHeight = $state(0);
  let copied = $state(false);
  let seconds = $state(0);
  const reveal = new Reveal();

  const answerText = $derived(plainText(answer));
  const voice = $derived(mode === 'listening' || mode === 'speaking');
  const showSteps = $derived((mode === 'working' || mode === 'thinking') && steps.length > 0 && !confirm);
  const visibleSteps = $derived(steps.slice(-4));
  /** The request, shown while it is worked on and above its answer. */
  const requestText = $derived(
    confirm ? '' : mode === 'thinking' ? request || detail : mode === 'working' || mode === 'answer' ? request : ''
  );
  /** What is heard or said right now. */
  const spoken = $derived(voice && !confirm ? detail : '');
  const errorText = $derived(mode === 'error' && !confirm ? detail : '');
  const expanded = $derived(
    !!confirm || mode === 'text-input' || mode === 'answer' || showSteps || !!requestText || !!spoken || !!errorText
  );
  const shape = $derived(islandShape(mode, { expanded, hovered, confirm: !!confirm }));
  const faceSize = $derived(mode === 'idle' && !hovered && !confirm ? 18 : 22);

  // A little overshoot on every change makes the shape feel alive; the radius is stiffer so corners never wobble.
  const lively = { stiffness: 0.15, damping: 0.52 };
  const width = new Spring(152, lively);
  const height = new Spring(30, lively);
  const band = new Spring(30, lively);
  const radius = new Spring(13, { stiffness: 0.2, damping: 0.8 });
  const faceX = new Spring(67, lively);

  $effect(() => {
    // "Reduce motion" snaps to the new shape instead of springing.
    const instant = typeof document !== 'undefined' && document.documentElement.classList.contains('reduce-motion');
    // While a voice is heard the band swells a little with it.
    const swell = voice && !expanded ? Math.min(1, level) * 18 : 0;
    void width.set(shape.width + swell, { instant });
    void band.set(shape.height, { instant });
    void height.set(shape.height + (expanded ? bodyHeight : 0), { instant });
    void radius.set(shape.radius, { instant });
    // Alone in the idle notch the face sits in the middle; with anything beside it, it moves to the left.
    void faceX.set(mode === 'idle' && !hovered && !confirm ? (shape.width - faceSize) / 2 : 10, { instant });
  });

  $effect(() => {
    if (mode === 'text-input') {
      text = '';
      queueMicrotask(() => input?.focus());
    }
  });

  $effect(() => {
    if (mode === 'answer') {
      copied = false;
      reveal.start(answerText);
    } else {
      reveal.stop();
    }
  });

  // How long the microphone has been open.
  $effect(() => {
    if (mode !== 'listening') return;
    seconds = 0;
    const started = Date.now();
    const timer = setInterval(() => (seconds = Math.floor((Date.now() - started) / 1000)), 250);
    return () => clearInterval(timer);
  });
  const elapsed = $derived(`${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`);

  const glow = $derived(
    confirm
      ? 0.85
      : mode === 'idle'
        ? hovered
          ? 0.6
          : 0.32
        : voice
          ? 0.9
          : mode === 'thinking' || mode === 'working'
            ? 0.8
            : mode === 'answer' || mode === 'text-input'
              ? 0.55
              : 0.75
  );
  const glowColors = $derived(
    confirm
      ? ['#fbbf24', '#f97316', '#7c2d12']
      : mode === 'error'
        ? ['#f87171', '#b91c1c', '#7f1d1d']
        : mode === 'success'
          ? ['#6ee7b7', '#10b981', '#064e3b']
          : colors
  );

  const label = $derived.by(() => {
    if (confirm) return (i18n.lang === 'fa' && CONFIRM_TITLES_FA[confirm.title]) || confirm.title;
    switch (mode) {
      case 'idle':
        return hovered ? L('Ask anything', 'هر چی می‌خوای بپرس') : '';
      case 'listening':
        return '';
      case 'speaking':
        return '';
      case 'thinking':
        return L('Thinking…', 'فکر می‌کنم…');
      case 'working':
        return detail || L('Working…', 'در حال انجام…');
      case 'success':
        return detail || L('Done', 'انجام شد');
      case 'error':
        return L('Something went wrong', 'مشکلی پیش آمد');
      case 'text-input':
        return L(`Ask ${name}`, `از ${name} بپرس`);
      default:
        return name;
    }
  });
  const shimmering = $derived(!confirm && (mode === 'thinking' || mode === 'working'));

  function onKey(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.isComposing) {
      event.preventDefault();
      send();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      oncancel?.();
    }
  }

  function send() {
    const value = text.trim();
    if (value) onsubmit?.(value);
  }

  function setHovered(value: boolean) {
    hovered = value;
    onhover?.(value);
  }

  async function copyAnswer() {
    try {
      await navigator.clipboard.writeText(answerText);
      copied = true;
      setTimeout(() => (copied = false), 1600);
    } catch {
      // The text can still be selected and copied by hand.
    }
  }

  /** Runs a button's action without also "clicking the island". */
  const act = (run: () => void) => (event: MouseEvent) => {
    event.stopPropagation();
    run();
  };

  const interactive = $derived(mode === 'text-input' || mode === 'answer' || !!confirm);
</script>

<div
  class="island {mode}"
  class:confirming={!!confirm}
  class:expanded
  class:hovered
  data-hit
  dir="ltr"
  role="button"
  tabindex="-1"
  style="width: {width.current}px; height: {height.current}px; --shoulder: {Math.max(4, radius.current)}px; --level: {voice ? Math.min(1, level) : 0}; --glow-strength: {glow};"
  onmouseenter={() => setHovered(true)}
  onmouseleave={() => setHovered(false)}
  onclick={() => !interactive && onactivate?.()}
  onkeydown={() => {}}
>
  <!-- The living edge: the theme colours flow behind the notch and only their blurred fringe shows. -->
  <div class="halo"><Aurora colors={glowColors} {level} intensity={1} speed={mode === 'thinking' ? 2.4 : voice ? 1.6 : 1} /></div>

  <div class="core">
    <div class="band" style="height: {band.current}px">
      <div class="face" style="left: {faceX.current}px">
        {#if confirm}
          <div class="shield" in:scale={{ start: 0.4, duration: 420, easing: backOut }}>
            <svg viewBox="0 0 24 24"><path d="M12 3l8 3.5v5.5c0 4.4-3.4 8.2-8 9-4.6-.8-8-4.6-8-9V6.5z" /><path d="M12 8v4.5M12 16h.01" /></svg>
          </div>
        {:else}
          <BotFace {mode} {level} size={faceSize} />
        {/if}
      </div>

      <div class="center">
        {#key confirm ? 'confirm' : `${mode}:${mode === 'idle' && hovered}`}
          <div class="label" in:blur={{ amount: 5, duration: 280, delay: 70, easing: cubicOut }} out:fade={{ duration: 110 }}>
            {#if voice && !confirm}
              <WaveBars {level} bars={expanded ? 16 : 22} height={20} />
            {:else if label}
              <span class="title" class:shimmer={shimmering} class:hint={mode === 'idle'} dir="auto">{label}</span>
            {/if}
          </div>
        {/key}
      </div>

      <div class="side">
        {#if confirm}
          <span class="pulse-ring" in:fade></span>
        {:else if mode === 'listening'}
          <span class="timer" in:fade={{ duration: 200 }}><i class="rec"></i>{elapsed}</span>
        {:else if mode === 'thinking' || mode === 'working'}
          <div class="spinner" in:fade={{ duration: 200 }}></div>
        {:else if mode === 'success'}
          <svg class="check" viewBox="0 0 24 24" in:scale={{ start: 0.3, duration: 360, easing: backOut }}><path d="M5 12.5l4.2 4.2L19 7" /></svg>
        {:else if mode === 'answer' || mode === 'text-input'}
          <button
            class="icon-btn"
            aria-label={L('Close', 'بستن')}
            onclick={act(() => (mode === 'answer' ? ondismiss?.() : oncancel?.()))}
            in:scale={{ start: 0.5, duration: 260, easing: backOut }}
          >
            <svg viewBox="0 0 12 12"><path d="M3 3l6 6M9 3l-6 6" /></svg>
          </button>
        {/if}
      </div>
    </div>

    <div class="body-clip">
      <div class="body" style="width: {shape.width}px" bind:clientHeight={bodyHeight}>
        {#if expanded}
          <div class="inner">
            {#if requestText}
              <div class="bubble" dir="auto" in:fly={{ y: -6, duration: 280, easing: cubicOut }}>
                <svg viewBox="0 0 24 24"><path d="M5 12h14M13 6l6 6-6 6" /></svg>
                <span>{requestText}</span>
              </div>
            {/if}

            {#if spoken}
              <div class="bubble" class:said={mode === 'speaking'} dir="auto" in:fly={{ y: -6, duration: 280, easing: cubicOut }}>
                <svg viewBox="0 0 24 24">
                  {#if mode === 'speaking'}
                    <path d="M4 10v4h4l5 4V6L8 10zM16.5 9a4 4 0 010 6" />
                  {:else}
                    <path d="M12 3a3 3 0 00-3 3v6a3 3 0 006 0V6a3 3 0 00-3-3zM6 11a6 6 0 0012 0M12 17v4" />
                  {/if}
                </svg>
                <span>{spoken}</span>
              </div>
            {/if}

            {#if showSteps}
              <ol class="steps">
                {#each visibleSteps as step, i (step.id)}
                  <li
                    class={step.status}
                    class:latest={i === visibleSteps.length - 1}
                    dir="auto"
                    animate:flip={{ duration: 280 }}
                    in:fly={{ x: -16, duration: 320, easing: cubicOut }}
                  >
                    <span class="mark">
                      {#if step.status === 'running'}
                        <span class="ring"></span>
                      {:else if step.status === 'done'}
                        <svg viewBox="0 0 24 24" in:scale={{ start: 0.3, duration: 320, easing: backOut }}><path d="M5 12.5l4.2 4.2L19 7" /></svg>
                      {:else}
                        <svg viewBox="0 0 24 24" in:scale={{ start: 0.3, duration: 320, easing: backOut }}><path d="M7 7l10 10M17 7L7 17" /></svg>
                      {/if}
                    </span>
                    <span class="step-label">{step.label}</span>
                  </li>
                {/each}
              </ol>
            {/if}

            {#if errorText}
              <p class="error-text" dir="auto" in:fly={{ y: -6, duration: 280, easing: cubicOut }}>{errorText}</p>
            {/if}

            {#if confirm}
              <p class="confirm-detail" title={confirm.detail} in:fly={{ y: -6, duration: 280, easing: cubicOut }}>{confirm.detail}</p>
              <div class="actions end" in:fly={{ y: 8, duration: 320, delay: 100, easing: cubicOut }}>
                <button class="chip" onclick={act(() => onconfirm?.(false))}>{L("Don't allow", 'اجازه نده')}</button>
                <button class="chip warn" onclick={act(() => onconfirm?.(true))}>{L('Allow', 'اجازه بده')}</button>
              </div>
            {:else if mode === 'answer'}
              <div class="card" in:fly={{ y: 10, duration: 360, easing: cubicOut }}>
                <div class="card-head">
                  <svg viewBox="0 0 24 24"><path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8zM18.5 15l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z" /></svg>
                  {L('Answer', 'جواب')}
                </div>
                <!-- One line on purpose: the text keeps its line breaks (pre-wrap), so spaces here would show. -->
                <div class="answer-text" dir="auto" style="max-height: {answerMaxHeight(answerText)}px">{reveal.shown}{#if !reveal.done}<span class="caret"></span>{/if}</div>
                {#if reveal.done}
                  <div class="actions" in:fly={{ y: 6, duration: 260, easing: cubicOut }}>
                    <button class="chip" onclick={act(copyAnswer)}>
                      {#if copied}
                        <svg viewBox="0 0 24 24" in:scale={{ start: 0.3, duration: 260, easing: backOut }}><path d="M5 12.5l4.2 4.2L19 7" /></svg>
                        {L('Copied', 'کپی شد')}
                      {:else}
                        <svg viewBox="0 0 24 24"><rect x="9" y="9" width="11" height="11" rx="2.5" /><path d="M5 15V6.5A2.5 2.5 0 017.5 4H15" /></svg>
                        {L('Copy', 'کپی')}
                      {/if}
                    </button>
                  </div>
                {/if}
              </div>
              <button class="composer ghost" onclick={act(() => onfollowup?.())} in:fly={{ y: 10, duration: 360, delay: 60, easing: cubicOut }}>
                <span dir="auto">{L('Ask a follow-up…', 'ادامه بده…')}</span>
                <span class="send small"><svg viewBox="0 0 24 24"><path d="M12 19V5M6 11l6-6 6 6" /></svg></span>
              </button>
            {:else if mode === 'text-input'}
              <div class="composer" in:fly={{ y: -6, duration: 280, easing: cubicOut }}>
                <input
                  bind:this={input}
                  bind:value={text}
                  dir="auto"
                  placeholder={placeholder || L('Ask anything…', 'چه کاری انجام بدهم؟')}
                  onkeydown={onKey}
                  spellcheck="false"
                  autocomplete="off"
                />
                <button class="send" aria-label={L('Send', 'ارسال')} disabled={!text.trim()} onclick={act(send)}>
                  <svg viewBox="0 0 24 24"><path d="M12 19V5M6 11l6-6 6 6" /></svg>
                </button>
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .island {
    --notch: #020204;
    position: relative;
    isolation: isolate;
    cursor: pointer;
    color: #f5f6f8;
    font-size: 13px;
    /* It drops out of the top edge when it first shows. */
    animation: drop 0.75s var(--ease-spring) backwards;
  }
  .island.text-input,
  .island.answer,
  .island.confirming {
    cursor: default;
  }
  .island.error {
    animation: nudge 0.45s var(--ease-out);
  }

  /* Concave shoulders: the notch flows out of the top edge of the screen instead of ending in a corner. */
  .island::before,
  .island::after {
    content: '';
    position: absolute;
    top: 0;
    width: var(--shoulder);
    height: var(--shoulder);
    pointer-events: none;
  }
  .island::before {
    left: calc(-1 * var(--shoulder) + 0.5px);
    background: radial-gradient(circle at 0 100%, transparent calc(var(--shoulder) - 0.5px), var(--notch) var(--shoulder));
  }
  .island::after {
    right: calc(-1 * var(--shoulder) + 0.5px);
    background: radial-gradient(circle at 100% 100%, transparent calc(var(--shoulder) - 0.5px), var(--notch) var(--shoulder));
  }

  .halo {
    position: absolute;
    inset: 10px -9px -11px;
    z-index: -1;
    border-radius: 0 0 calc(var(--shoulder) + 10px) calc(var(--shoulder) + 10px);
    overflow: hidden;
    filter: blur(14px) saturate(1.6);
    opacity: calc(var(--glow-strength) + var(--level) * 0.4);
    transform: scale(calc(1 + var(--level) * 0.03), calc(1 + var(--level) * 0.16));
    transform-origin: 50% 0;
    transition: opacity var(--slow) var(--ease-out), transform 110ms linear;
    pointer-events: none;
    animation: breathe 5.5s ease-in-out infinite;
  }

  .core {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    border-radius: 0 0 var(--shoulder) var(--shoulder);
    background: var(--notch);
    box-shadow: inset 0 -1px 0 rgba(255, 255, 255, 0.09), inset 1px 0 0 rgba(255, 255, 255, 0.035), inset -1px 0 0 rgba(255, 255, 255, 0.035);
  }
  /* A wash of the theme colour rises from the bottom edge when the island is doing something. */
  .core::after {
    content: '';
    position: absolute;
    inset: auto 0 0;
    height: 70px;
    background: radial-gradient(70% 100% at 50% 100%, color-mix(in srgb, var(--accent-text) 16%, transparent), transparent 75%);
    opacity: 0;
    transition: opacity var(--slow) var(--ease-out);
    pointer-events: none;
  }
  .island:not(.idle) .core::after,
  .island.hovered .core::after {
    opacity: 1;
  }
  /* A thin light along the bottom edge; while the assistant thinks or works it runs from side to side. */
  .core::before {
    content: '';
    position: absolute;
    inset: auto 0 0;
    height: 1.5px;
    z-index: 2;
    background: linear-gradient(90deg, transparent 0%, color-mix(in srgb, var(--accent-text) 85%, #fff) 50%, transparent 100%) 50% 0 / 60% 100% no-repeat;
    opacity: calc(0.25 + var(--level) * 0.7);
    transition: opacity var(--medium);
    pointer-events: none;
  }
  .island.thinking .core::before,
  .island.working .core::before {
    background-size: 35% 100%;
    opacity: 0.95;
    animation: sweep 1.5s ease-in-out infinite alternate;
  }
  .island.listening .core::before,
  .island.speaking .core::before {
    opacity: calc(0.45 + var(--level) * 0.55);
  }
  .island.error .core::before {
    background-image: linear-gradient(90deg, transparent, #f87171, transparent);
    opacity: 0.9;
  }
  .island.confirming .core::before {
    background-image: linear-gradient(90deg, transparent, #fbbf24, transparent);
    opacity: 0.9;
  }
  .island.success .core::before {
    background-image: linear-gradient(90deg, transparent, #4ade80, transparent);
    opacity: 0.9;
  }
  .island.error .core::after {
    background: radial-gradient(70% 100% at 50% 100%, rgba(248, 113, 113, 0.2), transparent 75%);
  }
  .island.confirming .core::after {
    background: radial-gradient(70% 100% at 50% 100%, rgba(251, 191, 36, 0.2), transparent 75%);
  }

  /* ---------- Top band ---------- */
  .band {
    position: relative;
    z-index: 1;
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    padding: 0 10px 0 40px;
  }
  .face {
    position: absolute;
    top: 50%;
    display: grid;
    place-items: center;
    translate: 0 -50%;
  }
  .center {
    flex: 1;
    min-width: 0;
    display: grid;
    place-items: center;
  }
  .label {
    grid-area: 1 / 1;
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 0;
    max-width: 100%;
  }
  .title {
    font-size: 12.5px;
    font-weight: 600;
    letter-spacing: 0.005em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    color: rgba(255, 255, 255, 0.94);
  }
  .title.hint {
    font-weight: 500;
    color: rgba(255, 255, 255, 0.58);
  }
  .error .title {
    color: #fecaca;
  }
  .confirming .title {
    color: #fde68a;
  }
  .shimmer {
    background: linear-gradient(90deg, rgba(255, 255, 255, 0.38) 0%, #fff 45%, var(--accent-text) 55%, rgba(255, 255, 255, 0.38) 100%);
    background-size: 220% 100%;
    -webkit-background-clip: text;
    background-clip: text;
    -webkit-text-fill-color: transparent;
    animation: shimmer 1.5s linear infinite;
  }
  .side {
    display: grid;
    place-items: center;
    min-width: 30px;
    justify-items: end;
  }
  .idle .side {
    min-width: 0;
  }
  .timer {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-family: var(--font-latin);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    color: rgba(255, 255, 255, 0.72);
  }
  .rec {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #ff453a;
    animation: rec 1.4s ease-out infinite;
  }
  .spinner {
    width: 15px;
    height: 15px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.14);
    border-top-color: var(--accent-text);
    animation: spin 0.8s linear infinite;
  }
  .check {
    width: 20px;
    height: 20px;
    padding: 3px;
    border-radius: 50%;
    background: #22c55e;
    fill: none;
    stroke: #04170a;
    stroke-width: 3.2;
    stroke-linecap: round;
    stroke-linejoin: round;
    box-shadow: 0 0 14px rgba(34, 197, 94, 0.6);
  }
  .shield {
    width: 22px;
    height: 22px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: radial-gradient(circle at 35% 30%, #fde68a, #f59e0b);
    box-shadow: 0 0 14px rgba(251, 191, 36, 0.6);
  }
  .shield svg {
    width: 14px;
    height: 14px;
    fill: none;
    stroke: #451a03;
    stroke-width: 2.2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .pulse-ring {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: #fbbf24;
    animation: ping 1.4s var(--ease-out) infinite;
  }
  .icon-btn {
    width: 22px;
    height: 22px;
    display: grid;
    place-items: center;
    border: none;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.09);
    cursor: pointer;
    transition: background var(--fast), transform var(--medium) var(--ease-spring);
  }
  .icon-btn:hover {
    background: rgba(255, 255, 255, 0.18);
    transform: rotate(90deg) scale(1.08);
  }
  .icon-btn svg {
    width: 10px;
    height: 10px;
    stroke: rgba(255, 255, 255, 0.8);
    stroke-width: 1.6;
    stroke-linecap: round;
    fill: none;
  }

  /* ---------- Body ---------- */
  .body-clip {
    position: relative;
    z-index: 1;
    display: flex;
    justify-content: center;
  }
  .body {
    flex: 0 0 auto;
  }
  .inner {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 4px 12px 12px;
  }
  .bubble {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 0 4px;
    font-size: 13.5px;
    line-height: 1.5;
    color: rgba(255, 255, 255, 0.95);
  }
  .bubble span {
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .bubble svg {
    width: 14px;
    height: 14px;
    flex-shrink: 0;
    margin-top: 3px;
    fill: none;
    stroke: rgba(255, 255, 255, 0.45);
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .bubble.said {
    color: var(--accent-text);
  }
  .steps {
    margin: 0;
    padding: 0 4px;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .steps li {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 18px;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.45);
    transition: color var(--medium);
  }
  .steps li.latest,
  .steps li.running {
    color: rgba(255, 255, 255, 0.92);
  }
  .steps li.failed {
    color: #fca5a5;
  }
  .step-label {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mark {
    width: 14px;
    height: 14px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
  }
  .mark svg {
    width: 13px;
    height: 13px;
    fill: none;
    stroke-width: 2.8;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .done .mark svg {
    stroke: #4ade80;
  }
  .failed .mark svg {
    stroke: var(--danger);
  }
  .ring {
    width: 11px;
    height: 11px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.15);
    border-top-color: var(--accent-text);
    box-shadow: 0 0 8px var(--glow-soft);
    animation: spin 0.7s linear infinite;
  }
  .error-text {
    margin: 0;
    padding: 0 4px 2px;
    font-size: 13px;
    line-height: 1.55;
    color: #fecaca;
    display: -webkit-box;
    -webkit-line-clamp: 4;
    line-clamp: 4;
    -webkit-box-orient: vertical;
    overflow: hidden;
    user-select: text;
  }
  .confirm-detail {
    margin: 0;
    padding: 9px 12px;
    border-radius: 14px;
    background: rgba(251, 191, 36, 0.08);
    box-shadow: inset 0 0 0 1px rgba(251, 191, 36, 0.18);
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.5;
    color: #fde68a;
    direction: ltr;
    text-align: left;
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .card {
    padding: 11px 14px 12px;
    border-radius: 19px;
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.1), rgba(255, 255, 255, 0.045));
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.1), inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }
  .card-head {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: rgba(255, 255, 255, 0.5);
  }
  .card-head svg {
    width: 13px;
    height: 13px;
    fill: var(--accent-text);
    filter: drop-shadow(0 0 5px var(--glow));
  }
  .answer-text {
    margin-top: 6px;
    font-size: 13.5px;
    line-height: 21px;
    color: rgba(255, 255, 255, 0.95);
    white-space: pre-wrap;
    overflow-y: auto;
    user-select: text;
    cursor: text;
  }
  .caret {
    display: inline-block;
    width: 2px;
    height: 1.05em;
    margin-inline-start: 2px;
    vertical-align: -2px;
    border-radius: 1px;
    background: var(--accent-text);
    box-shadow: 0 0 8px var(--glow);
    animation: blink 0.9s steps(2) infinite;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
  }
  .actions.end {
    justify-content: flex-end;
    margin-top: 0;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 13px;
    border: none;
    border-radius: 99px;
    font-size: 12px;
    font-weight: 600;
    color: rgba(255, 255, 255, 0.92);
    background: rgba(255, 255, 255, 0.1);
    cursor: pointer;
    transition: background var(--fast), transform var(--medium) var(--ease-spring), box-shadow var(--fast);
  }
  .chip:hover {
    background: rgba(255, 255, 255, 0.18);
    transform: translateY(-1px);
  }
  .chip:active {
    transform: scale(0.95);
  }
  .chip svg {
    width: 13px;
    height: 13px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2.2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .chip.warn {
    background: linear-gradient(180deg, #fcd34d, #f59e0b);
    color: #1c1003;
    box-shadow: 0 6px 18px -6px rgba(245, 158, 11, 0.8);
  }
  .chip.warn:hover {
    box-shadow: 0 8px 24px -6px rgba(245, 158, 11, 0.95);
  }
  .composer {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 40px;
    padding: 0 6px 0 15px;
    border: none;
    border-radius: 20px;
    font: inherit;
    color: inherit;
    text-align: start;
    background: rgba(255, 255, 255, 0.08);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
    transition: box-shadow var(--medium), background var(--fast);
  }
  .composer:focus-within {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent-text) 55%, transparent), 0 0 22px -6px var(--glow);
  }
  .composer.ghost {
    cursor: text;
  }
  .composer.ghost span:first-child {
    flex: 1;
    font-size: 13.5px;
    color: rgba(255, 255, 255, 0.4);
  }
  .composer.ghost:hover {
    background: rgba(255, 255, 255, 0.11);
  }
  .composer input {
    flex: 1;
    min-width: 0;
    border: none;
    outline: none;
    background: transparent;
    font-size: 14px;
    font-weight: 500;
    color: #fff;
    caret-color: var(--accent-text);
  }
  .composer input::placeholder {
    color: rgba(255, 255, 255, 0.4);
  }
  .send {
    width: 28px;
    height: 28px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    border: none;
    border-radius: 50%;
    background: #fff;
    cursor: pointer;
    transition: opacity var(--fast), transform var(--medium) var(--ease-spring), box-shadow var(--fast);
  }
  .send svg {
    width: 15px;
    height: 15px;
    fill: none;
    stroke: #000;
    stroke-width: 2.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .send:disabled {
    opacity: 0.25;
    cursor: default;
  }
  .send:not(:disabled):hover {
    transform: scale(1.08);
    box-shadow: 0 0 16px var(--glow);
  }
  .send:not(:disabled):active {
    transform: scale(0.92);
  }
  .send.small {
    opacity: 0.3;
  }

  @keyframes shimmer {
    from {
      background-position: 110% 0;
    }
    to {
      background-position: -110% 0;
    }
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @keyframes blink {
    to {
      opacity: 0;
    }
  }
  @keyframes breathe {
    0%,
    100% {
      filter: blur(14px) saturate(1.6) brightness(0.9);
    }
    50% {
      filter: blur(17px) saturate(1.8) brightness(1.25);
    }
  }
  @keyframes drop {
    from {
      translate: 0 -110%;
    }
  }
  @keyframes sweep {
    from {
      background-position: 130% 0;
    }
    to {
      background-position: -30% 0;
    }
  }
  @keyframes rec {
    0% {
      box-shadow: 0 0 0 0 rgba(255, 69, 58, 0.55);
    }
    100% {
      box-shadow: 0 0 0 7px rgba(255, 69, 58, 0);
    }
  }
  @keyframes ping {
    0% {
      box-shadow: 0 0 0 0 rgba(251, 191, 36, 0.6);
    }
    100% {
      box-shadow: 0 0 0 10px rgba(251, 191, 36, 0);
    }
  }
  @keyframes nudge {
    0%,
    100% {
      translate: 0;
    }
    20% {
      translate: -6px;
    }
    40% {
      translate: 5px;
    }
    60% {
      translate: -3px;
    }
    80% {
      translate: 2px;
    }
  }
</style>