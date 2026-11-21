<script lang="ts">
  // The phone app: pair with the PC once, then five tabs: the assistant (live state, steps, chat, Stop), remote
  // control (touchpad, keyboard, screen view), the PC's files, the PC's power, and settings. Everything goes through
  // the encrypted link in src-tauri; the shared state and the event handling live in components/store.svelte.ts.
  import { onMount } from 'svelte';
  import { currentVersion, findUpdate } from './lib/update';
  import { fly, slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { Aurora, applyTheme, normalizeTheme } from '@anytask/ui';
  import { invoke, listen } from './lib/ipc';
  import type { LinkStatus, RemoteMessage, TransferEvent } from './lib/types';
  import Chat from './components/Chat.svelte';
  import Files from './components/Files.svelte';
  import Pairing from './components/Pairing.svelte';
  import Power from './components/Power.svelte';
  import Settings from './components/Settings.svelte';
  import TabBar from './components/TabBar.svelte';
  import Touchpad from './components/Touchpad.svelte';
  import { loadAccess, onHistory, onRemote, onTransfer, toast, ui, type Tab } from './components/store.svelte';

  const COLORS = ['#38bdf8', '#ffffff', '#bae6fd'];
  const ORDER: Tab[] = ['chat', 'control', 'files', 'pc', 'settings'];
  const USED: Record<string, string> = {
    status: 'وضعیت گوشی',
    notifications: 'اعلان‌ها',
    messages: 'پیامک‌ها',
    apps: 'فهرست برنامه‌ها',
    openApp: 'باز کردن برنامه',
    openUrl: 'باز کردن لینک',
    ring: 'زنگ زدن گوشی',
    stopRing: 'قطع زنگ',
    setAlarm: 'تنظیم زنگ هشدار',
    setTimer: 'شروع تایمر',
    dial: 'شماره‌گیر',
    composeSms: 'نوشتن پیامک',
    navigate: 'نقشه'
  };

  const level = $derived(ui.mode === 'thinking' || ui.mode === 'working' ? 0.35 : ui.mode === 'speaking' || ui.mode === 'listening' ? 0.5 : 0.12);

  /** Tabs are drawn the first time they are opened and then kept (their state, scroll and polling rules stay). */
  let seen = $state<Record<Tab, boolean>>({ chat: true, control: false, files: false, pc: false, settings: false });
  let previous: Tab = 'chat';
  /** Which side the new tab slides in from (RTL: a tab further along comes from the left). */
  let from = $state(0);

  $effect.pre(() => {
    const tab = ui.tab;
    seen[tab] = true;
    from = ORDER.indexOf(tab) > ORDER.indexOf(previous) ? -1 : 1;
    previous = tab;
  });

  onMount(() => {
    applyTheme(normalizeTheme({ name: 'Cloudy', colors: COLORS, weights: [45, 35, 20] }));
    void invoke<LinkStatus>('status').then(
      (s) => {
        if (s) ui.link = s;
        ui.loaded = true;
      },
      () => (ui.loaded = true)
    );
    void invoke<string | null>('last_crash')
      .then((text) => text && (ui.crash = text))
      .catch(() => {});
    const stops: Array<() => void> = [];
    void listen<LinkStatus>('link', (s) => (ui.link = s)).then((u) => stops.push(u));
    void listen<RemoteMessage>('remote', onRemote).then((u) => stops.push(u));
    void listen<TransferEvent>('transfer', onTransfer).then((u) => stops.push(u));
    void listen<string>('crash', (text) => (ui.crash = text)).then((u) => stops.push(u));
    void listen<string>('phone-used', (name) => toast(`کامپیوتر از گوشی استفاده کرد: ${USED[name] ?? name}`)).then((u) => stops.push(u));
    void loadAccess();
    void currentVersion().then((v) => (ui.version = v));
    void findUpdate().then((u) => {
      if (!u) return;
      ui.update = u;
      toast(`نسخه جدید برنامه گوشی (${u.version}) آمده؛ از تنظیمات نصبش کن`, 'info', 6000);
    });

    // Back from Android's settings screen: show the new access state.
    const onVisible = () => document.visibilityState === 'visible' && void loadAccess();
    document.addEventListener('visibilitychange', onVisible);
    stops.push(() => document.removeEventListener('visibilitychange', onVisible));

    // Android's back button walks the tab history (back to the chat, then out of the app).
    const onPop = (e: PopStateEvent) => onHistory(e.state);
    window.addEventListener('popstate', onPop);
    stops.push(() => window.removeEventListener('popstate', onPop));

    // A text field with the keyboard up: the tab bar steps aside to give the field room.
    let blurTimer: ReturnType<typeof setTimeout> | undefined;
    const isField = (t: EventTarget | null) => t instanceof HTMLTextAreaElement || (t instanceof HTMLInputElement && t.type !== 'checkbox');
    const onFocusIn = (e: FocusEvent) => {
      if (!isField(e.target)) return;
      clearTimeout(blurTimer);
      ui.typing = true;
    };
    const onFocusOut = (e: FocusEvent) => {
      if (!isField(e.target)) return;
      clearTimeout(blurTimer);
      blurTimer = setTimeout(() => (ui.typing = isField(document.activeElement)), 120);
    };
    // Android's back button can close the keyboard and leave the field focused: when the keyboard goes away, the
    // field lets go too, so the tab bar comes back.
    const view = window.visualViewport;
    const height = () => view?.height ?? window.innerHeight;
    let tall = height();
    let keyboard = false;
    const onResize = () => {
      const h = height();
      tall = Math.max(tall, h);
      const up = h < tall * 0.8;
      if (keyboard && !up && ui.typing) {
        (document.activeElement as HTMLElement | null)?.blur();
        ui.typing = false;
      }
      keyboard = up;
    };
    const onTurn = () => setTimeout(() => ((tall = height()), (keyboard = false)), 400);
    document.addEventListener('focusin', onFocusIn);
    document.addEventListener('focusout', onFocusOut);
    (view ?? window).addEventListener('resize', onResize);
    window.addEventListener('orientationchange', onTurn);
    stops.push(() => {
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
      (view ?? window).removeEventListener('resize', onResize);
      window.removeEventListener('orientationchange', onTurn);
    });

    return () => stops.forEach((s) => s());
  });
</script>

<div class="backdrop" aria-hidden="true">
  <Aurora colors={COLORS} level={ui.link.paired ? level : 0.2} intensity={0.9} speed={ui.mode === 'thinking' || ui.mode === 'working' ? 2.2 : 1} />
  <div class="veil"></div>
</div>

<main dir="rtl">
  {#if !ui.loaded}
    <div class="center"><span class="spin"></span></div>
  {:else if !ui.link.paired}
    <Pairing />
  {:else}
    <div class="pages" style="--from: {from}">
      {#if seen.chat}
        <section class="page" class:active={ui.tab === 'chat'} hidden={ui.tab !== 'chat'}><Chat active={ui.tab === 'chat'} /></section>
      {/if}
      {#if seen.control}
        <section class="page" class:active={ui.tab === 'control'} hidden={ui.tab !== 'control'}><Touchpad active={ui.tab === 'control'} /></section>
      {/if}
      {#if seen.files}
        <section class="page" class:active={ui.tab === 'files'} hidden={ui.tab !== 'files'}><Files active={ui.tab === 'files'} /></section>
      {/if}
      {#if seen.pc}
        <section class="page" class:active={ui.tab === 'pc'} hidden={ui.tab !== 'pc'}><Power /></section>
      {/if}
      {#if seen.settings}
        <section class="page" class:active={ui.tab === 'settings'} hidden={ui.tab !== 'settings'}><Settings /></section>
      {/if}
    </div>
    {#if !ui.typing}
      <div transition:slide={{ duration: 220, easing: cubicOut }}><TabBar /></div>
    {/if}
  {/if}

  <div class="toasts" aria-live="polite">
    {#each ui.toasts as t (t.id)}
      <div class="toast {t.kind}" in:fly={{ y: -20, duration: 300, easing: cubicOut }} out:fly={{ y: -12, duration: 200 }}>{t.text}</div>
    {/each}
  </div>
</main>

<style>
  :global(body) {
    margin: 0;
    background: #07070c;
    color: var(--text);
    font-family: var(--font);
    overscroll-behavior: none;
    -webkit-tap-highlight-color: transparent;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 0;
    pointer-events: none;
  }
  .veil {
    position: absolute;
    inset: 0;
    background:
      radial-gradient(120% 70% at 50% 0%, transparent, rgba(7, 7, 12, 0.4) 75%),
      linear-gradient(180deg, rgba(7, 7, 12, 0.1), rgba(7, 7, 12, 0.55) 50%, rgba(7, 7, 12, 0.78));
  }
  main {
    position: relative;
    z-index: 1;
    display: flex;
    flex-direction: column;
    height: 100dvh;
    padding: env(safe-area-inset-top) 0 0;
    box-sizing: border-box;
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
  }
  .pages {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .page {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .page[hidden] {
    display: none;
  }
  .page.active {
    animation: page-in 300ms var(--ease-out);
  }
  .toasts {
    position: fixed;
    top: calc(10px + env(safe-area-inset-top));
    inset-inline: 12px;
    z-index: 60;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    pointer-events: none;
  }
  .toast {
    max-width: 100%;
    padding: 10px 16px;
    border-radius: 18px;
    background: rgba(28, 31, 46, 0.78);
    backdrop-filter: blur(22px) saturate(170%);
    -webkit-backdrop-filter: blur(22px) saturate(170%);
    border: 1px solid rgba(255, 255, 255, 0.12);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.16),
      0 14px 34px -14px rgba(0, 0, 0, 0.8);
    font-size: 13.5px;
    line-height: 1.7;
    text-align: center;
  }
  .toast.ok {
    border-color: rgba(52, 211, 153, 0.35);
  }
  .toast.error {
    border-color: rgba(248, 113, 113, 0.4);
    color: #fecaca;
  }
  @keyframes page-in {
    from {
      opacity: 0;
      transform: translateX(calc(var(--from) * 18px));
    }
  }

  /* ---------- shared pieces (used by every tab) ---------- */

  /* Liquid glass: a translucent panel that blurs the aurora behind it, with a soft rim. */
  :global(.glass) {
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.085), rgba(255, 255, 255, 0.03));
    backdrop-filter: blur(22px) saturate(160%);
    -webkit-backdrop-filter: blur(22px) saturate(160%);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 22px;
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.14),
      0 12px 32px -18px rgba(0, 0, 0, 0.75);
  }
  :global(.page-scroll) {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }
  :global(button) {
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
    transition: transform 140ms ease;
  }
  :global(button:active:not(:disabled)) {
    transform: scale(0.97);
  }
  :global(textarea),
  :global(input) {
    box-sizing: border-box;
    color: var(--text);
    font: inherit;
    outline: none;
  }
  :global(textarea) {
    width: 100%;
    padding: 12px 14px;
    border-radius: 16px;
    border: 1px solid var(--border);
    background: rgba(255, 255, 255, 0.05);
    font-size: 15px;
    resize: none;
  }
  :global(textarea:focus) {
    border-color: var(--accent-text);
  }
  :global(.btn) {
    display: inline-flex;
    justify-content: center;
    align-items: center;
    gap: 8px;
    padding: 14px;
    border: 1px solid transparent;
    border-radius: 18px;
    font: inherit;
    font-size: 15.5px;
    font-weight: 800;
  }
  :global(.btn.primary) {
    background: #fff;
    color: #000;
    box-shadow: 0 10px 30px -14px var(--glow);
  }
  :global(.btn.ghost) {
    background: rgba(255, 255, 255, 0.08);
    border-color: rgba(255, 255, 255, 0.1);
    color: var(--text);
  }
  :global(.btn.danger) {
    background: rgba(244, 63, 94, 0.12);
    border-color: rgba(244, 63, 94, 0.25);
    color: #fda4af;
  }
  :global(.btn.danger.solid) {
    background: #e11d48;
    border-color: transparent;
    color: #fff;
  }
  :global(.btn:disabled) {
    opacity: 0.45;
  }
  :global(.icon-btn) {
    width: 40px;
    height: 40px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    border-radius: 14px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    background: rgba(255, 255, 255, 0.07);
    color: var(--text);
  }
  :global(.icon-btn:disabled) {
    opacity: 0.4;
  }
  :global(.error) {
    margin: 0;
    padding: 8px 12px;
    border-radius: 12px;
    font-size: 13px;
    color: #fecaca;
    background: rgba(248, 113, 113, 0.1);
  }
  :global(.spin) {
    display: inline-block;
    width: 16px;
    height: 16px;
    flex-shrink: 0;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.25);
    border-top-color: #fff;
    animation: spin 0.7s linear infinite;
  }
  :global(.spin.dark) {
    border-color: rgba(0, 0, 0, 0.2);
    border-top-color: #000;
  }
  :global(.spin.small) {
    width: 10px;
    height: 10px;
    border-width: 1.5px;
  }
  @keyframes -global-spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>