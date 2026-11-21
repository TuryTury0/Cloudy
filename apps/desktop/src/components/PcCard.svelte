<script lang="ts">
  // Settings > General: what the assistant knows about this PC (default browser, apps by kind) and the
  // preferences it learned, e.g. which music app to use. A preference can be removed; it will ask again.
  import { onMount } from 'svelte';
  import { fly, slide } from 'svelte/transition';
  import { L, day, num } from '@anytask/ui';
  import { invoke, listen } from '../lib/ipc';
  import type { PcView } from '../lib/types';

  /** [English, Persian] */
  const PREFERENCE_LABEL: Record<string, [string, string]> = {
    preferred_music_app: ['Music app', 'برنامه موسیقی'],
    preferred_video_app: ['Video app', 'برنامه ویدیو'],
    preferred_browser: ['Browser', 'مرورگر'],
    preferred_chat_app: ['Messaging app', 'برنامه پیام‌رسان']
  };

  let pc = $state<PcView | null>(null);
  let busy = $state(false);
  let error = $state('');

  async function rescan() {
    busy = true;
    error = '';
    try {
      pc = (await invoke<PcView>('pc_rescan')) ?? pc;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function forget(key: string) {
    pc = (await invoke<PcView>('forget_preference', { key })) ?? pc;
  }

  onMount(() => {
    void invoke<PcView>('pc_view').then((v) => v && (pc = v));
    let stop = () => {};
    void listen<PcView>('pc-changed', (v) => (pc = v)).then((u) => (stop = u));
    return () => stop();
  });
</script>

<h2 class="sub">{L('This computer', 'این کامپیوتر')}</h2>
<div class="card">
  <div class="row">
    <div>
      <b>{L('Apps the assistant knows', 'برنامه‌هایی که دستیار می‌شناسد')}</b>
      <small>
        {#if pc?.scannedAt}
          {L(
            `${num(pc.appCount)} apps, scanned ${day(pc.scannedAt)}; tasks use these apps, not websites`,
            `${num(pc.appCount)} برنامه — اسکن ${day(pc.scannedAt)}؛ برای کارها از همین برنامه‌ها استفاده می‌کند، نه سایت‌ها`
          )}
        {:else}{L('Not scanned yet', 'هنوز اسکن نشده')}{/if}
      </small>
    </div>
    <button class="mini" disabled={busy} onclick={rescan}>{busy ? L('Scanning…', 'در حال اسکن…') : L('Scan again', 'اسکن دوباره')}</button>
  </div>
  {#if pc?.profile}
    <pre dir="ltr" transition:slide={{ duration: 200 }}>{pc.profile}</pre>
  {/if}
  {#if pc?.preferences.length}
    <div class="prefs">
      <small>{L('Preferences it has learned (remove one and it asks again):', 'ترجیح‌هایی که یاد گرفته (حذف کنی، دوباره می‌پرسد):')}</small>
      {#each pc.preferences as p (p.key)}
        <span class="pref" in:fly={{ y: 6, duration: 200 }}>
          {PREFERENCE_LABEL[p.key] ? L(...PREFERENCE_LABEL[p.key]) : p.key}: <b dir="auto">{p.value}</b>
          <button aria-label="forget" onclick={() => forget(p.key)}>×</button>
        </span>
      {/each}
    </div>
  {/if}
  {#if error}<p class="error" dir="auto">{error}</p>{/if}
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
    padding: 6px 18px 10px;
    margin-bottom: 16px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 18px;
    padding: 12px 0 8px;
  }
  .row b {
    display: block;
    font-size: 13.5px;
  }
  small {
    display: block;
    margin-top: 3px;
    color: var(--text-dim);
    font-size: 12px;
  }
  pre {
    margin: 0 0 8px;
    padding: 10px 12px;
    border-radius: 10px;
    background: rgba(0, 0, 0, 0.3);
    color: var(--text-muted);
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.7;
    white-space: pre-wrap;
    text-align: left;
  }
  .prefs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
    padding-bottom: 6px;
  }
  .prefs small {
    width: 100%;
  }
  .pref {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 6px 4px 10px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.07);
    font-size: 12px;
  }
  .pref button {
    width: 18px;
    height: 18px;
    border: none;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.1);
    color: var(--text-muted);
    cursor: pointer;
    line-height: 1;
  }
  .pref button:hover {
    background: rgba(225, 29, 72, 0.85);
    color: #fff;
  }
  .mini {
    padding: 6px 12px;
    border: none;
    border-radius: 9px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }
  .mini:disabled {
    opacity: 0.5;
  }
  .error {
    margin: 0;
    font-size: 12.5px;
    color: #fecaca;
  }
</style>
