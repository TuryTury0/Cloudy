<script lang="ts">
  // Settings > General > wake word: train it (six takes, free speech, a self-test), turn it on or off, remove it.
  // Everything runs on this PC; nothing is sent anywhere.
  import { onMount } from 'svelte';
  import { fade, scale, slide } from 'svelte/transition';
  import { backOut, cubicOut } from 'svelte/easing';
  import { L, num } from '@anytask/ui';
  import { invoke, listen } from '../lib/ipc';
  import type { WakeStatus, WakeTraining } from '../lib/types';
  import Toggle from './Toggle.svelte';

  /** Texts are [English, Persian]; an unknown code is shown as it is. */
  type Texts = Record<string, [string, string]>;
  const text = (texts: Texts, code: string) => (texts[code] ? L(...texts[code]) : code);
  const QUALITY: Texts = { good: ['good', 'خوب'], fair: ['fair', 'متوسط'], weak: ['weak', 'ضعیف'], unverified: ['not checked', 'بررسی نشده'] };
  const REJECTED: Texts = {
    timeout: ['I heard nothing', 'صدایی نشنیدم'],
    too_short: ['That was too short', 'خیلی کوتاه بود'],
    too_long: ['That was too long', 'خیلی طولانی بود'],
    too_quiet: ['That was quiet; say it a little louder', 'صدا آرام بود؛ کمی بلندتر بگو'],
    inconsistent: ['That differed from the earlier takes; say it the same way', 'با دفعه‌های قبل فرق داشت؛ همان‌طور بگو'],
    not_enough_speech: ['I did not hear enough speech; talk some more', 'حرف کافی نشنیدم؛ بیشتر حرف بزن']
  };
  const FAILED: Texts = {
    no_speech: ['No sound came in; check the microphone', 'صدایی نیامد؛ میکروفون را بررسی کن'],
    too_many_failures: ['It failed several times; try again', 'چند بار درست نشد؛ دوباره امتحان کن'],
    mic_unavailable: ['The microphone is not available', 'میکروفون در دسترس نیست']
  };

  let status = $state<WakeStatus | null>(null);
  let label = $state('');
  let training = $state(false);
  let takes = $state(0);
  let total = $state(6);
  let stage = $state<'enroll' | 'negatives' | 'test' | 'test_retry' | ''>('');
  let remaining = $state(0);
  let hint = $state('');
  let result = $state<{ ok: boolean; text: string } | null>(null);
  let confirmRemove = $state(false);
  let error = $state('');

  const message = $derived(
    stage === 'enroll'
      ? L(`After the beep, say the word (${takes + 1} of ${total})`, `بعد از صدای بوق، کلمه را بگو (${num(takes + 1)} از ${num(total)})`)
      : stage === 'negatives'
        ? L(`Now talk freely, anything but the word (${remaining} seconds)`, `حالا آزادانه حرف بزن، هر چیزی غیر از کلمه (${num(remaining)} ثانیه)`)
        : stage === 'test'
          ? L('Say the word once more so we can test it', 'یک بار دیگر کلمه را بگو تا امتحانش کنیم')
          : stage === 'test_retry'
            ? L('Once more, a little clearer', 'یک بار دیگر، کمی واضح‌تر')
            : L('Get ready…', 'آماده شو…')
  );

  async function start() {
    error = '';
    result = null;
    takes = 0;
    stage = '';
    hint = '';
    try {
      await invoke('wake_train', { label: label.trim() });
      training = true;
    } catch (e) {
      error = String(e);
    }
  }

  function onTraining(event: WakeTraining) {
    switch (event.kind) {
      case 'started':
        training = true;
        break;
      case 'prompt':
        stage = event.stage;
        hint = '';
        if (event.stage === 'negatives') remaining = event.seconds;
        break;
      case 'accepted':
        takes = event.index;
        total = event.total;
        hint = '';
        break;
      case 'rejected':
        hint = text(REJECTED, event.reason);
        break;
      case 'tick':
        remaining = event.remaining;
        break;
      case 'finished':
        training = false;
        result = {
          ok: true,
          text:
            L(`Ready ✓ Quality: ${text(QUALITY, event.quality)}`, `آماده است ✓ کیفیت: ${text(QUALITY, event.quality)}`) +
            (event.tested ? '' : L(' (the last test did not pass; train it again if it does not answer)', ' (امتحان آخر نگرفت؛ اگر جواب نداد دوباره آموزش بده)'))
        };
        break;
      case 'failed':
        training = false;
        result = { ok: false, text: text(FAILED, event.reason) };
        break;
      case 'cancelled':
        training = false;
        break;
    }
  }

  async function setEnabled(enabled: boolean) {
    status = (await invoke<WakeStatus>('wake_set_enabled', { enabled })) ?? status;
  }

  async function remove() {
    if (!confirmRemove) {
      confirmRemove = true;
      setTimeout(() => (confirmRemove = false), 3000);
      return;
    }
    confirmRemove = false;
    status = (await invoke<WakeStatus>('wake_remove')) ?? status;
    result = null;
  }

  onMount(() => {
    void invoke<WakeStatus>('wake_status').then((s) => {
      if (!s) return;
      status = s;
      training = s.training;
    });
    const stops: Array<() => void> = [];
    void listen<WakeStatus>('wake-changed', (s) => (status = s)).then((u) => stops.push(u));
    void listen<WakeTraining>('wake-training', onTraining).then((u) => stops.push(u));
    return () => stops.forEach((s) => s());
  });
</script>

<h2 class="sub">{L('Wake word', 'کلمه بیدارباش')}</h2>
<div class="card">
  {#if training}
    <div class="training" transition:slide={{ duration: 280, easing: cubicOut }}>
      <div class="mic" class:hot={stage !== ''}>
        <span class="ring"></span>
        <svg viewBox="0 0 24 24"><path d="M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3zM5 11a7 7 0 0 0 14 0M12 18v3" /></svg>
      </div>
      {#key message}<p class="say" in:fade={{ duration: 200 }}>{message}</p>{/key}
      <div class="dots">
        {#each Array(total) as _, i (i)}
          <span class="dot" class:done={i < takes} class:now={stage === 'enroll' && i === takes}>
            {#if i < takes}<span class="fill" in:scale={{ start: 0.2, duration: 380, easing: backOut }}></span>{/if}
          </span>
        {/each}
        <span class="dot wide" class:done={stage === 'test' || stage === 'test_retry'} class:now={stage === 'negatives'}></span>
        <span class="dot" class:now={stage === 'test' || stage === 'test_retry'}></span>
      </div>
      {#if hint}<p class="hint" transition:slide={{ duration: 180 }}>{hint}</p>{/if}
      <button class="mini" onclick={() => invoke('wake_cancel_training')}>{L('Cancel', 'انصراف')}</button>
    </div>
  {:else if status?.hasModel}
    <div class="row">
      <div>
        <b>{status.label || L('Trained word', 'کلمه آموزش‌دیده')} <span class="badge {status.quality}">{text(QUALITY, status.quality)}</span></b>
        <small>
          {status.enabled
            ? status.listening
              ? L('Listening: say the word to open a conversation', 'در حال گوش دادن — کلمه را بگو تا گفتگو باز شود')
              : L('On (it pauses during a conversation)', 'روشن (در گفتگو مکث می‌کند)')
            : L('Off', 'خاموش')}
        </small>
      </div>
      <Toggle checked={status.enabled} onchange={setEnabled} label="wake word" />
    </div>
    <div class="row actions">
      <input class="field" maxlength="40" placeholder={L('The word, written (optional)', 'متن کلمه (اختیاری)')} bind:value={label} />
      <button class="mini" onclick={start}>{L('Train again', 'آموزش دوباره')}</button>
      <button class="mini danger" class:sure={confirmRemove} onclick={remove}>{confirmRemove ? L('Sure?', 'مطمئنی؟') : L('Remove', 'حذف')}</button>
    </div>
  {:else}
    <div class="row">
      <div>
        <b>{L('Hands-free, with one word', 'بدون دست، با یک کلمه')}</b>
        <small>{L('Say a word or a short phrase six times; training and recognition stay on this computer.', 'یک کلمه یا عبارت کوتاه را شش بار بگو؛ آموزش و تشخیص فقط روی همین کامپیوتر است.')}</small>
      </div>
    </div>
    <div class="row actions">
      <input class="field" maxlength="40" placeholder={L('The word, written (optional), e.g. "Hey Nova"', 'متن کلمه (اختیاری)، مثلاً «هی نوا»')} bind:value={label} />
      <button class="mini primary" onclick={start}>{L('Train a wake word', 'آموزش کلمه بیدارباش')}</button>
    </div>
  {/if}
  {#if result}
    <div class="result" class:ok={result.ok} transition:slide={{ duration: 200 }}>{result.text}</div>
  {/if}
  {#if error}
    <div class="result" transition:slide={{ duration: 200 }} dir="auto">✕ {error}</div>
  {/if}
</div>

<style>
  .sub {
    margin: 4px 4px 10px;
    font-size: 15px;
    font-weight: 700;
    color: var(--text-muted);
  }
  .card {
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
    padding: 6px 18px;
    margin-bottom: 16px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 18px;
    padding: 14px 0;
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  }
  .row:last-of-type {
    border-bottom: none;
  }
  .row b {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13.5px;
  }
  .row small {
    display: block;
    margin-top: 3px;
    color: var(--text-dim);
    font-size: 12px;
  }
  .actions {
    justify-content: flex-start;
    gap: 8px;
  }
  .field {
    flex: 1;
    padding: 8px 12px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: rgba(0, 0, 0, 0.3);
    color: var(--text);
    font-size: 13px;
    outline: none;
  }
  .field:focus {
    border-color: var(--accent-text);
  }
  .badge {
    font-size: 10.5px;
    font-weight: 600;
    padding: 1px 8px;
    border-radius: 999px;
    color: var(--text-dim);
    background: rgba(255, 255, 255, 0.06);
  }
  .badge.good {
    color: #a7f3d0;
    background: rgba(52, 211, 153, 0.1);
  }
  .badge.fair {
    color: #fde68a;
    background: rgba(251, 191, 36, 0.1);
  }
  .badge.weak {
    color: #fecaca;
    background: rgba(248, 113, 113, 0.1);
  }
  .mini {
    padding: 6px 12px;
    border: none;
    border-radius: 9px;
    background: rgba(255, 255, 255, 0.07);
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition: background var(--fast), color var(--fast);
  }
  .mini:hover {
    background: rgba(255, 255, 255, 0.13);
    color: var(--text);
  }
  .mini.primary {
    background: #fff;
    color: #000;
  }
  .mini.danger:hover,
  .mini.sure {
    background: rgba(225, 29, 72, 0.85);
    color: #fff;
  }
  .training {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 18px 0 14px;
    text-align: center;
  }
  .mic {
    position: relative;
    width: 64px;
    height: 64px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: radial-gradient(circle at 35% 30%, var(--accent-2), var(--accent));
    box-shadow: 0 0 26px var(--glow);
  }
  .mic svg {
    width: 26px;
    height: 26px;
    fill: none;
    stroke: #fff;
    stroke-width: 1.8;
    stroke-linecap: round;
  }
  .ring {
    position: absolute;
    inset: -6px;
    border-radius: 50%;
    border: 2px solid var(--accent-text);
    opacity: 0;
  }
  .mic.hot .ring {
    animation: ping 1.6s ease-out infinite;
  }
  .say {
    margin: 0;
    font-size: 14px;
    font-weight: 700;
  }
  .hint {
    margin: 0;
    font-size: 12.5px;
    color: #fde68a;
  }
  .dots {
    display: flex;
    gap: 7px;
    align-items: center;
  }
  .dot {
    position: relative;
    width: 12px;
    height: 12px;
    border-radius: 999px;
    border: 2px solid var(--border);
    transition: border-color var(--medium), width var(--medium) var(--ease-out);
  }
  .dot.wide {
    width: 30px;
  }
  .dot.now {
    border-color: var(--accent-text);
    box-shadow: 0 0 10px var(--glow-soft);
  }
  .dot.done {
    border-color: var(--accent-text);
    background: var(--accent-text);
  }
  .fill {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    background: var(--accent-text);
  }
  .result {
    margin: 0 0 12px;
    padding: 8px 12px;
    border-radius: 10px;
    font-size: 12.5px;
    color: #fecaca;
    background: rgba(248, 113, 113, 0.08);
  }
  .result.ok {
    color: #a7f3d0;
    background: rgba(52, 211, 153, 0.08);
  }
  @keyframes ping {
    0% {
      transform: scale(0.9);
      opacity: 0.8;
    }
    100% {
      transform: scale(1.5);
      opacity: 0;
    }
  }
</style>
