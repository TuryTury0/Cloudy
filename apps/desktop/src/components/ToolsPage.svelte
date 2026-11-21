<script lang="ts">
  // Settings > Tools & MCP: AI-assisted setup chat, environment check, server management,
  // live server & per-tool testing, tool allowlist toggles, per-server logs, and manual/JSON fallback.
  import { onMount } from 'svelte';
  import { fade, fly, scale, slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { L, num } from '@anytask/ui';
  import { inTauri, invoke, listen } from '../lib/ipc';
  import { formatKeyValues, joinCommandLine, parseKeyValues, splitCommandLine } from '../lib/mcp-form';
  import type {
    McpReport,
    McpServerConfig,
    McpServerStatus,
    McpServerTestReport,
    McpStatus,
    McpToolInfo
  } from '../lib/types';
  import McpSetupChat from './McpSetupChat.svelte';
  import McpToolTester from './McpToolTester.svelte';
  import Toggle from './Toggle.svelte';

  interface Preset {
    id: string;
    label: [string, string];
    name: string;
    kind: 'command' | 'url';
    line: string;
    vars: string;
    hint: [string, string];
  }

  const PRESETS: Preset[] = [
    {
      id: 'playwright',
      label: ['Playwright (browser)', 'Playwright (مرورگر)'],
      name: 'playwright',
      kind: 'command',
      line: 'npx -y @playwright/mcp@latest',
      vars: '',
      hint: ['Needs Node.js', 'به Node.js نیاز دارد']
    },
    {
      id: 'github',
      label: ['GitHub', 'GitHub'],
      name: 'github',
      kind: 'command',
      line: 'npx -y @modelcontextprotocol/server-github',
      vars: 'GITHUB_PERSONAL_ACCESS_TOKEN=secret:GITHUB_TOKEN',
      hint: ['After saving, put the token under "Secrets"', 'توکن را بعد از ذخیره در بخش «رمزها» بگذار']
    },
    {
      id: 'memory',
      label: ['Memory (notes)', 'حافظه (یادداشت‌ها)'],
      name: 'memory',
      kind: 'command',
      line: 'npx -y @modelcontextprotocol/server-memory',
      vars: '',
      hint: ['Needs Node.js', 'به Node.js نیاز دارد']
    },
    {
      id: 'filesystem',
      label: ['Folder access', 'دسترسی به پوشه'],
      name: 'filesystem',
      kind: 'command',
      line: 'npx -y @modelcontextprotocol/server-filesystem C:/Users/Public/Documents',
      vars: '',
      hint: ['Change the folder at the end; needs Node.js', 'پوشهٔ آخر دستور را عوض کن؛ به Node.js نیاز دارد']
    },
    {
      id: 'thinking',
      label: ['Step-by-step thinking', 'فکر قدم‌به‌قدم'],
      name: 'sequential-thinking',
      kind: 'command',
      line: 'npx -y @modelcontextprotocol/server-sequential-thinking',
      vars: '',
      hint: ['Needs Node.js', 'به Node.js نیاز دارد']
    },
    { id: 'command', label: ['Any program', 'برنامه دلخواه'], name: '', kind: 'command', line: '', vars: '', hint: ['Any MCP stdio program', 'هر برنامه MCP روی stdio'] },
    { id: 'url', label: ['Internet server (URL)', 'سرور اینترنتی (URL)'], name: '', kind: 'url', line: 'https://', vars: '', hint: ['Streamable HTTP', 'Streamable HTTP'] }
  ];

  const STATUS: Record<McpStatus, [string, string]> = {
    connecting: ['connecting…', 'در حال اتصال…'],
    connected: ['connected', 'وصل'],
    error: ['error', 'خطا'],
    disabled: ['off', 'خاموش'],
    stopped: ['not started', 'شروع نشده']
  };

  let report = $state<McpReport | null>(null);
  let form = $state<{ name: string; kind: 'command' | 'url'; line: string; vars: string; only: string; previous?: string } | null>(null);
  let preset = $state('playwright');
  let busy = $state('');
  let installingPrereq = $state('');
  let formError = $state('');
  let confirmDelete = $state('');
  let openTools = $state<Record<string, boolean>>({});
  let openLogs = $state<Record<string, boolean>>({});
  let toolSearch = $state<Record<string, string>>({});
  let testingTool = $state<Record<string, McpToolInfo | null>>({});
  let testNotes = $state<Record<string, { ok: boolean; text: string }>>({});
  let secretInputs = $state<Record<string, string>>({});
  let note = $state('');
  let noteTimer: ReturnType<typeof setTimeout> | undefined;

  const servers = $derived(report?.servers ?? []);
  const connectedTools = $derived(report?.activeToolCount ?? servers.reduce((n, s) => n + (s.status === 'connected' ? s.tools.length : 0), 0));
  const maxTools = $derived(report?.maxTools ?? 40);
  const overToolLimit = $derived(connectedTools > maxTools);

  function flash(text: string) {
    clearTimeout(noteTimer);
    note = text;
    noteTimer = setTimeout(() => note === text && (note = ''), 4500);
  }

  async function run(label: string, call: () => Promise<McpReport | null | undefined>) {
    busy = label;
    try {
      const next = await call();
      if (next) report = next;
      return true;
    } catch (e) {
      flash(String(e));
      return false;
    } finally {
      busy = '';
    }
  }

  function choosePreset(p: Preset) {
    preset = p.id;
    form = { name: p.name, kind: p.kind, line: p.line, vars: p.vars, only: '' };
    formError = '';
  }

  function startAdd() {
    choosePreset(PRESETS[0]);
  }

  function startEdit(name: string) {
    const cfg: McpServerConfig = report?.config.mcpServers[name] ?? {};
    const kind = cfg.url ? 'url' : 'command';
    preset = kind === 'url' ? 'url' : 'command';
    form = {
      name,
      kind,
      line: cfg.url ?? joinCommandLine([cfg.command ?? '', ...(cfg.args ?? [])]),
      vars: formatKeyValues(kind === 'url' ? cfg.headers : cfg.env),
      only: (cfg.tools ?? []).join(', '),
      previous: name
    };
    formError = '';
  }

  async function saveForm() {
    if (!form) return;
    const old: McpServerConfig = (form.previous && report?.config.mcpServers[form.previous]) || {};
    const vars = parseKeyValues(form.vars);
    const only = form.only.split(/[,\s]+/).filter(Boolean);
    let server: McpServerConfig;
    if (form.kind === 'url') {
      server = { url: form.line.trim(), headers: vars, tools: only, disabled: old.disabled };
    } else {
      const [command, ...args] = splitCommandLine(form.line);
      if (!command) {
        formError = L('Write the command to run (for example npx -y …)', 'دستور اجرا را بنویس (مثلاً npx -y …)');
        return;
      }
      server = { command, args, env: vars, tools: only, disabled: old.disabled };
    }
    formError = '';
    busy = 'save';
    try {
      report = (await invoke<McpReport>('mcp_save_server', { name: form.name.trim(), server, previous: form.previous ?? null })) ?? report;
      const saved = form.name.trim();
      form = null;
      const status = report?.servers.find((s) => s.name === saved);
      flash(status?.status === 'connected' ? L(`${saved} connected ✓`, `${saved} وصل شد ✓`) : L(`${saved} saved`, `${saved} ذخیره شد`));
    } catch (e) {
      formError = String(e);
    } finally {
      busy = '';
    }
  }

  async function remove(name: string, cleanSecrets = false) {
    const exclusive = report?.exclusiveSecrets[name] ?? [];
    if (confirmDelete !== name) {
      confirmDelete = name;
      setTimeout(() => confirmDelete === name && (confirmDelete = ''), 5000);
      return;
    }
    confirmDelete = '';
    if (exclusive.length > 0 || cleanSecrets) {
      await run(`remove:${name}`, () => invoke<McpReport>('mcp_remove_server_full', { name, cleanSecrets }));
    } else {
      await run(`remove:${name}`, () => invoke<McpReport>('mcp_remove_server', { name }));
    }
  }

  function setDisabled(name: string, disabled: boolean) {
    void run(`toggle:${name}`, () => invoke<McpReport>('mcp_set_disabled', { name, disabled }));
  }

  async function testServer(name: string) {
    busy = `test:${name}`;
    try {
      const res = await invoke<McpServerTestReport>('mcp_test_server', { name });
      if (res) {
        testNotes[name] = {
          ok: res.ok,
          text: res.ok
            ? L(`Healthy ✓ (${res.tools.length} tools · ${res.ms} ms)`, `سالم است ✓ (${num(res.tools.length)} ابزار · ${num(res.ms)} ms)`)
            : `✕ ${res.error ?? L('Connection failed', 'اتصال ناموفق')}`
        };
      }
    } catch (e) {
      testNotes[name] = { ok: false, text: `✕ ${String(e)}` };
    } finally {
      busy = '';
    }
  }

  async function diagnoseWithAi(name: string) {
    document.getElementById('mcp-setup-panel')?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    busy = `diagnose:${name}`;
    try {
      await invoke('mcp_setup_diagnose', { server: name });
    } catch (e) {
      flash(String(e));
    } finally {
      busy = '';
    }
  }

  async function toggleTool(s: McpServerStatus, toolName: string, enable: boolean) {
    const current = s.allTools.filter((t) => t.enabled).map((t) => t.name);
    let next: string[];
    if (enable) {
      next = Array.from(new Set([...current, toolName]));
    } else {
      next = current.filter((n) => n !== toolName);
      if (next.length === 0) {
        flash(L('Keep at least 1 tool enabled, or switch the server off.', 'حداقل ۱ ابزار باید روشن بماند؛ برای خاموش کردن کامل، کلید سرور را ببند.'));
        return;
      }
    }
    const payload = next.length >= s.allTools.length ? [] : next;
    await run(`tools:${s.name}`, () => invoke<McpReport>('mcp_set_server_tools', { name: s.name, tools: payload }));
  }

  async function enableAllTools(s: McpServerStatus) {
    await run(`tools:${s.name}`, () => invoke<McpReport>('mcp_set_server_tools', { name: s.name, tools: [] }));
  }

  async function installPrereq(kind: string) {
    installingPrereq = kind;
    try {
      const next = await invoke<McpReport>('mcp_install_prereq', { kind });
      if (next) report = next;
      flash(L(`${kind} installed ✓`, `${kind} با موفقیت نصب شد ✓`));
    } catch (e) {
      flash(String(e));
    } finally {
      installingPrereq = '';
    }
  }

  async function saveSecret(name: string, clear = false) {
    const value = clear ? '' : (secretInputs[name] ?? '');
    if (await run(`secret:${name}`, () => invoke<McpReport>('mcp_set_secret', { name, value }))) {
      secretInputs[name] = '';
      flash(
        clear
          ? L(`${name} removed`, `${name} حذف شد`)
          : L(`${name} saved in Credential Manager ✓ Press "Reconnect all" to use it`, `${name} در Credential Manager ذخیره شد ✓ — برای استفاده «اتصال دوباره» بزن`)
      );
    }
  }

  function filteredTools(s: McpServerStatus): McpToolInfo[] {
    const q = (toolSearch[s.name] ?? '').trim().toLowerCase();
    if (!q) return s.allTools;
    return s.allTools.filter((t) => t.name.toLowerCase().includes(q) || t.description.toLowerCase().includes(q));
  }

  onMount(() => {
    void invoke<McpReport>('mcp_report').then((r) => r && (report = r));
    let stop = () => {};
    void listen<McpReport>('mcp-changed', (r) => (report = r)).then((u) => (stop = u));
    return () => {
      stop();
      clearTimeout(noteTimer);
    };
  });
</script>

<h1>{L('Tools & MCP', 'ابزارها و MCP')}</h1>
<p class="lead">
  {L(
    'With MCP the assistant connects to Gmail, GitHub, Notion, browsers and other apps. Ask the AI installer below to connect anything in seconds.',
    'با MCP دستیار به Gmail، GitHub، Notion، مرورگر و برنامه‌های دیگر وصل می‌شود. همین پایین به دستیار بگو چه چیزی را وصل کند تا خودش برایت ردیف کند.'
  )}
</p>

<div class="summary">
  <div class="stat"><small>{L('Built-in tools', 'ابزارهای داخلی')}</small><b>{num(report?.builtin.length ?? 0)}</b></div>
  <div class="stat"><small>{L('Connected servers', 'سرورهای وصل')}</small><b>{num(servers.filter((s) => s.status === 'connected').length)}<span> / {num(servers.length)}</span></b></div>
  <div class="stat" class:warn={overToolLimit}>
    <small>{L('MCP tools', 'ابزارهای MCP')}</small>
    <b>{num(connectedTools)}<span> / {num(maxTools)}</span></b>
  </div>
</div>

{#if overToolLimit}
  <div class="warn-banner" transition:slide={{ duration: 200 }}>
    {L(
      `Only the first ${maxTools} active MCP tools are sent to the model (${connectedTools} active now). Uncheck tools you don't need inside each server card.`,
      `فقط ${num(maxTools)} ابزار اول به مدل هوش مصنوعی داده می‌شوند (الان ${num(connectedTools)} ابزار فعال است). داخل کارت سرورها، ابزارهای غیرضروری را تیک بردار.`
    )}
  </div>
{/if}

{#if report}
  <div class="env-bar">
    <span class="env-label">{L('Runtimes:', 'پیش‌نیازها:')}</span>
    <span class="env-pill" class:ok={report.env.npx}>
      <i class="env-dot" class:ok={report.env.npx}></i>
      Node / npx
    </span>
    {#if !report.env.npx}
      <button class="mini" disabled={installingPrereq !== '' || !inTauri} onclick={() => void installPrereq('npx')}>
        {installingPrereq === 'npx' ? L('Installing…', 'در حال نصب…') : L('Install Node', 'نصب Node')}
      </button>
    {/if}
    <span class="env-pill" class:ok={report.env.uvx}>
      <i class="env-dot" class:ok={report.env.uvx}></i>
      uv / uvx
    </span>
    {#if !report.env.uvx}
      <button class="mini" disabled={installingPrereq !== '' || !inTauri} onclick={() => void installPrereq('uvx')}>
        {installingPrereq === 'uvx' ? L('Installing…', 'در حال نصب…') : L('Install uv', 'نصب uv')}
      </button>
    {/if}
    <button class="mini refresh-env" onclick={() => run('env', () => invoke<McpReport>('mcp_report'))} disabled={busy !== '' || !inTauri}>
      {L('Recheck', 'بررسی مجدد')}
    </button>
  </div>
{/if}

<McpSetupChat
  env={report?.env}
  {installingPrereq}
  onupdated={(r) => (report = r)}
  oninstallprereq={installPrereq}
/>

{#if report?.error}
  <div class="result" transition:slide={{ duration: 200 }} dir="auto">✕ {report.error}</div>
{/if}

<div class="servers">
  {#each servers as s (s.name)}
    {@const exclusive = report?.exclusiveSecrets[s.name] ?? []}
    <div class="server" class:off={s.status === 'disabled'} in:fly={{ y: 10, duration: 300, easing: cubicOut }} out:scale={{ start: 0.95, duration: 180 }}>
      <span class="dot {s.status}" title={L(...STATUS[s.status])}></span>
      <button class="info" onclick={() => (openTools[s.name] = !openTools[s.name])}>
        <span class="name">
          {s.name}
          <em class={s.status}>
            {L(...STATUS[s.status])}{s.status === 'connected' ? L(` · ${s.tools.length} tools`, ` · ${num(s.tools.length)} ابزار`) : ''}{s.latencyMs != null && s.status === 'connected' ? ` · ${s.latencyMs}ms` : ''}
          </em>
        </span>
        <span class="meta" dir="ltr">{s.runs}</span>
      </button>
      <Toggle checked={s.status !== 'disabled'} onchange={(v) => setDisabled(s.name, !v)} label="enabled" />
      <div class="card-actions">
        <button class="mini" disabled={busy !== '' || s.status === 'disabled'} onclick={() => void testServer(s.name)}>
          {busy === `test:${s.name}` ? '…' : L('Test', 'تست')}
        </button>
        {#if s.allTools.length}
          <button class="mini" class:active-btn={openTools[s.name]} onclick={() => (openTools[s.name] = !openTools[s.name])}>
            {L('Tools', 'ابزارها')} ({num(s.tools.length)}/{num(s.allTools.length)})
          </button>
        {/if}
        <button class="mini" class:active-btn={openLogs[s.name]} onclick={() => (openLogs[s.name] = !openLogs[s.name])}>
          {L('Logs', 'لاگ')}
        </button>
        <button class="mini" onclick={() => startEdit(s.name)}>{L('Edit', 'ویرایش')}</button>
        {#if confirmDelete === s.name && exclusive.length}
          <button class="mini sure" onclick={() => void remove(s.name, false)} disabled={busy === `remove:${s.name}`}>
            {L('Server only', 'فقط سرور')}
          </button>
          <button class="mini sure" onclick={() => void remove(s.name, true)} disabled={busy === `remove:${s.name}`}>
            {L('+ Delete secrets', '+ حذف رمزها')}
          </button>
          <button class="mini" onclick={() => (confirmDelete = '')}>{L('Cancel', 'لغو')}</button>
        {:else}
          <button class="mini danger" class:sure={confirmDelete === s.name} onclick={() => void remove(s.name, false)} disabled={busy === `remove:${s.name}`}>
            {confirmDelete === s.name ? L('Sure?', 'مطمئنی؟') : L('Remove', 'حذف')}
          </button>
        {/if}
      </div>

      {#if testNotes[s.name]}
        <div class="test-banner" class:ok={testNotes[s.name].ok} dir="auto" transition:slide={{ duration: 180 }}>
          {testNotes[s.name].text}
        </div>
      {/if}

      {#if s.error}
        <div class="result error-row" dir="auto" transition:slide={{ duration: 200 }}>
          <span>✕ {s.error}</span>
          <button class="mini fix-btn" disabled={busy !== ''} onclick={() => void diagnoseWithAi(s.name)}>
            {busy === `diagnose:${s.name}` ? L('Checking…', 'در حال بررسی…') : L('Fix with AI', 'رفع مشکل با AI')}
          </button>
        </div>
      {/if}

      {#if openLogs[s.name]}
        <div class="logs-box" transition:slide={{ duration: 200 }}>
          <div class="panel-head">
            <b>{L('Server logs', 'لاگ سرور')}</b>
            <button class="mini" disabled={busy !== ''} onclick={() => void diagnoseWithAi(s.name)}>
              {L('Ask AI to inspect', 'بررسی با AI')}
            </button>
          </div>
          {#if s.logs.length}
            <pre dir="ltr">{s.logs.join('\n')}</pre>
          {:else}
            <small>{L('No log lines yet.', 'هنوز لاگی ثبت نشده.')}</small>
          {/if}
        </div>
      {/if}

      {#if openTools[s.name] && s.allTools.length}
        <div class="tools-panel" transition:slide={{ duration: 220 }}>
          <div class="panel-head">
            <div class="panel-title">
              <b>{L('Tools', 'ابزارها')} ({num(s.tools.length)} / {num(s.allTools.length)})</b>
              {#if s.allTools.length > 6}
                <input
                  class="search-tools"
                  dir="ltr"
                  placeholder={L('Filter tools…', 'فیلتر ابزارها…')}
                  value={toolSearch[s.name] ?? ''}
                  oninput={(e) => (toolSearch[s.name] = (e.currentTarget as HTMLInputElement).value)}
                />
              {/if}
            </div>
            {#if s.tools.length < s.allTools.length}
              <button class="mini" onclick={() => void enableAllTools(s)} disabled={busy !== ''}>{L('Enable all', 'فعال کردن همه')}</button>
            {/if}
          </div>

          <div class="tool-list">
            {#each filteredTools(s) as t (t.name)}
              <div class="tool-item" class:disabled-tool={!t.enabled}>
                <label class="tool-check">
                  <input type="checkbox" checked={t.enabled} disabled={busy !== ''} onchange={(e) => void toggleTool(s, t.name, (e.currentTarget as HTMLInputElement).checked)} />
                  <code dir="ltr">{t.name}</code>
                </label>
                {#if t.description}<span class="tool-desc" dir="auto">{t.description}</span>{/if}
                <button
                  class="mini"
                  disabled={s.status !== 'connected'}
                  onclick={() => (testingTool[s.name] = testingTool[s.name]?.name === t.name ? null : t)}
                >
                  {testingTool[s.name]?.name === t.name ? L('Hide test', 'بستن تست') : L('Test tool', 'تست')}
                </button>
              </div>
            {/each}
          </div>

          {#if testingTool[s.name]}
            <McpToolTester server={s.name} tool={testingTool[s.name]!} onclose={() => (testingTool[s.name] = null)} />
          {/if}
        </div>
      {/if}
    </div>
  {:else}
    {#if report}
      <div class="empty" in:fade>
        <b>{L('No server added yet', 'هنوز سروری اضافه نشده')}</b>
        <small>{L('Use the AI installer above, or click "Manual add" below.', 'از دستیار هوشمند بالا استفاده کن، یا دکمه «افزودن دستی» پایین را بزن.')}</small>
      </div>
    {/if}
  {/each}
</div>

{#if form}
  <div class="card form" transition:slide={{ duration: 320, easing: cubicOut }}>
    {#if !form.previous}
      <div class="presets">
        {#each PRESETS as p (p.id)}
          <button class="preset" class:on={preset === p.id} onclick={() => choosePreset(p)}>{L(...p.label)}</button>
        {/each}
      </div>
    {/if}
    <div class="grid">
      <label>
        <span>{L('Name', 'اسم')}</span>
        <input class="field mono" bind:value={form.name} maxlength="40" dir="ltr" spellcheck="false" placeholder="github" />
      </label>
      <div>
        <span>{L('Type', 'نوع')}</span>
        <div class="segmented">
          <button class:on={form.kind === 'command'} onclick={() => form && (form.kind = 'command')}>{L('Program', 'برنامه')}</button>
          <button class:on={form.kind === 'url'} onclick={() => form && (form.kind = 'url')}>URL</button>
        </div>
      </div>
      <label class="wide">
        <span>{form.kind === 'url' ? L('Server address', 'آدرس سرور') : L('Command to run', 'دستور اجرا')} <small>{L(...(PRESETS.find((p) => p.id === preset)?.hint ?? ['', '']))}</small></span>
        <input class="field mono" bind:value={form.line} dir="ltr" spellcheck="false" placeholder={form.kind === 'url' ? 'https://example.com/mcp' : 'npx -y @scope/server'} />
      </label>
      <label class="wide">
        <span>{form.kind === 'url' ? L('Headers', 'هدرها') : L('Environment variables', 'متغیرهای محیطی')} <small>{L('One KEY=VALUE per line · for secrets write secret:NAME', 'هر خط KEY=VALUE · برای رمزها بنویس secret:NAME')}</small></span>
        <textarea class="field mono" rows="3" bind:value={form.vars} dir="ltr" spellcheck="false" placeholder="TOKEN=secret:MY_TOKEN"></textarea>
      </label>
      <label class="wide">
        <span>{L('Only these tools', 'فقط این ابزارها')} <small>{L('Optional; separate with commas. Empty = all', 'اختیاری؛ با کاما جدا کن. خالی = همه')}</small></span>
        <input class="field mono" bind:value={form.only} dir="ltr" spellcheck="false" />
      </label>
    </div>
    {#if formError}
      <div class="result" dir="auto" transition:slide={{ duration: 200 }}>✕ {formError}</div>
    {/if}
    <div class="form-actions">
      <button class="btn ghost" onclick={() => (form = null)}>{L('Cancel', 'انصراف')}</button>
      <button class="btn primary" disabled={busy !== ''} onclick={saveForm}>
        {#if busy === 'save'}<span class="spin dark"></span>{L('Connecting…', 'اتصال…')}{:else}{L('Save and connect', 'ذخیره و اتصال')}{/if}
      </button>
    </div>
  </div>
{:else}
  <button class="add" onclick={startAdd} in:fly={{ y: 8, duration: 260 }}>
    <svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14" /></svg>
    {L('Manual add / custom server', 'افزودن دستی / سرور دلخواه')}
  </button>
{/if}

<div class="actions">
  <button class="mini" disabled={busy !== '' || !inTauri} onclick={() => run('reload', () => invoke<McpReport>('mcp_reload'))}>
    {busy === 'reload' ? L('Connecting…', 'در حال اتصال…') : L('Reconnect all', 'اتصال دوباره همه')}
  </button>
  <button class="mini" onclick={() => run('open', async () => (await invoke('mcp_open_file'), null))}>{L('Open the MCP file', 'باز کردن فایل MCP')}</button>
  {#if report}<code class="path" dir="ltr">{report.path}</code>{/if}
</div>
{#if note}<div class="note" dir="auto" transition:slide={{ duration: 200 }}>{note}</div>{/if}

{#if report?.secrets.length}
  <h2>{L('Secrets', 'رمزها')}</h2>
  <p class="lead">{L('Values written in the file as', 'مقدارهایی که در فایل به این شکل آمده‌اند:')} <code>secret:NAME</code>. {L('They are kept only in Windows Credential Manager.', 'فقط در Windows Credential Manager ذخیره می‌شوند.')}</p>
  <div class="card">
    {#each report.secrets as secret (secret.name)}
      <div class="row">
        <b dir="ltr">{secret.name} <span class="key" class:has={secret.set}>{secret.set ? L('saved', 'ذخیره شده') : L('none', 'ندارد')}</span></b>
        <div class="secret">
          <input class="field mono small-field" type="password" dir="ltr" autocomplete="off" placeholder={L('Value', 'مقدار')} bind:value={secretInputs[secret.name]} />
          <button class="mini" disabled={!secretInputs[secret.name]?.trim()} onclick={() => saveSecret(secret.name)}>{L('Save', 'ذخیره')}</button>
          {#if secret.set}<button class="mini danger" onclick={() => saveSecret(secret.name, true)}>{L('Remove', 'حذف')}</button>{/if}
        </div>
      </div>
    {/each}
  </div>
{/if}

{#if report?.builtin.length}
  <h2>{L('Built-in tools', 'ابزارهای داخلی')}</h2>
  <div class="chips builtin">
    {#each report.builtin as t (t)}<code>{t}</code>{/each}
  </div>
{/if}

<style>
  h1 {
    margin: 0 0 14px;
    font-size: 22px;
    font-weight: 800;
  }
  h2 {
    margin: 28px 0 10px;
    font-size: 16px;
    font-weight: 800;
  }
  .lead {
    margin: -6px 0 16px;
    color: var(--text-dim);
    font-size: 13px;
  }
  .lead code {
    font-family: var(--mono);
    font-size: 12px;
  }
  .summary {
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
  .stat.warn {
    border-color: rgba(251, 191, 36, 0.45);
  }
  .stat.warn b {
    color: #fde68a;
  }
  .stat small {
    display: block;
    color: var(--text-dim);
    font-size: 11.5px;
  }
  .stat b {
    font-size: 20px;
    font-weight: 800;
  }
  .stat b span {
    font-size: 12px;
    color: var(--text-dim);
  }
  .warn-banner {
    margin-bottom: 12px;
    padding: 9px 12px;
    border-radius: 12px;
    background: rgba(251, 191, 36, 0.1);
    border: 1px solid rgba(251, 191, 36, 0.25);
    color: #fde68a;
    font-size: 12px;
  }
  .env-bar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 12px;
    padding: 8px 12px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.06);
    font-size: 12px;
  }
  .env-label {
    color: var(--text-dim);
    font-weight: 600;
  }
  .env-pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 9px;
    border-radius: 999px;
    background: rgba(251, 191, 36, 0.08);
    color: #fde68a;
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .env-pill.ok {
    background: rgba(52, 211, 153, 0.1);
    color: #a7f3d0;
  }
  .env-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #fbbf24;
  }
  .env-dot.ok {
    background: #34d399;
  }
  .refresh-env {
    margin-inline-start: auto;
  }
  .servers {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-bottom: 12px;
  }
  .server {
    display: grid;
    grid-template-columns: auto 1fr auto auto;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
    transition: opacity var(--medium);
  }
  .server.off {
    opacity: 0.6;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--text-dim);
  }
  .dot.connected {
    background: #34d399;
    box-shadow: 0 0 10px rgba(52, 211, 153, 0.7);
  }
  .dot.connecting {
    background: #fbbf24;
    animation: pulse 1s ease-in-out infinite;
  }
  .dot.error {
    background: #f87171;
    box-shadow: 0 0 10px rgba(248, 113, 113, 0.6);
  }
  .info {
    display: flex;
    flex-direction: column;
    min-width: 0;
    border: none;
    background: none;
    color: inherit;
    text-align: start;
    cursor: pointer;
    padding: 0;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
    font-weight: 700;
    direction: ltr;
    justify-content: flex-start;
  }
  :global([dir='rtl']) .name {
    justify-content: flex-end;
  }
  .name em {
    font-style: normal;
    font-size: 10.5px;
    font-weight: 600;
    border-radius: 999px;
    padding: 1px 8px;
    color: var(--text-dim);
    background: rgba(255, 255, 255, 0.06);
    unicode-bidi: plaintext;
  }
  .name em.connected {
    color: #a7f3d0;
    background: rgba(52, 211, 153, 0.1);
  }
  .name em.error {
    color: #fecaca;
    background: rgba(248, 113, 113, 0.1);
  }
  .meta {
    margin-top: 2px;
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
  }
  :global([dir='rtl']) .meta {
    text-align: right;
  }
  .card-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .mini {
    padding: 5px 10px;
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
  .mini.active-btn {
    background: color-mix(in srgb, var(--accent-text) 22%, rgba(255, 255, 255, 0.08));
    color: var(--text);
  }
  .mini:active {
    transform: scale(0.96);
  }
  .mini:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .mini.danger:hover,
  .mini.sure {
    background: rgba(225, 29, 72, 0.85);
    color: #fff;
  }
  .fix-btn {
    background: rgba(255, 255, 255, 0.14);
    color: #fff;
    flex-shrink: 0;
  }
  .test-banner {
    grid-column: 1 / -1;
    padding: 7px 10px;
    border-radius: 9px;
    font-size: 12px;
    color: #fecaca;
    background: rgba(248, 113, 113, 0.09);
  }
  .test-banner.ok {
    color: #a7f3d0;
    background: rgba(52, 211, 153, 0.1);
  }
  .result {
    grid-column: 1 / -1;
    margin-bottom: 10px;
    padding: 8px 12px;
    border-radius: 10px;
    font-size: 12px;
    color: #fecaca;
    background: rgba(248, 113, 113, 0.08);
    word-break: break-word;
  }
  .server .result {
    margin-bottom: 0;
  }
  .error-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  .logs-box,
  .tools-panel {
    grid-column: 1 / -1;
    padding: 10px 12px;
    border-radius: 12px;
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid rgba(255, 255, 255, 0.06);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .panel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    flex-wrap: wrap;
  }
  .panel-title {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .panel-head b {
    font-size: 12.5px;
  }
  .search-tools {
    padding: 4px 9px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: rgba(0, 0, 0, 0.35);
    color: var(--text);
    font-size: 11.5px;
    outline: none;
    width: 170px;
  }
  .logs-box pre {
    margin: 0;
    max-height: 170px;
    overflow: auto;
    font-family: var(--mono);
    font-size: 11px;
    line-height: 1.5;
    color: var(--text-muted);
    white-space: pre-wrap;
    word-break: break-word;
    user-select: text;
  }
  .logs-box small {
    color: var(--text-dim);
    font-size: 12px;
  }
  .tool-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 280px;
    overflow-y: auto;
  }
  .tool-item {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 10px;
    padding: 6px 9px;
    border-radius: 9px;
    background: rgba(255, 255, 255, 0.03);
  }
  .tool-item.disabled-tool {
    opacity: 0.52;
  }
  .tool-check {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    cursor: pointer;
  }
  .tool-check code {
    font-family: var(--mono);
    font-size: 12px;
    color: var(--text);
  }
  .tool-desc {
    font-size: 11.5px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    direction: ltr;
  }
  .chips code {
    font-family: var(--mono);
    font-size: 11.5px;
    padding: 3px 9px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--text-muted);
  }
  :global([dir='rtl']) .builtin {
    justify-content: flex-end;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 24px 18px;
    border-radius: 16px;
    border: 1px dashed var(--border);
    text-align: center;
  }
  .empty small {
    color: var(--text-dim);
  }
  .add {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    width: 100%;
    padding: 13px;
    border-radius: 16px;
    border: 1px dashed rgba(255, 255, 255, 0.18);
    background: transparent;
    color: var(--text-muted);
    font-size: 13.5px;
    font-weight: 700;
    cursor: pointer;
    transition: border-color var(--fast), color var(--fast), background var(--fast), transform var(--medium) var(--ease-spring);
  }
  .add:hover {
    border-color: var(--accent-text);
    color: var(--text);
    background: var(--surface);
    transform: translateY(-1px);
  }
  .add svg {
    width: 18px;
    height: 18px;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    fill: none;
  }
  .card {
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
    padding: 6px 18px;
    margin-bottom: 16px;
  }
  .form {
    padding: 16px 18px;
    border-color: color-mix(in srgb, var(--accent-text) 30%, transparent);
  }
  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 14px;
  }
  .preset {
    padding: 6px 13px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all var(--fast);
  }
  .preset:hover {
    color: var(--text);
  }
  .preset.on {
    background: #fff;
    border-color: #fff;
    color: #000;
    box-shadow: 0 0 16px var(--glow-soft);
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px 14px;
  }
  .grid label,
  .grid > div {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-muted);
  }
  .grid small {
    color: var(--text-dim);
    font-weight: 400;
  }
  .wide {
    grid-column: 1 / -1;
  }
  .field {
    width: 100%;
    padding: 9px 12px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: rgba(0, 0, 0, 0.3);
    color: var(--text);
    font-size: 13px;
    outline: none;
    resize: vertical;
    transition: border-color var(--fast), box-shadow var(--fast);
  }
  .field:focus {
    border-color: var(--accent-text);
    box-shadow: 0 0 0 3px var(--glow-soft);
  }
  .mono {
    font-family: var(--mono);
    font-size: 12.5px;
  }
  .small-field {
    width: 200px;
  }
  .segmented {
    display: flex;
    padding: 3px;
    border-radius: 12px;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid var(--border);
  }
  .segmented button {
    flex: 1;
    padding: 7px 8px;
    border: none;
    border-radius: 9px;
    background: transparent;
    color: var(--text-dim);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--medium) var(--ease-out), color var(--fast);
  }
  .segmented button.on {
    background: rgba(255, 255, 255, 0.12);
    color: var(--text);
  }
  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 18px;
    border-radius: 10px;
    border: none;
    font-size: 13px;
    font-weight: 700;
    cursor: pointer;
    transition: transform var(--fast), opacity var(--fast);
  }
  .btn:active {
    transform: scale(0.97);
  }
  .btn:disabled {
    opacity: 0.5;
  }
  .primary {
    background: #fff;
    color: #000;
  }
  .ghost {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }
  .spin.dark {
    width: 13px;
    height: 13px;
    border-radius: 50%;
    border: 2px solid rgba(0, 0, 0, 0.2);
    border-top-color: #000;
    animation: spin 0.7s linear infinite;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 12px;
  }
  .path {
    margin-inline-start: auto;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 50%;
  }
  .note {
    padding: 10px 2px 0;
    font-size: 12px;
    color: var(--accent-text);
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
    font-size: 13px;
    font-family: var(--mono);
  }
  .key {
    font-family: var(--font);
    font-size: 11px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 999px;
    color: #fde68a;
    background: rgba(251, 191, 36, 0.1);
  }
  .key.has {
    color: #a7f3d0;
    background: rgba(52, 211, 153, 0.1);
  }
  .secret {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
</style>