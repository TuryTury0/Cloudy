<script lang="ts">
  // The files tab: browse the PC's folders, download a file to the phone, send a phone file to the PC, and follow
  // the transfers (progress, open a downloaded file, where an uploaded one was saved on the PC).
  import { fade, fly, slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { invoke } from '../lib/ipc';
  import type { FileEntry, Listing, TransferEvent } from '../lib/types';
  import Icon, { type IconName } from './Icon.svelte';
  import PageHead from './PageHead.svelte';
  import Sheet from './Sheet.svelte';
  import { haptic, onTransfer, toast, ui } from './store.svelte';
  import { faError, fileKind, formatDate, formatSize, type FileKind } from './text';

  let { active }: { active: boolean } = $props();

  const ICONS: Record<FileKind, IconName> = {
    folder: 'folder',
    drive: 'drive',
    image: 'image',
    video: 'video',
    audio: 'audio',
    doc: 'doc',
    archive: 'archive',
    app: 'app',
    file: 'file'
  };
  /** Rows drawn at first; a long folder shows more on request. */
  const PAGE = 200;

  let listing = $state<Listing | null>(null);
  let loading = $state(false);
  let error = $state('');
  let filter = $state('');
  let shown = $state(PAGE);
  let asked = $state<FileEntry | null>(null);
  let sending = $state(false);
  let crumbsEl = $state<HTMLElement>();

  const entries = $derived.by(() => {
    const all = listing?.entries ?? [];
    const q = filter.trim().toLowerCase();
    return q ? all.filter((e) => e.name.toLowerCase().includes(q)) : all;
  });

  /** "C:\Users\me" -> C:\ > Users > me, each with the path to open. */
  const crumbs = $derived.by(() => {
    const path = listing?.path ?? '';
    if (!path) return [];
    const parts = path.split(/[\\/]+/).filter(Boolean);
    const out: Array<{ name: string; path: string }> = [];
    let acc = path.startsWith('/') ? '/' : '';
    parts.forEach((part, i) => {
      if (i === 0 && /^[A-Za-z]:$/.test(part)) acc = `${part}\\`;
      else acc = acc && !/[\\/]$/.test(acc) ? `${acc}\\${part}` : `${acc}${part}`;
      out.push({ name: part, path: acc });
    });
    return out;
  });

  let request = 0;

  async function open(path: string | null) {
    const mine = ++request;
    loading = true;
    error = '';
    try {
      const result = await invoke<Listing>('files', { path });
      if (mine !== request) return;
      if (result) {
        listing = result;
        filter = '';
        shown = PAGE;
      }
    } catch (e) {
      if (mine === request) error = faError(e);
    } finally {
      if (mine === request) loading = false;
    }
  }

  function back() {
    if (!listing?.path) return;
    haptic();
    void open(listing.parent);
  }

  function pick(entry: FileEntry) {
    haptic();
    if (entry.dir) void open(entry.path);
    else asked = entry;
  }

  async function download() {
    const entry = asked;
    asked = null;
    if (!entry) return;
    try {
      await invoke('download', { path: entry.path, name: entry.name, size: entry.size ?? 0 });
      toast(`دانلود «${entry.name}» شروع شد.`);
    } catch (e) {
      toast(faError(e), 'error');
    }
  }

  async function sendFile() {
    if (sending) return;
    haptic();
    sending = true;
    try {
      const started = await invoke<Pick<TransferEvent, 'id' | 'name' | 'size'>>('send_file');
      if (started) onTransfer({ ...started, direction: 'up', done: 0 });
    } catch (e) {
      // Closing the picker is not an error.
      if (!/No file was chosen/i.test(String(e))) toast(faError(e), 'error');
    } finally {
      sending = false;
    }
  }

  async function openFile(uri: string) {
    haptic();
    try {
      await invoke('open_file', { uri });
    } catch (e) {
      toast(faError(e), 'error');
    }
  }

  function clearDone() {
    ui.transfers = ui.transfers.filter((t) => !t.finished && !t.error);
  }

  // The starting places load the first time this tab is seen with the link up.
  $effect(() => {
    if (active && ui.link.connected && !listing && !loading && !error) void open(null);
  });

  // The breadcrumb shows its end (the open folder).
  $effect(() => {
    void crumbs;
    crumbsEl?.scrollTo({ left: crumbsEl.scrollWidth, behavior: 'smooth' });
  });
</script>

<div class="files-page">
  <PageHead title="فایل‌ها" sub="فایل‌های کامپیوتر">
    {#snippet actions()}
      <button class="icon-btn" onclick={() => (haptic(), open(listing?.path || null))} aria-label="تازه کردن" disabled={loading || !ui.link.connected}>
        <span class:turning={loading}><Icon name="refresh" size={19} /></span>
      </button>
    {/snippet}
  </PageHead>

  <div class="nav">
    <button class="icon-btn" onclick={back} disabled={!listing?.path} aria-label="برگشت">
      <Icon name="back" size={20} />
    </button>
    <div class="crumbs" dir="ltr" bind:this={crumbsEl}>
      <button class="crumb" class:on={!listing?.path} onclick={() => (haptic(), open(null))} aria-label="شروع">
        <Icon name="home" size={16} />
      </button>
      {#each crumbs as c, i (c.path)}
        <span class="sep">›</span>
        <button class="crumb" class:on={i === crumbs.length - 1} onclick={() => (haptic(), open(c.path))}>{c.name}</button>
      {/each}
    </div>
  </div>

  <div class="scroll">
    {#if ui.transfers.length}
      <section class="transfers glass" transition:slide={{ duration: 240 }}>
        <header>
          <b>انتقال‌ها</b>
          {#if ui.transfers.some((t) => t.finished || t.error)}
            <button class="text-btn" onclick={clearDone}>پاک کردن</button>
          {/if}
        </header>
        {#each ui.transfers as t (t.id)}
          {@const pct = t.size ? Math.min(100, ((t.done ?? 0) / t.size) * 100) : t.finished ? 100 : 0}
          <div class="transfer" transition:slide={{ duration: 200 }}>
            <span class="dir {t.direction}" class:bad={t.error} class:ok={t.finished}>
              <Icon name={t.error ? 'close' : t.finished ? 'check' : t.direction === 'down' ? 'download' : 'upload'} size={16} stroke={2.2} />
            </span>
            <div class="info">
              <b dir="auto">{t.name}</b>
              {#if t.error}
                <small class="bad">{faError(t.error)}</small>
              {:else if t.finished}
                <small>
                  {#if t.direction === 'down'}در پوشه Downloads گوشی ذخیره شد{:else}ذخیره شد در کامپیوتر: <bdi dir="ltr">{t.path}</bdi>{/if}
                </small>
              {:else}
                <div class="bar"><i style="width: {pct}%"></i></div>
                <small>
                  {t.direction === 'down' ? 'دریافت' : 'ارسال'}: {formatSize(t.done ?? 0)} از {formatSize(t.size)}{t.rate > 0 ? ` · ${formatSize(t.rate)}/ث` : ''}
                </small>
              {/if}
            </div>
            {#if t.finished && t.direction === 'down' && t.uri}
              <button class="open" onclick={() => openFile(t.uri!)}>باز کردن</button>
            {/if}
          </div>
        {/each}
      </section>
    {/if}

    {#if (listing?.entries.length ?? 0) > 12}
      <input class="filter glass" bind:value={filter} placeholder="جستجو در این پوشه" aria-label="جستجو در این پوشه" dir="auto" />
    {/if}

    {#if !ui.link.connected && !listing}
      <div class="state"><span class="spin"></span><p>در حال اتصال به کامپیوتر…</p></div>
    {:else if error}
      <div class="state" in:fade>
        <p class="bad">{error}</p>
        <button class="btn ghost" onclick={() => open(listing?.path || null)}>دوباره</button>
      </div>
    {:else if !listing}
      <div class="state"><span class="spin"></span></div>
    {:else}
      {#key listing.path}
        <div class="list glass" class:dim={loading} in:fly={{ y: 12, duration: 260, easing: cubicOut }}>
          {#each entries.slice(0, shown) as e (e.path)}
            {@const kind = fileKind(e.name, e.dir, e.path)}
            <button class="row" onclick={() => pick(e)}>
              <span class="ficon {kind}"><Icon name={ICONS[kind]} size={20} /></span>
              <span class="meta">
                <b dir="auto">{e.name}</b>
                {#if !e.dir || e.modified}
                  <small>{e.dir ? '' : formatSize(e.size)}{!e.dir && e.modified ? ' · ' : ''}{formatDate(e.modified)}</small>
                {/if}
              </span>
              <span class="go"><Icon name={e.dir ? 'forward' : 'download'} size={17} /></span>
            </button>
          {:else}
            <p class="empty">{filter ? 'چیزی پیدا نشد.' : 'این پوشه خالی است.'}</p>
          {/each}
          {#if entries.length > shown}
            <button class="more" onclick={() => (shown += PAGE)}>نمایش بیشتر ({entries.length - shown})</button>
          {/if}
        </div>
      {/key}
    {/if}
  </div>

  <button class="fab" onclick={sendFile} disabled={sending || !ui.link.connected}>
    {#if sending}<span class="spin dark"></span>{:else}<Icon name="upload" size={20} stroke={2.2} />{/if}
    ارسال فایل به کامپیوتر
  </button>
</div>

<Sheet open={asked !== null} title="دانلود به گوشی؟" onclose={() => (asked = null)}>
  {#if asked}
    {@const kind = fileKind(asked.name, false)}
    <div class="ask">
      <span class="ficon big {kind}"><Icon name={ICONS[kind]} size={26} /></span>
      <div class="meta">
        <b dir="auto">{asked.name}</b>
        <small>{formatSize(asked.size)}{asked.modified ? ` · ${formatDate(asked.modified)}` : ''}</small>
      </div>
    </div>
    <p class="where">در پوشه Downloads گوشی ذخیره می‌شود.</p>
    <div class="sheet-buttons">
      <button class="btn primary" onclick={download}><Icon name="download" size={19} />دانلود</button>
      <button class="btn ghost" onclick={() => (asked = null)}>لغو</button>
    </div>
  {/if}
</Sheet>

<style>
  .files-page {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .nav {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px 8px;
  }
  .crumbs {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 2px;
    overflow-x: auto;
    padding: 4px;
    border-radius: 16px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.07);
    scrollbar-width: none;
  }
  .crumbs::-webkit-scrollbar {
    display: none;
  }
  .crumb {
    flex-shrink: 0;
    display: grid;
    place-items: center;
    padding: 6px 9px;
    border: none;
    border-radius: 11px;
    background: none;
    color: var(--text-dim);
    font: inherit;
    font-size: 13px;
    white-space: nowrap;
  }
  .crumb.on {
    background: rgba(255, 255, 255, 0.1);
    color: var(--text);
  }
  .sep {
    color: var(--text-dim);
    opacity: 0.5;
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 0 12px 96px;
  }
  .turning {
    display: block;
    animation: spin 0.9s linear infinite;
  }
  .filter {
    width: 100%;
    padding: 11px 16px;
    color: var(--text);
    font: inherit;
    font-size: 15px;
    outline: none;
    border-radius: 18px;
  }
  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 48px 0;
    color: var(--text-dim);
    font-size: 14px;
    text-align: center;
  }
  .state p {
    margin: 0;
  }
  .bad {
    color: #fca5a5;
  }
  .list {
    display: flex;
    flex-direction: column;
    padding: 6px;
    transition: opacity 200ms ease;
  }
  .list.dim {
    opacity: 0.55;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px;
    border: none;
    border-radius: 16px;
    background: none;
    color: var(--text);
    font: inherit;
    text-align: start;
  }
  .row:active {
    background: rgba(255, 255, 255, 0.08);
  }
  .row + .row {
    box-shadow: 0 -1px 0 rgba(255, 255, 255, 0.05);
  }
  .ficon {
    width: 40px;
    height: 40px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    border-radius: 13px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-muted);
  }
  .ficon.big {
    width: 54px;
    height: 54px;
    border-radius: 17px;
  }
  .ficon.folder,
  .ficon.drive {
    background: rgba(56, 189, 248, 0.16);
    color: #7dd3fc;
  }
  .ficon.image {
    background: rgba(236, 72, 153, 0.16);
    color: #f9a8d4;
  }
  .ficon.video {
    background: rgba(168, 85, 247, 0.18);
    color: #d8b4fe;
  }
  .ficon.audio {
    background: rgba(34, 197, 94, 0.16);
    color: #86efac;
  }
  .ficon.doc {
    background: rgba(251, 191, 36, 0.15);
    color: #fde68a;
  }
  .ficon.archive {
    background: rgba(249, 115, 22, 0.16);
    color: #fdba74;
  }
  .ficon.app {
    background: rgba(148, 163, 184, 0.18);
    color: #e2e8f0;
  }
  .meta {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .meta b {
    font-size: 14.5px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .meta small {
    font-size: 12px;
    color: var(--text-dim);
  }
  .go {
    color: var(--text-dim);
  }
  .empty {
    margin: 0;
    padding: 28px 0;
    text-align: center;
    color: var(--text-dim);
    font-size: 14px;
  }
  .more {
    margin: 6px;
    padding: 10px;
    border: none;
    border-radius: 14px;
    background: rgba(255, 255, 255, 0.07);
    color: var(--text-muted);
    font: inherit;
    font-size: 13px;
  }
  .transfers {
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-height: 46vh;
    overflow-y: auto;
  }
  .transfers header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 14px;
  }
  .text-btn {
    padding: 4px 8px;
    border: none;
    background: none;
    color: var(--text-dim);
    font: inherit;
    font-size: 12.5px;
  }
  .transfer {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .dir {
    width: 32px;
    height: 32px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    border-radius: 11px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--accent-text);
  }
  .dir.ok {
    background: rgba(52, 211, 153, 0.16);
    color: #6ee7b7;
  }
  .dir.bad {
    background: rgba(248, 113, 113, 0.16);
    color: #fca5a5;
  }
  .info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .info b {
    font-size: 13.5px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .info small {
    font-size: 11.5px;
    color: var(--text-dim);
    overflow-wrap: anywhere;
  }
  .info small.bad {
    color: #fca5a5;
  }
  .bar {
    height: 5px;
    border-radius: 5px;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: linear-gradient(90deg, var(--accent), var(--accent-2));
    box-shadow: 0 0 10px var(--glow);
    transition: width 300ms ease;
  }
  .open {
    flex-shrink: 0;
    padding: 7px 12px;
    border: none;
    border-radius: 12px;
    background: #fff;
    color: #000;
    font: inherit;
    font-size: 12.5px;
    font-weight: 750;
  }
  .fab {
    position: absolute;
    bottom: 14px;
    inset-inline-end: 14px;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 14px 18px;
    border: none;
    border-radius: 22px;
    background: #fff;
    color: #000;
    font: inherit;
    font-size: 14.5px;
    font-weight: 800;
    box-shadow:
      0 14px 34px -10px rgba(0, 0, 0, 0.8),
      0 0 30px -10px var(--glow);
  }
  .fab:disabled {
    opacity: 0.55;
  }
  .ask {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .ask .meta b {
    white-space: normal;
    overflow-wrap: anywhere;
  }
  .where {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .sheet-buttons {
    display: flex;
    gap: 10px;
  }
  .sheet-buttons :global(.btn) {
    flex: 1;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>