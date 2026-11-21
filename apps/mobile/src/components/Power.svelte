<script lang="ts">
  // The computer tab: how the link is, turning the PC on (Wake-on-LAN), and lock / sleep / restart / shut down.
  // Restart and shutdown wait 20 s on the PC, so a big Cancel shows while they count down.
  import { onDestroy } from 'svelte';
  import { fly, slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { invoke } from '../lib/ipc';
  import type { PowerAction } from '../lib/types';
  import HoldButton from './HoldButton.svelte';
  import Icon from './Icon.svelte';
  import PageHead from './PageHead.svelte';
  import { haptic, toast, ui } from './store.svelte';
  import { fa, faError } from './text';

  /** How long the PC waits before a shutdown or restart (POWER_DELAY on the PC). */
  const DELAY = 20;
  const DONE: Record<PowerAction, string> = {
    lock: 'کامپیوتر قفل شد.',
    sleep: 'کامپیوتر به خواب رفت.',
    restart: `کامپیوتر تا ${fa(DELAY)} ثانیه دیگر ری‌استارت می‌شود.`,
    shutdown: `کامپیوتر تا ${fa(DELAY)} ثانیه دیگر خاموش می‌شود.`,
    cancel: 'خاموشی لغو شد.'
  };

  let working = $state<PowerAction | null>(null);
  let pending = $state<{ action: 'restart' | 'shutdown'; until: number } | null>(null);
  let now = $state(Date.now());
  let waking = $state(false);
  let wakeNote = $state('');
  let clock: ReturnType<typeof setInterval> | undefined;

  const left = $derived(pending ? Math.max(0, Math.ceil((pending.until - now) / 1000)) : 0);

  function startClock() {
    clearInterval(clock);
    clock = setInterval(() => {
      now = Date.now();
      if (pending && now >= pending.until + 1500) {
        pending = null;
        clearInterval(clock);
      }
    }, 250);
  }

  onDestroy(() => clearInterval(clock));

  async function power(action: PowerAction) {
    if (working) return;
    working = action;
    try {
      await invoke('power', { action });
      toast(DONE[action], action === 'cancel' ? 'ok' : 'info');
      if (action === 'restart' || action === 'shutdown') {
        pending = { action, until: Date.now() + DELAY * 1000 };
        now = Date.now();
        startClock();
      } else if (action === 'cancel') {
        pending = null;
        clearInterval(clock);
      }
    } catch (e) {
      toast(faError(e), 'error');
    } finally {
      working = null;
    }
  }

  async function wake() {
    if (waking) return;
    haptic();
    waking = true;
    wakeNote = '';
    try {
      const result = await invoke<{ sent: number }>('wake');
      wakeNote = result && !result.sent
        ? 'فرمان روشن شدن فرستاده نشد؛ گوشی به شبکه وصل است؟'
        : 'فرمان روشن شدن فرستاده شد. اگر تا یک دقیقه روشن نشد: گوشی باید روی همان وای‌فای خانه باشد و Wake-on-LAN در کامپیوتر روشن باشد.';
    } catch (e) {
      wakeNote = faError(e);
    } finally {
      waking = false;
    }
  }
</script>

<div class="page-scroll">
  <PageHead title="کامپیوتر" sub="روشن، خاموش و وضعیت اتصال" />

  <div class="body">
    <section class="card glass hero" class:on={ui.link.connected}>
      <div class="screen-icon"><Icon name="pc" size={30} /></div>
      <div class="who">
        <b dir="auto">{ui.link.pc || 'کامپیوتر'}</b>
        <span class="state"><i></i>{ui.link.connected ? 'وصل است' : 'در حال اتصال…'}</span>
      </div>
    </section>

    {#if !ui.link.connected}
      <section class="card glass wake" transition:slide={{ duration: 240 }}>
        {#if ui.link.error}<p class="why">{faError(ui.link.error)}</p>{/if}
        <button class="btn primary" onclick={wake} disabled={waking || !ui.link.canWake}>
          {#if waking}<span class="spin dark"></span>{:else}<Icon name="wake" size={19} />{/if}روشن کردن
        </button>
        {#if !ui.link.canWake}
          <p class="note">یک بار به کامپیوتر وصل شو تا گوشی یاد بگیرد چطور روشنش کند.</p>
        {/if}
        {#if wakeNote}<p class="note" transition:slide={{ duration: 200 }}>{wakeNote}</p>{/if}
      </section>
    {/if}

    {#if pending}
      <section class="card countdown" transition:fly={{ y: 16, duration: 300, easing: cubicOut }}>
        <div class="count">
          <b>{fa(left)}</b>
          <span>{pending.action === 'shutdown' ? 'تا خاموش شدن' : 'تا ری‌استارت'}</span>
        </div>
        <button class="cancel" onclick={() => (haptic(20), power('cancel'))} disabled={working === 'cancel'}>
          {#if working === 'cancel'}<span class="spin"></span>{:else}<Icon name="close" size={20} stroke={2.4} />{/if}لغو
        </button>
      </section>
    {/if}

    <div class="grid">
      <button class="tap glass" onclick={() => (haptic(), power('lock'))} disabled={!ui.link.connected || working !== null}>
        <span class="ring">{#if working === 'lock'}<span class="spin"></span>{:else}<Icon name="lock" size={24} />{/if}</span>
        <b>قفل</b>
        <small>یک ضربه</small>
      </button>
      <HoldButton icon="sleep" label="خواب" disabled={!ui.link.connected || working !== null} onconfirm={() => power('sleep')} />
      <HoldButton icon="restart" label="ری‌استارت" tone="warn" disabled={!ui.link.connected || working !== null} onconfirm={() => power('restart')} />
      <HoldButton icon="power" label="خاموش" tone="danger" disabled={!ui.link.connected || working !== null} onconfirm={() => power('shutdown')} />
    </div>

    {#if !pending}
      <button class="btn ghost" onclick={() => (haptic(), power('cancel'))} disabled={!ui.link.connected || working !== null}>
        <Icon name="close" size={18} />لغو خاموشی
      </button>
    {/if}

    <section class="card glass info">
      <h2><Icon name="info" size={17} />اتصال</h2>
      <dl>
        <dt>وضعیت</dt>
        <dd>{ui.link.connected ? 'وصل' : 'قطع'}</dd>
        <dt>روشن کردن از گوشی</dt>
        <dd>{ui.link.canWake ? 'آماده' : 'هنوز نه'}</dd>
        {#if ui.addresses.length}
          <dt>آدرس‌ها</dt>
          <dd class="addresses" dir="ltr">
            {#each ui.addresses as a (a)}<span>{a}</span>{/each}
          </dd>
        {/if}
      </dl>
    </section>
  </div>
</div>

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 4px 12px 16px;
  }
  .card {
    padding: 14px;
  }
  .hero {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .screen-icon {
    width: 58px;
    height: 58px;
    display: grid;
    place-items: center;
    border-radius: 18px;
    background: rgba(251, 191, 36, 0.12);
    color: #fde68a;
    transition:
      background 300ms ease,
      color 300ms ease,
      box-shadow 300ms ease;
  }
  .hero.on .screen-icon {
    background: rgba(52, 211, 153, 0.14);
    color: #6ee7b7;
    box-shadow: 0 0 26px -6px rgba(52, 211, 153, 0.6);
  }
  .who {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .who b {
    font-size: 18px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .state {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--text-muted);
  }
  .state i {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #fbbf24;
    animation: pulse 1.2s ease-in-out infinite;
  }
  .hero.on .state i {
    background: #34d399;
    animation: none;
    box-shadow: 0 0 8px rgba(52, 211, 153, 0.8);
  }
  .wake {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .why {
    margin: 0;
    font-size: 13px;
    color: #fde68a;
  }
  .note {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.8;
    color: var(--text-dim);
  }
  .countdown {
    display: flex;
    align-items: center;
    gap: 14px;
    border-radius: 24px;
    background: linear-gradient(135deg, rgba(225, 29, 72, 0.28), rgba(225, 29, 72, 0.1));
    border: 1px solid rgba(251, 113, 133, 0.3);
    box-shadow: 0 0 40px -14px rgba(225, 29, 72, 0.8);
  }
  .count {
    flex: 1;
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .count b {
    font-size: 40px;
    font-weight: 900;
    line-height: 1;
    font-variant-numeric: tabular-nums;
  }
  .count span {
    font-size: 14px;
    color: #fecdd3;
  }
  .cancel {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 14px 24px;
    border: none;
    border-radius: 18px;
    background: #fff;
    color: #be123c;
    font: inherit;
    font-size: 17px;
    font-weight: 900;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .tap {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 16px 8px 12px;
    border-radius: 22px;
    color: var(--text);
    font: inherit;
  }
  .tap:disabled {
    opacity: 0.45;
  }
  .ring {
    width: 52px;
    height: 52px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    box-shadow: inset 0 0 0 3px rgba(255, 255, 255, 0.1);
  }
  .tap b {
    font-size: 15px;
    font-weight: 750;
  }
  .tap small {
    font-size: 11px;
    color: var(--text-dim);
  }
  .info h2 {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 8px;
    font-size: 14px;
    font-weight: 750;
    color: var(--text-muted);
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px 14px;
    margin: 0;
    font-size: 13px;
  }
  dt {
    color: var(--text-dim);
  }
  dd {
    margin: 0;
    text-align: end;
  }
  .addresses {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-family: var(--mono);
    font-size: 12px;
    text-align: start;
    color: var(--text-muted);
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
</style>
