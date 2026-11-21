<script lang="ts">
  // The title row of a tab, with the connection pill (tap it to see the computer tab) and room for buttons.
  import type { Snippet } from 'svelte';
  import { goTab, ui } from './store.svelte';

  interface Props {
    title: string;
    sub?: string;
    actions?: Snippet;
  }

  let { title, sub = '', actions }: Props = $props();
</script>

<header class="head">
  <div class="titles">
    <h1>{title}</h1>
    {#if sub}<p>{sub}</p>{/if}
  </div>
  {@render actions?.()}
  <button class="pill" class:on={ui.link.connected} onclick={() => goTab('pc')} aria-label="وضعیت اتصال">
    <i></i>{ui.link.connected ? ui.link.pc || 'وصل' : 'در حال اتصال…'}
  </button>
</header>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 18px 8px;
  }
  .titles {
    flex: 1;
    min-width: 0;
  }
  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 850;
    letter-spacing: -0.2px;
  }
  p {
    margin: 2px 0 0;
    font-size: 12.5px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 1;
    min-width: 0;
    max-width: 38%;
    padding: 6px 11px;
    border-radius: 999px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    background: rgba(255, 255, 255, 0.07);
    backdrop-filter: blur(16px);
    -webkit-backdrop-filter: blur(16px);
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
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
</style>
