<script lang="ts">
  // One message in the chat. Assistant answers type themselves in, live tool steps tick off one by one, and the
  // steps of a finished answer fold into a small chip that opens on click.
  import { onMount, untrack } from 'svelte';
  import { fly, scale, slide } from 'svelte/transition';
  import { backOut, cubicOut } from 'svelte/easing';
  import { L, Reveal, locale, num, plainText, renderMarkdown, type IslandStep } from '@anytask/ui';
  import { invoke } from '../lib/ipc';
  import type { BubbleStatus } from '../lib/types';

  interface Props {
    role: 'user' | 'assistant';
    text: string;
    status?: BubbleStatus;
    steps?: IslandStep[];
    /** Type the text in (a fresh answer) instead of showing it at once (history). */
    animate?: boolean;
    at?: number;
    /** Technical reason shown small under an error. */
    detail?: string;
    onopenmodels?: () => void;
  }

  let { role, text, status = 'done', steps = [], animate = false, at, detail = '', onopenmodels }: Props = $props();

  const reveal = new Reveal();
  let open = $state(false);
  let copied = $state(false);

  onMount(() => {
    if (animate && role === 'assistant' && status === 'done') reveal.start(text);
    else reveal.finish(text);
    return () => reveal.stop();
  });

  // A pending bubble receives its answer later: type it in then.
  let lastText = untrack(() => text);
  $effect(() => {
    if (text !== lastText) {
      lastText = text;
      if (role === 'assistant' && status === 'done') reveal.start(text);
      else reveal.finish(text);
    }
  });

  const time = $derived(at ? new Date(at).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' }) : '');
  const doneCount = $derived(steps.filter((s) => s.status === 'done').length);
  const failedCount = $derived(steps.filter((s) => s.status === 'failed').length);
  const chipText = $derived(
    failedCount === 0
      ? L(`${doneCount} ${doneCount === 1 ? 'step' : 'steps'} done`, `${num(doneCount)} کار انجام شد`)
      : doneCount === 0
        ? L(`${failedCount} ${failedCount === 1 ? 'step' : 'steps'} failed`, `${num(failedCount)} کار انجام نشد`)
        : L(`${doneCount} done · ${failedCount} failed`, `${num(doneCount)} کار انجام شد · ${num(failedCount)} ناموفق`)
  );

  function copy() {
    void invoke('copy_text', { text: plainText(text) });
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

<div class="msg {role} {status}" in:fly={{ y: 14, duration: 380, easing: cubicOut }}>
  {#if role === 'assistant' && steps.length}
    {#if status === 'pending'}
      <ol class="live-steps" transition:slide={{ duration: 240 }}>
        {#each steps as step (step.id)}
          <li class={step.status} in:fly={{ x: 12, duration: 280, easing: cubicOut }} dir="auto">
            <span class="mark">
              {#if step.status === 'running'}
                <span class="ring"></span>
              {:else if step.status === 'done'}
                <svg viewBox="0 0 24 24" in:scale={{ start: 0.3, duration: 280, easing: backOut }}><path d="M5 12.5l4.2 4.2L19 7" /></svg>
              {:else}
                <svg viewBox="0 0 24 24" in:scale={{ start: 0.3, duration: 280, easing: backOut }}><path d="M7 7l10 10M17 7L7 17" /></svg>
              {/if}
            </span>
            {step.label}
          </li>
        {/each}
      </ol>
    {:else}
      <button class="chip" class:partial={failedCount > 0} onclick={() => (open = !open)} in:scale={{ start: 0.8, duration: 260 }}>
        {#if failedCount > 0}
          <svg viewBox="0 0 24 24"><path d="M12 8v5M12 16.5h.01" /></svg>
        {:else}
          <svg viewBox="0 0 24 24"><path d="M5 12.5l4.2 4.2L19 7" /></svg>
        {/if}
        {chipText}
        <svg class="chev" class:open viewBox="0 0 24 24"><path d="M6 9l6 6 6-6" /></svg>
      </button>
      {#if open}
        <ol class="live-steps folded" transition:slide={{ duration: 240 }}>
          {#each steps as step (step.id)}
            <li class={step.status} dir="auto"><span class="dot" class:bad={step.status === 'failed'}></span>{step.label}</li>
          {/each}
        </ol>
      {/if}
    {/if}
  {/if}

  {#if status === 'pending' && !text}
    <div class="bubble typing" aria-label="thinking"><span></span><span></span><span></span></div>
  {:else}
    <div class="bubble" dir="auto" title={time}>
      {#if status === 'error'}
        <svg class="warn" viewBox="0 0 24 24"><path d="M12 9v4M12 17h.01M10.3 4l-8 14A2 2 0 004 21h16a2 2 0 001.7-3l-8-14a2 2 0 00-3.4 0z" /></svg>
      {/if}
      <div class="md">
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- renderMarkdown escapes all HTML first -->
        {@html renderMarkdown(reveal.shown)}{#if !reveal.done}<span class="caret"></span>{/if}
      </div>
      {#if status === 'error' && detail}
        <code class="why" dir="ltr">{detail}</code>
      {/if}
      {#if status === 'error' && onopenmodels}
        <button class="inline-btn" onclick={onopenmodels}>{L('Open model settings', 'رفتن به تنظیمات مدل')}</button>
      {/if}
    </div>
    {#if role === 'assistant' && status === 'done' && reveal.done}
      <div class="tools" in:fly={{ y: -4, duration: 200 }}>
        <button class="tool" onclick={copy} aria-label="copy">
          {#if copied}
            <svg viewBox="0 0 24 24" class="ok"><path d="M5 12.5l4.2 4.2L19 7" /></svg>
          {:else}
            <svg viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2.5" /><path d="M16 8V6a2 2 0 00-2-2H6a2 2 0 00-2 2v8a2 2 0 002 2h2" /></svg>
          {/if}
        </button>
        {#if time}<span class="time">{time}</span>{/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .msg {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: 86%;
  }
  .msg.user {
    align-self: flex-end;
    align-items: flex-end;
  }
  .msg.assistant {
    align-self: flex-start;
    align-items: flex-start;
  }
  /* The user's messages are on the right in both languages. */
  :global([dir='rtl']) .msg.user {
    align-self: flex-start;
    align-items: flex-start;
  }
  :global([dir='rtl']) .msg.assistant {
    align-self: flex-end;
    align-items: flex-end;
  }
  .bubble {
    position: relative;
    padding: 10px 14px;
    border-radius: 18px;
    font-size: 13.5px;
    line-height: 1.75;
    user-select: text;
    word-break: break-word;
  }
  .user .bubble {
    background: var(--shine),
      linear-gradient(135deg, color-mix(in srgb, var(--accent) 38%, rgba(27, 34, 54, 0.75)), color-mix(in srgb, var(--accent-2) 30%, rgba(18, 23, 38, 0.75)));
    border-top-right-radius: 6px;
    box-shadow: 0 10px 26px -14px var(--glow), inset 0 1px 0 rgba(255, 255, 255, 0.22), inset 0 -1px 0 rgba(0, 0, 0, 0.2);
    backdrop-filter: var(--glass-blur);
  }
  .assistant .bubble {
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.07);
    border-top-left-radius: 6px;
    box-shadow: var(--card-shadow);
    backdrop-filter: var(--glass-blur);
  }
  .error .bubble {
    background: rgba(248, 113, 113, 0.09);
    border-color: rgba(248, 113, 113, 0.3);
    color: #fecaca;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .stopped .bubble {
    opacity: 0.55;
    font-style: italic;
  }
  .warn {
    width: 18px;
    height: 18px;
    fill: none;
    stroke: var(--danger);
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .md :global(p) {
    margin: 0 0 6px;
  }
  .md :global(p:last-child) {
    margin-bottom: 0;
  }
  .md :global(ul),
  .md :global(ol) {
    margin: 2px 0 6px;
    padding-inline-start: 20px;
  }
  .md :global(code) {
    font-family: var(--mono);
    font-size: 12px;
    padding: 1px 6px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.35);
    direction: ltr;
    unicode-bidi: isolate;
  }
  .md :global(strong) {
    color: #fff;
  }
  .caret {
    display: inline-block;
    width: 2px;
    height: 1.05em;
    margin-inline-start: 2px;
    vertical-align: -3px;
    border-radius: 1px;
    background: var(--accent-text);
    box-shadow: 0 0 8px var(--glow);
    animation: blink 0.9s steps(2) infinite;
  }
  .typing {
    display: flex;
    gap: 5px;
    padding: 14px 16px;
    background: rgba(255, 255, 255, 0.055);
    border: 1px solid rgba(255, 255, 255, 0.07);
  }
  .typing span {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent-text);
    opacity: 0.4;
    animation: hop 1.1s var(--ease-out) infinite;
  }
  .typing span:nth-child(2) {
    animation-delay: 0.15s;
  }
  .typing span:nth-child(3) {
    animation-delay: 0.3s;
  }
  .live-steps {
    list-style: none;
    margin: 0;
    padding: 8px 12px;
    display: flex;
    flex-direction: column;
    gap: 5px;
    border-radius: 14px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px dashed rgba(255, 255, 255, 0.1);
    font-size: 12px;
    color: var(--text-muted);
  }
  .live-steps.folded {
    border-style: solid;
  }
  .live-steps li {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .live-steps li.failed {
    color: #fca5a5;
  }
  .mark {
    width: 14px;
    height: 14px;
    display: grid;
    place-items: center;
    flex-shrink: 0;
  }
  .mark svg {
    width: 13px;
    height: 13px;
    fill: none;
    stroke-width: 2.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .done .mark svg {
    stroke: var(--success);
  }
  .failed .mark svg {
    stroke: var(--danger);
  }
  .ring {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.15);
    border-top-color: var(--accent-text);
    animation: spin 0.7s linear infinite;
  }
  .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--success);
  }
  .dot.bad {
    background: var(--danger);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border: 1px solid rgba(52, 211, 153, 0.25);
    border-radius: 999px;
    background: rgba(52, 211, 153, 0.08);
    color: #a7f3d0;
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--fast);
  }
  .chip.partial {
    border-color: rgba(251, 191, 36, 0.3);
    background: rgba(251, 191, 36, 0.08);
    color: #fde68a;
  }
  .chip:hover {
    background: rgba(52, 211, 153, 0.14);
  }
  .chip svg {
    width: 12px;
    height: 12px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2.4;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .chev {
    transition: transform var(--medium) var(--ease-spring);
  }
  .chev.open {
    transform: rotate(180deg);
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 8px;
    opacity: 0;
    transition: opacity var(--fast);
  }
  .msg:hover .tools {
    opacity: 1;
  }
  .tool {
    width: 24px;
    height: 24px;
    display: grid;
    place-items: center;
    border: none;
    border-radius: 8px;
    background: transparent;
    cursor: pointer;
  }
  .tool:hover {
    background: rgba(255, 255, 255, 0.08);
  }
  .tool svg {
    width: 14px;
    height: 14px;
    fill: none;
    stroke: var(--text-dim);
    stroke-width: 1.8;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .tool svg.ok {
    stroke: var(--success);
  }
  .time {
    font-size: 10.5px;
    color: var(--text-dim);
  }
  .why {
    font-family: var(--mono);
    font-size: 10.5px;
    line-height: 1.5;
    color: rgba(254, 202, 202, 0.6);
    word-break: break-all;
  }
  .inline-btn {
    align-self: flex-start;
    padding: 5px 12px;
    border: none;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.1);
    color: var(--text);
    font-size: 12px;
    font-weight: 700;
    cursor: pointer;
  }
  .inline-btn:hover {
    background: rgba(255, 255, 255, 0.16);
  }
  @keyframes blink {
    to {
      opacity: 0;
    }
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @keyframes hop {
    0%,
    100% {
      transform: translateY(0);
      opacity: 0.35;
    }
    40% {
      transform: translateY(-5px);
      opacity: 1;
    }
  }
</style>
