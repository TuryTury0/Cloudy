<script lang="ts">
  // Settings > Home: the first page. What the assistant can do now and how each part is doing (model, phone, search,
  // MCP tools, today's use), with a few requests to try. Every card opens its own page; a call that fails shows "—".
  import { onMount } from 'svelte';
  import { slide } from 'svelte/transition';
  import { L, num } from '@anytask/ui';
  import { invoke, listen } from '../lib/ipc';
  import type { McpReport, PhoneStatus, SearchKeys, Settings, UsageSummary } from '../lib/types';

  type Target = 'models' | 'phone' | 'tools' | 'logs';
  interface Props {
    onopen: (page: Target) => void;
  }
  let { onopen }: Props = $props();

  /** What to type is written the way people type it; the line under it is [English, Persian]. */
  const EXAMPLES: Array<{ text: string; what: [string, string] }> = [
    { text: 'gheimat dollar chande?', what: ['Live prices and news', 'قیمت و خبر لحظه‌ای'] },
    { text: 'ye search-e kamel kon: behtarin laptop ta 50 milion', what: ['Deep research from many sources', 'جستجوی کامل از چند منبع'] },
    { text: 'saat 7 yadam bendaz', what: ['Alarms and reminders', 'زنگ و یادآوری'] },
    { text: 'ahang Believer ro play kon', what: ['Plays music on this computer', 'پخش آهنگ روی همین کامپیوتر'] },
    { text: 'file gozaresh.pdf ro befrest be goshim', what: ['Sends a file to your phone', 'فرستادن فایل به گوشی'] },
    { text: 'goshimo peyda kon', what: ['Rings your phone so you can find it', 'گوشی را زنگ می‌زند تا پیدایش کنی'] }
  ];

  const ICONS = {
    assistant: 'M12 3l1.8 4.7L18.5 9.5l-4.7 1.8L12 16l-1.8-4.7L5.5 9.5l4.7-1.8zM18 15l.9 2.1L21 18l-2.1.9L18 21l-.9-2.1L15 18l2.1-.9z',
    phone: 'M8 2h8a2 2 0 012 2v16a2 2 0 01-2 2H8a2 2 0 01-2-2V4a2 2 0 012-2zM11 18h2',
    search: 'M11 4a7 7 0 110 14 7 7 0 010-14zM20 20l-4-4',
    tools: 'M14 6l4 4-8 8H6v-4zM16 4l4 4',
    ask: 'M5 5h14v10H9l-4 4zM9 9h6M9 12h4'
  };

  // null = not known (still loading, the call failed, or a plain browser): the card shows "—".
  let settings = $state<Settings | null>(null);
  let modelKeys = $state<Record<string, boolean> | null>(null);
  let phone = $state<PhoneStatus | null>(null);
  let searchKeys = $state<SearchKeys | null>(null);
  let mcp = $state<McpReport | null>(null);
  let usage = $state<UsageSummary | null>(null);

  let ringing = $state(false);
  let ringBusy = $state(false);
  let phoneNote = $state('');
  let copied = $state('');
  let ringTimer: ReturnType<typeof setTimeout> | undefined;
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  /** A call that never throws: a failure or a plain browser gives null. */
  function safe<T>(command: string, args?: Record<string, unknown>): Promise<T | null> {
    return invoke<T>(command, args).then(
      (v) => v ?? null,
      () => null
    );
  }

  const activeModel = $derived.by(() => {
    const models = settings?.models ?? [];
    return models.find((m) => m.id === settings?.activeModel) ?? models[0] ?? null;
  });
  const backupModel = $derived(settings?.backupModel && settings.backupModel !== activeModel?.id ? (settings.models.find((m) => m.id === settings?.backupModel) ?? null) : null);
  const activeHasKey = $derived(activeModel && modelKeys ? modelKeys[activeModel.id] === true : null);

  const connectedPhone = $derived(phone?.devices.find((d) => d.connected) ?? null);

  const search = $derived.by(() => {
    if (!settings && !searchKeys) return null;
    // Same order as the web_search tool (brain.rs, tools/web.rs): a paid key (Tavily, then Brave), then Google
    // through a Gemini model's key, then the free engines.
    const paid = searchKeys?.tavily ? 'Tavily' : searchKeys?.brave ? 'Brave Search' : '';
    if (paid) return { value: L(`${paid} (your key)`, `${paid} (کلید خودت)`), note: L(`Your ${paid} key is used first`, `اول کلید ${paid} تو استفاده می‌شود`), good: true };
    const google = (settings?.models ?? []).some((m) => m.provider === 'gemini' && (modelKeys ? modelKeys[m.id] === true : true));
    if (google)
      return {
        value: L('Google (through your Gemini key)', 'گوگل (با کلید Gemini تو)'),
        note: L('Best for Persian; when it fails, the free search answers', 'بهترین برای فارسی؛ اگر نشد، جستجوی رایگان جواب می‌دهد'),
        good: true
      };
    return {
      value: L('Free search (weaker in Persian)', 'جستجوی رایگان (در فارسی ضعیف‌تر)'),
      note: L('A Gemini model or a Tavily or Brave key in Models gives better results', 'با یک مدل Gemini یا کلید Tavily یا Brave در بخش مدل‌ها نتیجه بهتر می‌شود'),
      good: false
    };
  });

  const servers = $derived(mcp?.servers ?? []);
  const connectedServers = $derived(servers.filter((s) => s.status === 'connected'));
  const failedServers = $derived(servers.filter((s) => s.status === 'error').length);
  const mcpTools = $derived(connectedServers.reduce((n, s) => n + s.tools.length, 0));

  const todayTokens = $derived(usage ? usage.today.input + usage.today.output + usage.today.thinking : 0);

  async function ring() {
    if (ringBusy) return;
    const stop = ringing;
    ringBusy = true;
    phoneNote = '';
    clearTimeout(ringTimer);
    try {
      await invoke('phone_ring', { stop });
      ringing = !stop;
      // The phone stops by itself after 30 s.
      if (ringing) ringTimer = setTimeout(() => (ringing = false), 30_000);
    } catch (e) {
      phoneNote = `✕ ${String(e)}`;
    } finally {
      ringBusy = false;
    }
  }

  async function copy(text: string) {
    clearTimeout(copyTimer);
    try {
      await invoke('copy_text', { text });
      copied = text;
    } catch {
      copied = '';
    }
    copyTimer = setTimeout(() => (copied = ''), 1500);
  }

  onMount(() => {
    void safe<Settings>('get_settings').then((s) => (settings = s));
    void safe<Record<string, boolean>>('model_keys').then((k) => (modelKeys = k));
    void safe<PhoneStatus>('phone_status').then((s) => (phone = s));
    void safe<SearchKeys>('search_keys').then((k) => (searchKeys = k));
    void safe<McpReport>('mcp_report').then((r) => (mcp = r));
    void safe<UsageSummary>('usage_summary').then((u) => (usage = u));
    const stops: Array<() => void> = [];
    function follow<T>(event: string, set: (v: T) => void) {
      void listen<T>(event, set).then(
        (u) => stops.push(u),
        () => {}
      );
    }
    follow<PhoneStatus>('phone-changed', (s) => (phone = s));
    follow<McpReport>('mcp-changed', (r) => (mcp = r));
    follow<UsageSummary>('usage-changed', (u) => (usage = u));
    return () => {
      stops.forEach((stop) => stop());
      clearTimeout(ringTimer);
      clearTimeout(copyTimer);
    };
  });
</script>

<div class="head">
  <div>
    <h1>{L('Home', 'خانه')}</h1>
    <p class="lead">
      {settings
        ? L(`What ${settings.assistantName} can do now, and how each part is doing.`, `${settings.assistantName} الان چه کارهایی می‌کند و هر بخش در چه حالی است.`)
        : L('What the assistant can do now, and how each part is doing.', 'دستیار الان چه کارهایی می‌کند و هر بخش در چه حالی است.')}
    </p>
  </div>
  <button class="today" onclick={() => onopen('logs')} title={L('Open the logs', 'باز کردن لاگ‌ها')}>
    <small>{L('Today', 'امروز')}</small>
    <b>
      {#if usage}
        {L(`${usage.today.requests} requests`, `${num(usage.today.requests)} درخواست`)}
        <span>· {num(todayTokens)} {L('tokens', 'توکن')}</span>
      {:else}—{/if}
    </b>
  </button>
</div>

<div class="grid">
  <div class="card tile" style="--i: 0">
    <button class="open" onclick={() => onopen('models')}>
      <span class="icon"><svg viewBox="0 0 24 24"><path d={ICONS.assistant} /></svg></span>
      <span class="kicker">{L('Assistant', 'دستیار')}</span>
      <svg class="chev" viewBox="0 0 24 24"><path d="M9 6l6 6-6 6" /></svg>
    </button>
    <b class="value" dir="auto">{settings?.assistantName || '—'}</b>
    <div class="line">
      <span class="dot" class:on={activeHasKey === true} class:warn={activeHasKey === false}></span>
      {#if !settings}—
      {:else if activeModel}<span dir="auto">{activeModel.name}</span>{#if activeHasKey === false}<em class="badge warn">{L('no key', 'بدون کلید')}</em>{/if}
      {:else}{L('No model yet; add one in Models', 'هنوز مدلی نیست؛ در بخش مدل‌ها اضافه کن')}{/if}
    </div>
    {#if backupModel}<small dir="auto">{L(`Backup: ${backupModel.name}`, `پشتیبان: ${backupModel.name}`)}</small>{/if}
    <div class="actions">
      <button class="mini" onclick={() => onopen('models')}>{L('Models', 'مدل‌ها')}</button>
    </div>
  </div>

  <div class="card tile" style="--i: 1">
    <button class="open" onclick={() => onopen('phone')}>
      <span class="icon"><svg viewBox="0 0 24 24"><path d={ICONS.phone} /></svg></span>
      <span class="kicker">{L('Phone', 'گوشی')}</span>
      <svg class="chev" viewBox="0 0 24 24"><path d="M9 6l6 6-6 6" /></svg>
    </button>
    <b class="value">
      <span class="dot" class:on={connectedPhone !== null}></span>
      {#if !phone}—
      {:else if !phone.enabled}{L('Off', 'خاموش')}
      {:else if connectedPhone}<span dir="auto">{L(`Connected: ${connectedPhone.name}`, `وصل: ${connectedPhone.name}`)}</span>
      {:else if phone.devices.length}{L('Not connected', 'وصل نیست')}
      {:else}{L('No phone paired yet', 'هنوز گوشی جفت نشده')}{/if}
    </b>
    <small dir="auto">
      {#if phone?.error}✕ {phone.error}
      {:else if !phone}&nbsp;
      {:else if !phone.enabled}{L('Turn it on to use the assistant from your phone', 'روشنش کن تا از گوشی با دستیار کار کنی')}
      {:else if connectedPhone}{L('Requests, files and ringing work from here', 'درخواست، فایل و زنگ از اینجا کار می‌کنند')}
      {:else if phone.devices.length}{L('Open Cloudy on the phone to connect', 'Cloudy را روی گوشی باز کن تا وصل شود')}
      {:else}{L('Pair one with the QR code or the 6-digit code', 'با کد QR یا کد ۶ رقمی جفتش کن')}{/if}
    </small>
    {#if phoneNote}<small class="error" dir="auto" transition:slide={{ duration: 200 }}>{phoneNote}</small>{/if}
    <div class="actions">
      {#if connectedPhone}
        <button class="mini" class:on={ringing} onclick={ring} disabled={ringBusy}>{ringing ? L('Stop ringing', 'قطع زنگ') : L('Ring it', 'زنگ بزن')}</button>
      {/if}
      <button class="mini" onclick={() => onopen('phone')}>{L('Open phone page', 'صفحه گوشی')}</button>
    </div>
  </div>

  <div class="card tile" style="--i: 2">
    <button class="open" onclick={() => onopen('models')}>
      <span class="icon"><svg viewBox="0 0 24 24"><path d={ICONS.search} /></svg></span>
      <span class="kicker">{L('Web search', 'جستجوی وب')}</span>
      <svg class="chev" viewBox="0 0 24 24"><path d="M9 6l6 6-6 6" /></svg>
    </button>
    <b class="value">
      <span class="dot" class:on={search?.good === true} class:warn={search?.good === false}></span>
      {search?.value ?? '—'}
    </b>
    <small>{search?.note ?? ' '}</small>
    <div class="actions">
      <button class="mini" onclick={() => onopen('models')}>{L('Search keys', 'کلیدهای جستجو')}</button>
    </div>
  </div>

  <div class="card tile" style="--i: 3">
    <button class="open" onclick={() => onopen('tools')}>
      <span class="icon"><svg viewBox="0 0 24 24"><path d={ICONS.tools} /></svg></span>
      <span class="kicker">{L('Tools & MCP', 'ابزارها و MCP')}</span>
      <svg class="chev" viewBox="0 0 24 24"><path d="M9 6l6 6-6 6" /></svg>
    </button>
    <b class="value">
      <span class="dot" class:on={connectedServers.length > 0} class:bad={failedServers > 0 && connectedServers.length === 0}></span>
      {#if !mcp}—
      {:else if servers.length}{L(`${connectedServers.length} of ${servers.length} servers connected`, `${num(connectedServers.length)} از ${num(servers.length)} سرور وصل`)}
      {:else}{L('No MCP server yet', 'هنوز سرور MCP نیست')}{/if}
    </b>
    <small>
      {#if mcp}
        {L(`${mcp.builtin.length} built-in tools · ${mcpTools} MCP tools`, `${num(mcp.builtin.length)} ابزار داخلی · ${num(mcpTools)} ابزار MCP`)}{failedServers
          ? L(` · ${failedServers} with an error`, ` · ${num(failedServers)} با خطا`)
          : ''}
      {:else}&nbsp;{/if}
    </small>
    <div class="actions">
      <button class="mini" onclick={() => onopen('tools')}>{L('Tools & MCP', 'ابزارها و MCP')}</button>
    </div>
  </div>

  <div class="card tile wide" style="--i: 4">
    <div class="open static">
      <span class="icon"><svg viewBox="0 0 24 24"><path d={ICONS.ask} /></svg></span>
      <span class="kicker">{L('What you can ask', 'چه چیزهایی می‌توانی بخواهی')}</span>
    </div>
    <small>
      {#if settings}
        {L(
          `Press ${settings.hotkeys.talk} to talk or ${settings.hotkeys.typeText} to type, in Persian, Finglish or English. Copy one to try it.`,
          `برای صحبت ${settings.hotkeys.talk} و برای نوشتن ${settings.hotkeys.typeText} را بزن؛ فارسی، فینگلیش یا انگلیسی. یکی را کپی کن و امتحان کن.`
        )}
      {:else}
        {L('Say it or type it, in Persian, Finglish or English. Copy one to try it.', 'بگو یا بنویس؛ فارسی، فینگلیش یا انگلیسی. یکی را کپی کن و امتحان کن.')}
      {/if}
    </small>
    <div class="asks">
      {#each EXAMPLES as ex (ex.text)}
        <div class="ask">
          <div class="ask-text">
            <code dir="ltr">{ex.text}</code>
            <small>{L(...ex.what)}</small>
          </div>
          <button class="copy" class:done={copied === ex.text} onclick={() => copy(ex.text)} aria-label={L('Copy', 'کپی')} title={L('Copy', 'کپی')}>
            {#if copied === ex.text}
              <svg viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7.5" /></svg>
            {:else}
              <svg viewBox="0 0 24 24"><path d="M9 9h10v11H9zM5 15V4h10" /></svg>
            {/if}
          </button>
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
  }
  h1 {
    margin: 0 0 14px;
    font-size: 22px;
    font-weight: 800;
  }
  .lead {
    margin: -6px 0 18px;
    color: var(--text-dim);
    font-size: 13px;
  }
  .today {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 2px;
    padding: 8px 14px;
    border-radius: 14px;
    border: 1px solid rgba(255, 255, 255, 0.06);
    background: var(--surface);
    box-shadow: var(--card-shadow);
    color: var(--text);
    cursor: pointer;
    transition: transform var(--medium) var(--ease-spring), border-color var(--fast);
  }
  .today:hover {
    transform: translateY(-1px);
    border-color: color-mix(in srgb, var(--accent-text) 30%, transparent);
  }
  .today small {
    color: var(--text-dim);
    font-size: 11px;
  }
  .today b {
    font-size: 13px;
    font-weight: 700;
  }
  .today b span {
    color: var(--text-dim);
    font-weight: 600;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 12px;
  }
  .card {
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
    padding: 6px 18px;
  }
  .tile {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 14px 16px;
    border-radius: 18px;
    box-shadow: var(--card-shadow);
    transition: transform var(--medium) var(--ease-spring), border-color var(--fast), box-shadow var(--fast);
    animation: rise 420ms var(--ease-out) backwards;
    animation-delay: calc(var(--i) * 45ms);
  }
  .tile:not(.wide):hover {
    transform: translateY(-2px);
    border-color: color-mix(in srgb, var(--accent-text) 28%, transparent);
    box-shadow: var(--card-shadow), 0 14px 30px -18px var(--glow);
  }
  .wide {
    grid-column: 1 / -1;
  }
  /* The head button covers the whole card, so a click anywhere opens the page; the small buttons sit above it. */
  .open {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0;
    border: none;
    background: none;
    color: var(--text-muted);
    font-size: 12.5px;
    font-weight: 700;
    text-align: start;
    cursor: pointer;
  }
  .open.static {
    cursor: default;
  }
  .open:not(.static)::after {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: inherit;
  }
  .open:focus-visible {
    outline: none;
  }
  .open:focus-visible::after {
    box-shadow: 0 0 0 2px var(--accent-text);
  }
  .icon {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 10px;
    background: color-mix(in srgb, var(--accent-text) 12%, rgba(255, 255, 255, 0.04));
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.18);
  }
  .icon svg,
  .chev {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: var(--accent-text);
    stroke-width: 1.7;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .kicker {
    flex: 1;
  }
  .chev {
    stroke: var(--text-dim);
    transition: transform var(--medium) var(--ease-spring), stroke var(--fast);
  }
  .tile:hover .chev {
    stroke: var(--text);
    transform: translateX(2px);
  }
  :global([dir='rtl']) .chev {
    scale: -1 1;
  }
  .value {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 6px;
    font-size: 15.5px;
    font-weight: 800;
    color: var(--text);
    overflow-wrap: anywhere;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-muted);
  }
  .tile small {
    color: var(--text-dim);
    font-size: 12px;
    line-height: 1.5;
  }
  .tile small.error {
    color: #fecaca;
  }
  .dot {
    flex-shrink: 0;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-dim);
  }
  .dot.on {
    background: #34d399;
    box-shadow: 0 0 10px rgba(52, 211, 153, 0.7);
  }
  .dot.warn {
    background: #fbbf24;
  }
  .dot.bad {
    background: #f87171;
    box-shadow: 0 0 10px rgba(248, 113, 113, 0.6);
  }
  .badge {
    font-style: normal;
    font-size: 10.5px;
    font-weight: 600;
    padding: 1px 8px;
    border-radius: 999px;
  }
  .badge.warn {
    color: #fde68a;
    background: rgba(251, 191, 36, 0.1);
  }
  .actions {
    position: relative;
    z-index: 1;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: auto;
    padding-top: 8px;
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
    transition: background var(--fast), color var(--fast), transform var(--fast);
  }
  .mini:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.13);
    color: var(--text);
  }
  .mini:active {
    transform: scale(0.96);
  }
  .mini:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .mini.on {
    background: color-mix(in srgb, var(--accent-text) 30%, transparent);
    color: var(--text);
  }
  .asks {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 8px;
    margin-top: 8px;
  }
  .ask {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 10px 9px 12px;
    border-radius: 12px;
    background: rgba(0, 0, 0, 0.18);
    border: 1px solid rgba(255, 255, 255, 0.05);
    transition: border-color var(--fast), background var(--fast);
  }
  .ask:hover {
    border-color: rgba(255, 255, 255, 0.12);
    background: rgba(0, 0, 0, 0.24);
  }
  .ask-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .ask code {
    font-family: var(--mono);
    font-size: 12.5px;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Requests are written left to right; in a Persian page they sit on the right. */
  :global([dir='rtl']) .ask code {
    text-align: right;
  }
  .ask small {
    font-size: 11.5px;
  }
  .copy {
    flex-shrink: 0;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.06);
    cursor: pointer;
    transition: background var(--fast), transform var(--fast);
  }
  .copy:hover {
    background: rgba(255, 255, 255, 0.13);
  }
  .copy:active {
    transform: scale(0.92);
  }
  .copy svg {
    width: 14px;
    height: 14px;
    fill: none;
    stroke: var(--text-muted);
    stroke-width: 1.7;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .copy.done {
    background: rgba(52, 211, 153, 0.14);
  }
  .copy.done svg {
    stroke: #34d399;
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
  }
</style>