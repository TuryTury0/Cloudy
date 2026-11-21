<script lang="ts">
  // The glass tab bar at the bottom: five tabs, a light that slides to the open one, and small dots for news
  // (the assistant is working, files are moving, something in Settings needs a look).
  import Icon, { type IconName } from './Icon.svelte';
  import { goTab, moving, ui, type Tab } from './store.svelte';

  const TABS: Array<{ id: Tab; label: string; icon: IconName }> = [
    { id: 'chat', label: 'دستیار', icon: 'chat' },
    { id: 'control', label: 'کنترل', icon: 'control' },
    { id: 'files', label: 'فایل‌ها', icon: 'folder' },
    { id: 'pc', label: 'کامپیوتر', icon: 'pc' },
    { id: 'settings', label: 'تنظیمات', icon: 'settings' }
  ];

  const index = $derived(TABS.findIndex((t) => t.id === ui.tab));
  const dots = $derived<Record<Tab, boolean>>({
    chat: ui.busy && ui.tab !== 'chat',
    control: false,
    files: moving() > 0,
    pc: !ui.link.connected,
    settings: Boolean(ui.crash) || (ui.access !== null && (!ui.access.notificationAccess || !ui.access.smsAccess))
  });
</script>

<nav class="bar" aria-label="بخش‌ها">
  <span class="light" style="--i: {index}"></span>
  {#each TABS as t (t.id)}
    <button class:on={t.id === ui.tab} aria-current={t.id === ui.tab ? 'page' : undefined} onclick={() => goTab(t.id)}>
      <span class="icon">
        <Icon name={t.icon} size={23} stroke={t.id === ui.tab ? 2.2 : 1.8} />
        {#if dots[t.id]}<i class="dot" class:warn={t.id === 'pc'}></i>{/if}
      </span>
      <span class="label">{t.label}</span>
    </button>
  {/each}
</nav>

<style>
  .bar {
    position: relative;
    display: flex;
    margin: 0 12px calc(10px + env(safe-area-inset-bottom));
    padding: 6px;
    border-radius: 26px;
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.1), rgba(255, 255, 255, 0.04));
    backdrop-filter: blur(24px) saturate(170%);
    -webkit-backdrop-filter: blur(24px) saturate(170%);
    border: 1px solid rgba(255, 255, 255, 0.12);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.18),
      0 18px 40px -16px rgba(0, 0, 0, 0.8);
    z-index: 20;
  }
  .light {
    position: absolute;
    top: 6px;
    bottom: 6px;
    inset-inline-start: calc(6px + var(--i) * (100% - 12px) / 5);
    width: calc((100% - 12px) / 5);
    border-radius: 20px;
    background: radial-gradient(120% 90% at 50% 0%, color-mix(in srgb, var(--accent) 45%, transparent), transparent 75%),
      rgba(255, 255, 255, 0.1);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.22);
    transition: inset-inline-start 420ms var(--ease-spring);
  }
  button {
    position: relative;
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 3px;
    padding: 8px 0 6px;
    border: none;
    background: none;
    color: var(--text-dim);
    font: inherit;
    transition: color 200ms ease;
  }
  button.on {
    color: var(--text);
  }
  .icon {
    position: relative;
    transition: transform 320ms var(--ease-spring);
  }
  button.on .icon {
    transform: translateY(-1px) scale(1.08);
  }
  .label {
    font-size: 11px;
    font-weight: 600;
  }
  .dot {
    position: absolute;
    top: -2px;
    inset-inline-end: -4px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 8px var(--accent);
  }
  .dot.warn {
    background: #fbbf24;
    box-shadow: 0 0 8px rgba(251, 191, 36, 0.7);
  }
</style>
