<script lang="ts">
  // Settings > About: bring over what AnyTask v1 left on this PC (shown first; nothing happens until "Import").
  import { onMount } from 'svelte';
  import { fade, slide } from 'svelte/transition';
  import { L, day, num } from '@anytask/ui';
  import { invoke } from '../lib/ipc';
  import type { V1Report } from '../lib/types';

  let report = $state<V1Report | null>(null);
  let busy = $state(false);
  let done = $state<string[] | null>(null);
  let error = $state('');

  const s = $derived(report?.summary);

  async function run() {
    busy = true;
    error = '';
    try {
      done = (await invoke<string[]>('v1_import')) ?? [];
      report = (await invoke<V1Report>('v1_report')) ?? report;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    void invoke<V1Report>('v1_report').then((r) => r && (report = r));
  });
</script>

{#if s?.found}
  <h2 class="sub" in:fade>{L('Move from the previous version', 'انتقال از نسخه قبل')}</h2>
  <div class="card" in:fade>
    <p class="lead">{L('AnyTask version 1 was found on this computer. These are moved over (keys go straight to Windows Credential Manager):', 'AnyTask نسخه ۱ روی این کامپیوتر پیدا شد. این‌ها منتقل می‌شوند (کلیدها مستقیم به Windows Credential Manager می‌روند):')}</p>
    <ul>
      {#if s.models.length}<li><b>{L(`${s.models.length} models`, `${num(s.models.length)} مدل`)}</b> <small dir="ltr">{s.models.join(' · ')}</small></li>{/if}
      {#if s.keys}<li><b>{L(`${s.keys} keys`, `${num(s.keys)} کلید`)}</b></li>{/if}
      {#if s.turns || s.facts}<li>{L(`${s.turns} conversations and ${s.facts} facts from memory`, `${num(s.turns)} گفتگو و ${num(s.facts)} نکته از حافظه`)}</li>{/if}
      {#if s.wakeWord}<li>{L('Wake word', 'کلمه بیدارباش')} <b dir="auto">«{s.wakeWord}»</b></li>{/if}
      {#if s.mcpServers.length}<li><b>{L(`${s.mcpServers.length} MCP servers`, `${num(s.mcpServers.length)} سرور MCP`)}</b> <small dir="ltr">{s.mcpServers.join(' · ')}</small></li>{/if}
      {#if s.assistantName || s.userName}<li>{L('Names:', 'اسم‌ها:')} {s.assistantName ?? ''} {s.userName ? `/ ${s.userName}` : ''}</li>{/if}
      {#if s.voice || s.theme}<li>{L('Voice and look:', 'صدا و ظاهر:')} <span dir="ltr">{[s.voice, s.theme].filter(Boolean).join(' · ')}</span></li>{/if}
    </ul>
    {#each s.problems as p (p)}<p class="warn" dir="auto">{p}</p>{/each}
    {#if done}
      <p class="ok" transition:slide={{ duration: 200 }}>{L('Moved ✓', 'منتقل شد ✓')} <span dir="ltr">{done.join(' · ')}</span></p>
    {:else if report?.importedAt}
      <p class="muted">{L(`Already moved on ${day(report.importedAt)}.`, `قبلاً در ${day(report.importedAt)} منتقل شده است.`)}</p>
    {/if}
    {#if error}<p class="warn" transition:slide={{ duration: 200 }} dir="auto">✕ {error}</p>{/if}
    <button class="btn" disabled={busy || Boolean(report?.importedAt)} onclick={run}>
      {#if busy}<span class="spin"></span>{L('Moving…', 'در حال انتقال…')}{:else if report?.importedAt}{L('Moved', 'منتقل شده')}{:else}{L('Move from version 1', 'انتقال از نسخه ۱')}{/if}
    </button>
  </div>
{/if}

<style>
  .sub {
    margin: 18px 4px 10px;
    font-size: 15px;
    font-weight: 700;
    color: var(--text-muted);
  }
  .card {
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
    padding: 14px 18px;
  }
  .lead {
    margin: 0 0 8px;
    font-size: 13px;
    color: var(--text-muted);
  }
  ul {
    margin: 0 0 12px;
    padding-inline-start: 18px;
    font-size: 13px;
    line-height: 2;
  }
  small {
    color: var(--text-dim);
    font-size: 11.5px;
  }
  .warn {
    margin: 0 0 10px;
    font-size: 12.5px;
    color: #fde68a;
  }
  .ok {
    margin: 0 0 10px;
    font-size: 12.5px;
    color: #a7f3d0;
  }
  .muted {
    margin: 0 0 10px;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 18px;
    border: none;
    border-radius: 10px;
    background: #fff;
    color: #000;
    font-size: 13px;
    font-weight: 700;
    cursor: pointer;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .spin {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid rgba(0, 0, 0, 0.2);
    border-top-color: #000;
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
