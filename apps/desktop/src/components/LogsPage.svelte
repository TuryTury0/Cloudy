<script lang="ts">
  // Settings > Logs: what the assistant did, live, with times (requests, tool steps, model answers, errors).
  import { onMount, tick } from 'svelte';
  import { fly } from 'svelte/transition';
  import { L, num } from '@anytask/ui';
  import { invoke, listen } from '../lib/ipc';
  import type { LogLine, Tally, UsageSummary } from '../lib/types';

  const KINDS: Array<{ id: string; en: string; fa: string }> = [
    { id: 'all', en: 'All', fa: 'همه' },
    { id: 'request', en: 'Requests', fa: 'درخواست‌ها' },
    { id: 'step', en: 'Steps', fa: 'قدم‌ها' },
    { id: 'detail', en: 'Details', fa: 'جزئیات' },
    { id: 'tokens', en: 'Tokens', fa: 'توکن‌ها' },
    { id: 'error', en: 'Errors', fa: 'خطاها' }
  ];

  let lines = $state<LogLine[]>([]);
  let usage = $state<UsageSummary | null>(null);
  const sum = (t: Tally | null | undefined) => (t ? t.input + t.output + t.thinking : 0);
  let filter = $state('');
  let kind = $state('all');
  let follow = $state(true);
  let copied = $state(false);
  let box = $state<HTMLDivElement>();

  const shown = $derived(
    lines.filter((l) => (kind === 'all' || l.kind === kind || (kind === 'error' && l.text.includes('FAILED'))) && (!filter || l.text.toLowerCase().includes(filter.toLowerCase())))
  );

  async function scrollDown() {
    await tick();
    if (follow && box) box.scrollTop = box.scrollHeight;
  }

  function copy() {
    const text = shown.map((l) => `${l.at} ${l.kind.padEnd(7)} ${l.text}`).join('\n');
    void invoke('copy_text', { text });
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  $effect(() => {
    void shown.length;
    void scrollDown();
  });

  onMount(() => {
    let stop = () => {};
    void invoke<LogLine[]>('logs_recent', { limit: 800 }).then((l) => (lines = l ?? []));
    void invoke<UsageSummary>('usage_summary').then((u) => (usage = u ?? null));
    let stopUsage = () => {};
    void listen<UsageSummary>('usage-changed', (u) => (usage = u)).then((u) => (stopUsage = u));
    void listen<LogLine>('log-line', (line) => {
      lines.push(line);
      if (lines.length > 2000) lines.splice(0, lines.length - 2000);
    }).then((u) => (stop = u));
    return () => {
      stop();
      stopUsage();
    };
  });
</script>

<h1>{L('Logs', 'لاگ‌ها')}</h1>
<p class="lead">{L('Everything the assistant does, with its time: requests, steps, model answers and errors. API keys never show here.', 'هر کاری که دستیار می‌کند با زمانش: درخواست‌ها، قدم‌ها، جواب مدل و خطاها. کلیدهای API هیچ‌وقت اینجا نمی‌آیند.')}</p>

<div class="stats">
  {#each [{ id: 'last', label: L('Last request', 'آخرین درخواست'), t: usage?.last }, { id: 'today', label: L('Today', 'امروز'), t: usage?.today }, { id: 'total', label: L('Since the start', 'از اول'), t: usage?.total }] as card (card.id)}
    <div class="stat">
      <small>{card.label}</small>
      <b>{num(sum(card.t))} <span>{L('tokens', 'توکن')}</span></b>
      <em>
        {#if card.t}
          {num(card.t.input)} {L('in', 'ورودی')} · {num(card.t.output)} {L('out', 'خروجی')}{card.t.thinking ? ` · ${num(card.t.thinking)} ${L('thinking', 'فکر')}` : ''}
          {#if card.id !== 'last'} · {num(card.t.requests)} {L('requests', 'درخواست')}{/if}
          · {num(card.t.calls)} {L('model calls', 'بار مدل')}
        {:else}{L('No requests yet', 'هنوز درخواستی نیست')}{/if}
      </em>
    </div>
  {/each}
</div>

<div class="bar">
  <div class="kinds">
    {#each KINDS as k (k.id)}
      <button class="kind" class:on={kind === k.id} onclick={() => (kind = k.id)}>{L(k.en, k.fa)}</button>
    {/each}
  </div>
  <input class="search" placeholder={L('Search…', 'جستجو…')} bind:value={filter} dir="auto" />
</div>

<div class="logs" bind:this={box} onscroll={() => box && (follow = box.scrollHeight - box.scrollTop - box.clientHeight < 40)}>
  {#each shown as line, i (i + line.at + line.text)}
    <div class="line {line.kind}" class:failed={line.text.includes('FAILED')} in:fly={{ y: 4, duration: 160 }}>
      <span class="at">{line.at}</span>
      <span class="tag">{line.kind}</span>
      <span class="text" dir="auto">{line.text}</span>
    </div>
  {:else}
    <div class="empty">{L('Nothing recorded yet.', 'هنوز چیزی ثبت نشده.')}</div>
  {/each}
</div>

<div class="actions">
  <span class="count">{L(`${shown.length} lines`, `${num(shown.length)} خط`)}</span>
  <button class="mini" onclick={copy}>{copied ? L('Copied ✓', 'کپی شد ✓') : L('Copy', 'کپی')}</button>
  <button class="mini" onclick={() => invoke('open_logs_folder')}>{L('Open the logs folder', 'باز کردن پوشه لاگ')}</button>
</div>

<style>
  h1 {
    margin: 0 0 14px;
    font-size: 22px;
    font-weight: 800;
  }
  .lead {
    margin: -6px 0 14px;
    color: var(--text-dim);
    font-size: 13px;
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
    margin-bottom: 12px;
  }
  .stat {
    padding: 12px 14px;
    border-radius: 14px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
  }
  .stat small {
    display: block;
    color: var(--text-dim);
    font-size: 11.5px;
  }
  .stat b {
    display: block;
    margin: 2px 0;
    font-size: 19px;
    font-weight: 800;
  }
  .stat b span {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-dim);
  }
  .stat em {
    font-style: normal;
    font-size: 11px;
    color: var(--text-muted);
  }
  .tokens .text {
    color: #fde68a;
  }
  .bar {
    display: flex;
    gap: 10px;
    align-items: center;
    margin-bottom: 10px;
  }
  .kinds {
    display: flex;
    gap: 4px;
    padding: 3px;
    border-radius: 12px;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid var(--border);
  }
  .kind {
    padding: 6px 12px;
    border: none;
    border-radius: 9px;
    background: transparent;
    color: var(--text-dim);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--medium) var(--ease-out), color var(--fast);
  }
  .kind.on {
    background: rgba(255, 255, 255, 0.12);
    color: var(--text);
  }
  .search {
    flex: 1;
    padding: 8px 12px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: rgba(0, 0, 0, 0.3);
    color: var(--text);
    font-size: 12.5px;
    outline: none;
  }
  .search:focus {
    border-color: var(--accent-text);
  }
  .logs {
    height: 380px;
    overflow-y: auto;
    padding: 8px 10px;
    border-radius: 14px;
    background: rgba(0, 0, 0, 0.35);
    border: 1px solid rgba(255, 255, 255, 0.06);
    direction: ltr;
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.6;
    user-select: text;
  }
  .line {
    display: flex;
    gap: 8px;
    padding: 1px 0;
    color: var(--text-muted);
  }
  .at {
    color: var(--text-dim);
    flex-shrink: 0;
  }
  .tag {
    flex-shrink: 0;
    width: 56px;
    color: var(--text-dim);
  }
  .text {
    white-space: pre-wrap;
    word-break: break-word;
  }
  .request .text {
    color: #fff;
    font-weight: 600;
  }
  .request .tag,
  .answer .tag {
    color: var(--accent-text);
  }
  .step .text {
    color: #c4b5fd;
  }
  .detail .text {
    color: var(--text-dim);
  }
  .answer .text {
    color: #a7f3d0;
  }
  .error .text,
  .failed .text {
    color: #fca5a5;
  }
  .empty {
    color: var(--text-dim);
    padding: 20px;
    text-align: center;
    font-family: var(--font);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 10px;
  }
  .count {
    margin-inline-end: auto;
    font-size: 12px;
    color: var(--text-dim);
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
  }
  .mini:hover {
    background: rgba(255, 255, 255, 0.14);
    color: var(--text);
  }
</style>
