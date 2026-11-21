<script lang="ts">
  // The assistant tab: see what the assistant does (live state and steps), read the chat, send requests or Stop.
  import { tick } from 'svelte';
  import { fade, fly, scale, slide } from 'svelte/transition';
  import { backOut, cubicOut } from 'svelte/easing';
  import { BotFace, renderMarkdown } from '@anytask/ui';
  import { invoke } from '../lib/ipc';
  import Icon from './Icon.svelte';
  import { goTab, haptic, ui } from './store.svelte';
  import { faError } from './text';

  let { active }: { active: boolean } = $props();

  const STATE_LABEL: Record<string, string> = {
    idle: 'آماده',
    listening: 'در حال شنیدن',
    thinking: 'در حال فکر کردن',
    speaking: 'در حال صحبت',
    working: 'در حال انجام کار',
    success: 'انجام شد',
    error: 'خطا',
    'text-input': 'آماده',
    answer: 'جواب داد'
  };
  const IDEAS = ['از صفحه کامپیوتر عکس بگیر', 'صدای کامپیوتر را کم کن', 'آخرین فایل دانلودشده را باز کن'];

  let input = $state('');
  let list = $state<HTMLDivElement>();
  let box = $state<HTMLTextAreaElement>();
  const level = $derived(ui.mode === 'thinking' || ui.mode === 'working' ? 0.35 : ui.mode === 'speaking' || ui.mode === 'listening' ? 0.5 : 0.12);

  async function scrollDown(smooth = true) {
    await tick();
    list?.scrollTo({ top: list.scrollHeight, behavior: smooth ? 'smooth' : 'instant' });
  }

  // New messages and steps scroll to the end; coming back to this tab shows the end too.
  $effect(() => {
    void ui.scroll;
    void scrollDown();
  });
  $effect(() => {
    if (active) void scrollDown(false);
  });

  // The field grows with its text (up to a few lines).
  $effect(() => {
    void input;
    if (!box) return;
    box.style.height = 'auto';
    box.style.height = `${Math.min(box.scrollHeight, 132)}px`;
  });

  async function send() {
    const text = input.trim();
    if (!text) return;
    ui.sendError = '';
    haptic();
    try {
      // With "read answers aloud" on, the PC speaks the answer and sends the sound (no VPN needed on the phone).
      await invoke('send', { message: { type: 'prompt', text, voice: ui.speakAnswers } });
      input = '';
    } catch (e) {
      ui.sendError = faError(e);
    }
  }

  function stop() {
    haptic(15);
    void invoke('send', { message: { type: 'stop' } }).catch(() => {});
  }

  // Speak instead of typing: the phone's own speech recognition, Persian on a tap, English on a long press. What
  // was heard is sent at once.
  let listening = $state(false);
  let pressTimer: ReturnType<typeof setTimeout> | undefined;
  let english = false;

  async function speak() {
    if (listening || !ui.link.connected) return;
    listening = true;
    ui.sendError = '';
    haptic(12);
    try {
      const heard = ((await invoke<string>('listen', { lang: english ? 'en-US' : 'fa-IR' })) ?? '').trim();
      if (heard) {
        input = heard;
        await send();
      }
    } catch (e) {
      const text = String(e);
      if (!/Nothing was heard/.test(text)) ui.sendError = faError(e);
    } finally {
      listening = false;
      english = false;
    }
  }

  // Only a press that started on the mic counts (a finger lifted from Stop as the mic replaced it does not).
  let pressed = false;

  function micDown(e: PointerEvent) {
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    pressed = true;
    english = false;
    clearTimeout(pressTimer);
    pressTimer = setTimeout(() => {
      english = true;
      haptic(25);
    }, 600);
  }

  function micUp() {
    clearTimeout(pressTimer);
    if (!pressed) return;
    pressed = false;
    void speak();
  }

  function micCancel() {
    clearTimeout(pressTimer);
    pressed = false;
    english = false;
  }
</script>

<div class="chat-page">
  <header class="status glass">
    <div class="face" class:alive={ui.busy}><BotFace mode={ui.mode} {level} size={36} /></div>
    <div class="lines">
      <b>{ui.assistant}</b>
      {#key ui.mode}<span in:fade={{ duration: 200 }}>{STATE_LABEL[ui.mode] ?? ui.mode}{ui.detail ? ` — ${ui.detail}` : ''}</span>{/key}
    </div>
    <button class="pill" class:on={ui.link.connected} onclick={() => goTab('pc')} aria-label="وضعیت اتصال">
      <i></i>{ui.link.connected ? ui.link.pc || 'وصل' : 'در حال اتصال…'}
    </button>
  </header>

  {#if !ui.link.connected && ui.link.error}
    <p class="banner" transition:slide={{ duration: 200 }}>{faError(ui.link.error)}</p>
  {/if}

  <div class="list" bind:this={list}>
    {#each ui.messages as m (m.key)}
      <div class="bubble {m.role} {m.status}" in:fly={{ y: 10, duration: 260, easing: cubicOut }}>
        {#if m.steps.length}
          <ul class="steps">
            {#each m.steps as s (s.id)}
              <li class={s.status} in:slide={{ duration: 180 }}>
                <span class="mark">
                  {#if s.status === 'done'}<span in:scale={{ start: 0.3, duration: 300, easing: backOut }}>✓</span>{:else if s.status === 'failed'}✕{:else}<span class="spin small"></span>{/if}
                </span>
                {s.label}
              </li>
            {/each}
          </ul>
        {/if}
        {#if m.status === 'pending' && !m.text}
          <span class="typing"><i></i><i></i><i></i></span>
        {:else if m.role === 'assistant'}
          <div class="md" dir="auto">{@html renderMarkdown(m.text || (m.status === 'stopped' ? 'متوقف شد' : ''))}</div>
        {:else}
          <div dir="auto">{m.text}</div>
        {/if}
      </div>
    {:else}
      <div class="empty" in:fade>
        <p>هنوز گفتگویی نیست. از همین‌جا درخواست بفرست.</p>
        <div class="ideas">
          {#each IDEAS as idea (idea)}
            <button onclick={() => ((input = idea), box?.focus())}>{idea}</button>
          {/each}
        </div>
      </div>
    {/each}
  </div>

  <footer class="composer">
    {#if ui.sendError}<p class="error" transition:slide={{ duration: 200 }}>{ui.sendError}</p>{/if}
    <div class="row glass">
      <textarea
        bind:this={box}
        bind:value={input}
        rows="1"
        dir="auto"
        placeholder="به دستیار بگو چه کار کند…"
        aria-label="درخواست"
        onkeydown={(e) => e.key === 'Enter' && !e.shiftKey && (e.preventDefault(), send())}
      ></textarea>
      {#if ui.busy}
        <button class="round stop" onclick={stop} aria-label="توقف" in:scale={{ duration: 200 }}><span></span></button>
      {:else if input.trim()}
        <button class="round" disabled={!ui.link.connected} onclick={send} aria-label="فرستادن" in:scale={{ duration: 200 }}>
          <Icon name="send" size={20} stroke={2.2} />
        </button>
      {:else}
        <button
          class="round mic"
          class:listening
          disabled={!ui.link.connected || listening}
          onpointerdown={micDown}
          onpointerup={micUp}
          onpointercancel={micCancel}
          onlostpointercapture={() => clearTimeout(pressTimer)}
          oncontextmenu={(e) => e.preventDefault()}
          onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), void speak())}
          aria-label="گفتن با صدا (نگه داشتن: انگلیسی)"
          in:scale={{ duration: 200 }}
        >
          <Icon name="mic" size={20} stroke={2.2} />
        </button>
      {/if}
    </div>
  </footer>
</div>

<style>
  .chat-page {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 12px 12px 6px;
    padding: 12px 14px;
    min-height: 70px;
  }
  .face {
    width: 50px;
    height: 50px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: radial-gradient(circle at 50% 30%, rgba(255, 255, 255, 0.14), transparent 70%);
    transition: box-shadow 400ms ease;
  }
  .face.alive {
    box-shadow: 0 0 28px -4px var(--glow);
  }
  .lines {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .lines b {
    font-size: 16px;
  }
  .lines span {
    font-size: 13px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 40%;
    padding: 6px 10px;
    border: none;
    border-radius: 999px;
    background: rgba(0, 0, 0, 0.3);
    color: var(--text-muted);
    font: inherit;
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pill i {
    flex-shrink: 0;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #fbbf24;
    animation: pulse 1.2s ease-in-out infinite;
  }
  .pill.on i {
    background: #34d399;
    animation: none;
    box-shadow: 0 0 8px rgba(52, 211, 153, 0.8);
  }
  .banner {
    margin: 0 12px 6px;
    padding: 8px 12px;
    border-radius: 14px;
    font-size: 12.5px;
    color: #fde68a;
    background: rgba(251, 191, 36, 0.09);
    border: 1px solid rgba(251, 191, 36, 0.12);
  }
  .list {
    flex: 1;
    overflow-y: auto;
    padding: 8px 12px 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .empty {
    margin: auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    text-align: center;
  }
  .empty p {
    margin: 0;
    color: var(--text-dim);
    font-size: 14px;
  }
  .ideas {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .ideas button {
    padding: 10px 16px;
    border-radius: 16px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    background: rgba(255, 255, 255, 0.06);
    color: var(--text-muted);
    font: inherit;
    font-size: 13.5px;
  }
  .bubble {
    max-width: 86%;
    padding: 10px 14px;
    border-radius: 20px;
    font-size: 15px;
    line-height: 1.7;
    word-break: break-word;
    user-select: text;
  }
  .bubble.user {
    align-self: flex-start;
    background: linear-gradient(180deg, color-mix(in srgb, var(--accent) 62%, #000), color-mix(in srgb, var(--accent) 45%, #000));
    border-bottom-right-radius: 6px;
    box-shadow: 0 8px 22px -14px var(--glow);
  }
  .bubble.assistant {
    align-self: flex-end;
    background: rgba(255, 255, 255, 0.07);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-bottom-left-radius: 6px;
  }
  .bubble.error {
    color: #fecaca;
  }
  .bubble.stopped {
    color: var(--text-dim);
  }
  .md :global(p) {
    margin: 0 0 6px;
  }
  .md :global(p:last-child) {
    margin: 0;
  }
  .steps {
    list-style: none;
    margin: 0 0 6px;
    padding: 0;
    font-size: 13px;
    color: var(--text-muted);
  }
  .steps li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 0;
  }
  .steps li.failed {
    color: #fca5a5;
  }
  .mark {
    width: 16px;
    display: inline-grid;
    place-items: center;
    color: #34d399;
  }
  .typing {
    display: inline-flex;
    gap: 4px;
    padding: 4px 0;
  }
  .typing i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-muted);
    animation: bounce 1s ease-in-out infinite;
  }
  .typing i:nth-child(2) {
    animation-delay: 0.15s;
  }
  .typing i:nth-child(3) {
    animation-delay: 0.3s;
  }
  .composer {
    padding: 6px 12px 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .composer .row {
    display: flex;
    gap: 8px;
    align-items: flex-end;
    padding: 6px;
    border-radius: 26px;
  }
  .composer textarea {
    flex: 1;
    min-height: 44px;
    max-height: 132px;
    padding: 11px 12px;
    border: none;
    background: transparent;
  }
  .round {
    width: 44px;
    height: 44px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    border: none;
    border-radius: 50%;
    background: #fff;
    color: #000;
    transition:
      opacity 200ms ease,
      transform 160ms ease;
  }
  .round:disabled {
    opacity: 0.35;
  }
  .round :global(svg) {
    transform: scaleX(-1);
  }
  .round.mic {
    touch-action: manipulation;
    -webkit-touch-callout: none;
    user-select: none;
  }
  .round.mic :global(svg) {
    transform: none;
  }
  .round.mic.listening {
    background: #22d3ee;
    box-shadow: 0 0 22px -2px rgba(34, 211, 238, 0.8);
    animation: hear 1.2s ease-in-out infinite;
  }
  @keyframes hear {
    50% {
      transform: scale(1.08);
    }
  }
  .round.stop {
    background: rgba(225, 29, 72, 0.92);
    box-shadow: 0 0 18px -2px rgba(225, 29, 72, 0.7);
  }
  .round.stop span {
    width: 14px;
    height: 14px;
    border-radius: 3px;
    background: #fff;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  @keyframes bounce {
    0%,
    100% {
      transform: translateY(0);
      opacity: 0.5;
    }
    50% {
      transform: translateY(-4px);
      opacity: 1;
    }
  }
</style>
