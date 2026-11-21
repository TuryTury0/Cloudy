<script lang="ts">
  // AI-assisted MCP setup & troubleshooting chat inside Settings > Tools & MCP.
  // Chat history lives in the Rust backend so switching Settings tabs never loses progress,
  // and secrets are collected only through inline secure cards straight into Windows Credential Manager.
  import { onMount, tick } from 'svelte';
  import { fly, slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { L, renderMarkdown } from '@anytask/ui';
  import { inTauri, invoke, listen } from '../lib/ipc';
  import type {
    McpApplyOutcome,
    McpCatalogItem,
    McpEnvStatus,
    McpReport,
    McpSetupField,
    McpSetupMessage,
    McpSetupPlan,
    McpSetupState,
    McpUploadedFile
  } from '../lib/types';

  interface Props {
    env?: McpEnvStatus;
    installingPrereq?: string;
    onupdated: (report: McpReport) => void;
    oninstallprereq: (kind: string) => Promise<void>;
  }

  let { env, installingPrereq = '', onupdated, oninstallprereq }: Props = $props();

  let catalog = $state<McpCatalogItem[]>([]);
  let setup = $state<McpSetupState>({ messages: [], busy: false, progress: '' });
  let input = $state('');
  let collapsed = $state(false);
  let sending = $state(false);
  let chatError = $state('');
  let threadEl = $state<HTMLDivElement>();

  // Per-message card state (keyed by message id)
  let fieldValues = $state<Record<number, Record<string, string>>>({});
  let uploadedFiles = $state<Record<number, Record<string, McpUploadedFile>>>({});
  let showSecret = $state<Record<string, boolean>>({});
  let showDetails = $state<Record<number, boolean>>({});
  let applyingId = $state<number | null>(null);
  let applyErrors = $state<Record<number, string>>({});
  let appliedIds = $state<Record<number, boolean>>({});

  const QUICK_IDS = ['gmail', 'google-workspace', 'github', 'github-remote', 'notion', 'playwright', 'filesystem'];
  const quickItems = $derived(
    catalog.length
      ? QUICK_IDS.map((id) => catalog.find((c) => c.id === id)).filter((c): c is McpCatalogItem => Boolean(c))
      : []
  );

  async function scrollBottom() {
    await tick();
    if (threadEl) {
      threadEl.scrollTop = threadEl.scrollHeight;
    }
  }

  function isMissingReq(req: string, currentEnv?: McpEnvStatus): boolean {
    if (!currentEnv) return false;
    switch (req) {
      case 'node':
        return !currentEnv.node;
      case 'npx':
        return !currentEnv.npx;
      case 'uv':
        return !currentEnv.uv;
      case 'uvx':
        return !currentEnv.uvx;
      case 'python':
        return !currentEnv.python;
      case 'git':
        return !currentEnv.git;
      case 'docker':
        return !currentEnv.docker;
      default:
        return false;
    }
  }

  function liveMissing(plan: McpSetupPlan): string[] {
    if (!env) return plan.missingRequires;
    return plan.requires.filter((r) => isMissingReq(r, env));
  }

  function getFieldVal(msgId: number, field: McpSetupField): string {
    return fieldValues[msgId]?.[field.key] ?? field.defaultValue ?? '';
  }

  function setFieldVal(msgId: number, key: string, value: string) {
    if (!fieldValues[msgId]) fieldValues[msgId] = {};
    fieldValues[msgId][key] = value;
  }

  async function pickFile(msgId: number, fieldKey: string, event: Event) {
    const target = event.currentTarget as HTMLInputElement;
    const file = target.files?.[0];
    if (!file) return;
    const content = await file.text();
    if (!uploadedFiles[msgId]) uploadedFiles[msgId] = {};
    uploadedFiles[msgId][fieldKey] = { fieldKey, filename: file.name, content };
    target.value = '';
  }

  function clearFile(msgId: number, fieldKey: string) {
    if (uploadedFiles[msgId]) {
      delete uploadedFiles[msgId][fieldKey];
      uploadedFiles[msgId] = { ...uploadedFiles[msgId] };
    }
  }

  async function startPreset(presetId: string) {
    collapsed = false;
    chatError = '';
    try {
      const next = await invoke<McpSetupState>('mcp_setup_preset', { presetId });
      if (next) {
        setup = next;
        await scrollBottom();
      }
    } catch (e) {
      chatError = String(e);
    }
  }

  async function send() {
    const text = input.trim();
    if (!text || sending || setup.busy) return;
    input = '';
    chatError = '';
    sending = true;
    try {
      const next = await invoke<McpSetupState>('mcp_setup_chat', { text });
      if (next) {
        setup = next;
        await scrollBottom();
      }
    } catch (e) {
      chatError = String(e);
      input = text;
    } finally {
      sending = false;
    }
  }

  async function clearChat() {
    chatError = '';
    const next = await invoke<McpSetupState>('mcp_setup_clear');
    if (next) {
      setup = next;
      fieldValues = {};
      uploadedFiles = {};
      applyErrors = {};
      appliedIds = {};
    }
  }

  async function applyCard(msg: McpSetupMessage) {
    if (!msg.plan || applyingId !== null) return;
    const plan = msg.plan;
    applyErrors[msg.id] = '';
    applyingId = msg.id;

    const values: Record<string, string> = {};
    for (const field of plan.fields) {
      values[field.key] = getFieldVal(msg.id, field);
    }
    const files: McpUploadedFile[] = Object.values(uploadedFiles[msg.id] ?? {});

    try {
      const outcome = await invoke<McpApplyOutcome>('mcp_setup_apply', { plan, values, files });
      if (outcome) {
        setup = outcome.setup;
        onupdated(outcome.report);
        if (outcome.server?.status === 'connected') {
          appliedIds[msg.id] = true;
          // Clear raw secret inputs from memory after saving safely to Credential Manager.
          for (const f of plan.fields) {
            if (f.kind === 'secret') setFieldVal(msg.id, f.key, '');
          }
        }
        await scrollBottom();
      }
    } catch (e) {
      applyErrors[msg.id] = String(e);
    } finally {
      applyingId = null;
    }
  }

  function commandLine(plan: McpSetupPlan): string {
    if (plan.kind === 'url') return plan.url ?? '';
    return [plan.command ?? '', ...plan.args].filter(Boolean).join(' ');
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      void send();
    }
  }

  onMount(() => {
    void invoke<McpCatalogItem[]>('mcp_catalog').then((items) => items && (catalog = items));
    void invoke<McpSetupState>('mcp_setup_state').then(async (s) => {
      if (s) {
        setup = s;
        await scrollBottom();
      }
    });
    let stop = () => {};
    void listen<McpSetupState>('mcp-setup-changed', async (s) => {
      setup = s;
      collapsed = false;
      await scrollBottom();
    }).then((u) => (stop = u));
    return () => stop();
  });
</script>

<div id="mcp-setup-panel" class="card assistant-card">
  <div class="head">
    <button class="head-toggle" onclick={() => (collapsed = !collapsed)}>
      <span class="spark">
        <svg viewBox="0 0 24 24"><path d="M12 3l1.8 4.7L18.5 9.5l-4.7 1.8L12 16l-1.8-4.7L5.5 9.5l4.7-1.8zM18 15l.9 2.1L21 18l-2.1.9L18 21l-.9-2.1L15 18l2.1-.9z" /></svg>
      </span>
      <div class="head-text">
        <b>{L('AI MCP Installer', 'دستیار هوشمند نصب MCP')}</b>
        <small>{L('Tell the assistant what you want to connect — Gmail, Notion, GitHub or any MCP server.', 'بگو چی می‌خوای وصل کنی — جیمیل، نوشن، گیت‌هاب یا هر سرور MCP دیگه.')}</small>
      </div>
      <svg class="chev" class:open={!collapsed} viewBox="0 0 24 24"><path d="M6 9l6 6 6-6" /></svg>
    </button>
    {#if setup.messages.length}
      <button class="mini" onclick={clearChat} disabled={setup.busy || sending}>{L('New chat', 'شروع مجدد')}</button>
    {/if}
  </div>

  {#if !collapsed}
    <div class="body" transition:slide={{ duration: 240, easing: cubicOut }}>
      {#if quickItems.length}
        <div class="quick-row">
          <span class="quick-label">{L('Quick connect:', 'نصب سریع:')}</span>
          {#each quickItems as item (item.id)}
            <button class="chip-btn" onclick={() => startPreset(item.id)} disabled={setup.busy || sending} title={L(item.summaryEn, item.summaryFa)}>
              {L(item.titleEn, item.titleFa)}
            </button>
          {/each}
        </div>
      {/if}

      {#if setup.messages.length}
        <div class="thread" bind:this={threadEl}>
          {#each setup.messages as msg (msg.id)}
            <div class="msg {msg.role}" in:fly={{ y: 8, duration: 220, easing: cubicOut }}>
              {#if msg.steps.length}
                <div class="steps-line">
                  {#each msg.steps as step, idx (`${msg.id}-${idx}`)}
                    <span>✓ {step}</span>
                  {/each}
                </div>
              {/if}

              <div class="bubble" dir="auto">
                <div class="md">
                  <!-- eslint-disable-next-line svelte/no-at-html-tags -- renderMarkdown escapes all HTML first -->
                  {@html renderMarkdown(msg.text)}
                </div>
                {#if msg.error}
                  <div class="err-box" dir="auto">✕ {msg.error}</div>
                {/if}
              </div>

              {#if msg.plan}
                {@const plan = msg.plan}
                {@const missing = liveMissing(plan)}
                <div class="plan-card" class:installed={appliedIds[msg.id]} transition:slide={{ duration: 220 }}>
                  <div class="plan-head">
                    <div>
                      <b>{L(plan.titleEn, plan.titleFa)}</b>
                      <small>{L(plan.summaryEn, plan.summaryFa)}</small>
                    </div>
                    <button class="mini" onclick={() => (showDetails[msg.id] = !showDetails[msg.id])}>
                      {showDetails[msg.id] ? L('Hide command', 'پنهان کردن دستور') : L('Command', 'دستور')}
                    </button>
                  </div>

                  {#if showDetails[msg.id]}
                    <div class="cmd-box" dir="ltr" transition:slide={{ duration: 180 }}>
                      <code>{commandLine(plan)}</code>
                    </div>
                  {/if}

                  {#if missing.length}
                    <div class="prereq-warn">
                      <span>
                        {L(
                          `Missing required runtime: ${missing.join(', ')}`,
                          `پیش‌نیاز نصب نیست: ${missing.join('، ')}`
                        )}
                      </span>
                      <div class="prereq-actions">
                        {#each missing as req (req)}
                          {#if ['npx', 'node', 'uvx', 'uv'].includes(req)}
                            <button
                              class="mini install-btn"
                              disabled={installingPrereq !== ''}
                              onclick={() => void oninstallprereq(req)}
                            >
                              {installingPrereq === req
                                ? L(`Installing ${req}…`, `در حال نصب ${req}…`)
                                : L(`Install ${req} (winget)`, `نصب ${req} (winget)`)}
                            </button>
                          {/if}
                        {/each}
                      </div>
                    </div>
                  {/if}

                  {#if plan.fields.length}
                    <div class="fields">
                      {#each plan.fields as field (field.key)}
                        {@const secretToggleKey = `${msg.id}:${field.key}`}
                        {@const picked = uploadedFiles[msg.id]?.[field.key]}
                        <div class="field-item">
                          <div class="field-top">
                            <label for={`f-${msg.id}-${field.key}`}>
                              {L(field.labelEn, field.labelFa)}
                              {#if field.kind === 'secret' && field.alreadySet}
                                <span class="saved-pill">✓ {L('Saved in Credential Manager', 'در Credential Manager ذخیره است')}</span>
                              {/if}
                            </label>
                            {#if field.helpUrl}
                              <button class="link-btn" onclick={() => void invoke('open_external_url', { url: field.helpUrl })}>
                                {L('Open guide / link ↗', 'باز کردن صفحه دریافت ↗')}
                              </button>
                            {/if}
                          </div>

                          {#if L(field.helpEn, field.helpFa)}
                            <small class="field-help">{L(field.helpEn, field.helpFa)}</small>
                          {/if}

                          {#if field.kind === 'file_json'}
                            <div class="file-row">
                              <label class="file-pick">
                                <input type="file" accept=".json,application/json,*/*" onchange={(e) => void pickFile(msg.id, field.key, e)} />
                                {picked ? `✓ ${picked.filename}` : L('Choose JSON file…', 'انتخاب فایل JSON…')}
                              </label>
                              {#if picked}
                                <button class="mini" onclick={() => clearFile(msg.id, field.key)}>✕</button>
                              {:else}
                                <input
                                  id={`f-${msg.id}-${field.key}`}
                                  class="input mono"
                                  dir="ltr"
                                  placeholder={field.placeholder ?? L('or paste file path', 'یا مسیر فایل را بچسبان')}
                                  value={getFieldVal(msg.id, field)}
                                  oninput={(e) => setFieldVal(msg.id, field.key, (e.currentTarget as HTMLInputElement).value)}
                                />
                              {/if}
                            </div>
                          {:else if field.kind === 'secret'}
                            <div class="secret-row">
                              <input
                                id={`f-${msg.id}-${field.key}`}
                                class="input mono"
                                dir="ltr"
                                autocomplete="off"
                                type={showSecret[secretToggleKey] ? 'text' : 'password'}
                                placeholder={field.alreadySet ? L('Leave empty to keep saved secret', 'خالی بگذار تا رمز فعلی بماند') : (field.placeholder ?? '')}
                                value={getFieldVal(msg.id, field)}
                                oninput={(e) => setFieldVal(msg.id, field.key, (e.currentTarget as HTMLInputElement).value)}
                              />
                              <button class="mini" onclick={() => (showSecret[secretToggleKey] = !showSecret[secretToggleKey])}>
                                {showSecret[secretToggleKey] ? L('Hide', 'مخفی') : L('Show', 'نمایش')}
                              </button>
                            </div>
                          {:else}
                            <input
                              id={`f-${msg.id}-${field.key}`}
                              class="input mono"
                              dir="ltr"
                              type="text"
                              placeholder={field.placeholder ?? ''}
                              value={getFieldVal(msg.id, field)}
                              oninput={(e) => setFieldVal(msg.id, field.key, (e.currentTarget as HTMLInputElement).value)}
                            />
                          {/if}
                        </div>
                      {/each}
                    </div>
                  {/if}

                  {#if applyErrors[msg.id]}
                    <div class="err-box" dir="auto">✕ {applyErrors[msg.id]}</div>
                  {/if}

                  <div class="plan-foot">
                    <span class="sec-note">
                      {#if plan.fields.some((f) => f.kind === 'secret')}
                        {L('Secrets go straight to Windows Credential Manager (never sent to AI).', 'رمزها مستقیم به Windows Credential Manager می‌روند (برای AI فرستاده نمی‌شوند).')}
                      {:else}
                        {L('Server name:', 'اسم سرور:')} <code dir="ltr">{plan.serverName}</code>
                      {/if}
                    </span>
                    <button class="btn primary" disabled={applyingId !== null || !inTauri} onclick={() => void applyCard(msg)}>
                      {#if applyingId === msg.id}
                        <span class="spin dark"></span>{L('Connecting…', 'در حال نصب و اتصال…')}
                      {:else if appliedIds[msg.id]}
                        ✓ {L('Connected — Reapply', 'وصل شد — اعمال دوباره')}
                      {:else}
                        {L('Install & connect', 'نصب و اتصال')}
                      {/if}
                    </button>
                  </div>
                </div>
              {/if}
            </div>
          {/each}

          {#if setup.busy || sending}
            <div class="msg assistant" in:fly={{ y: 6, duration: 200 }}>
              <div class="progress-pill">
                <span class="spin"></span>
                <span>{setup.progress || L('Working…', 'در حال انجام…')}</span>
              </div>
            </div>
          {/if}
        </div>
      {/if}

      {#if chatError}
        <div class="err-box" dir="auto">✕ {chatError}</div>
      {/if}

      <div class="composer">
        <textarea
          class="chat-input"
          rows="2"
          dir="auto"
          bind:value={input}
          onkeydown={onKeydown}
          disabled={setup.busy || sending || !inTauri}
          placeholder={L(
            'Example: "Connect my Gmail", "Install Slack MCP", or "Test gmail"',
            'مثلاً بنویس: «جیمیلم رو وصل کن»، «نوشن رو نصب کن»، یا «جیمیل رو تست کن»'
          )}
        ></textarea>
        <button class="btn primary send-btn" disabled={!input.trim() || setup.busy || sending || !inTauri} onclick={() => void send()}>
          {L('Send', 'ارسال')}
        </button>
      </div>
    </div>
  {/if}
</div>

<style>
  .assistant-card {
    padding: 12px 16px;
    border-radius: 18px;
    background: var(--surface);
    border: 1px solid color-mix(in srgb, var(--accent-text) 24%, rgba(255, 255, 255, 0.08));
    margin-bottom: 16px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .head-toggle {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 2px 0;
    border: none;
    background: none;
    color: inherit;
    text-align: start;
    cursor: pointer;
  }
  .spark {
    flex-shrink: 0;
    width: 34px;
    height: 34px;
    display: grid;
    place-items: center;
    border-radius: 11px;
    background: color-mix(in srgb, var(--accent-text) 16%, rgba(255, 255, 255, 0.05));
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.2);
  }
  .spark svg {
    width: 18px;
    height: 18px;
    fill: none;
    stroke: var(--accent-text);
    stroke-width: 1.8;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .head-text {
    flex: 1;
    min-width: 0;
  }
  .head-text b {
    display: block;
    font-size: 14px;
    font-weight: 800;
  }
  .head-text small {
    display: block;
    color: var(--text-dim);
    font-size: 12px;
    margin-top: 2px;
  }
  .chev {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: var(--text-dim);
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
    transition: transform var(--medium) var(--ease-spring);
  }
  .chev.open {
    transform: rotate(180deg);
  }
  .body {
    margin-top: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .quick-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .quick-label {
    font-size: 11.5px;
    color: var(--text-dim);
    margin-inline-end: 2px;
  }
  .chip-btn {
    padding: 5px 11px;
    border-radius: 999px;
    border: 1px solid rgba(255, 255, 255, 0.12);
    background: rgba(255, 255, 255, 0.04);
    color: var(--text-muted);
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
    transition: all var(--fast);
  }
  .chip-btn:hover:not(:disabled) {
    border-color: var(--accent-text);
    color: var(--text);
    background: color-mix(in srgb, var(--accent-text) 14%, transparent);
  }
  .chip-btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .thread {
    max-height: 430px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 10px;
    border-radius: 14px;
    background: rgba(0, 0, 0, 0.24);
    border: 1px solid rgba(255, 255, 255, 0.05);
  }
  .msg {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: 94%;
  }
  .msg.user {
    align-self: flex-end;
    align-items: flex-end;
  }
  .msg.assistant {
    align-self: flex-start;
    align-items: flex-start;
    width: 100%;
  }
  .steps-line {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    font-size: 11px;
    color: var(--text-dim);
  }
  .steps-line span {
    padding: 2px 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.05);
  }
  .bubble {
    padding: 9px 13px;
    border-radius: 14px;
    font-size: 13px;
    line-height: 1.65;
    user-select: text;
  }
  .user .bubble {
    background: color-mix(in srgb, var(--accent) 26%, rgba(28, 34, 54, 0.8));
    border: 1px solid rgba(255, 255, 255, 0.14);
  }
  .assistant .bubble {
    background: rgba(255, 255, 255, 0.055);
    border: 1px solid rgba(255, 255, 255, 0.07);
  }
  .md :global(p) {
    margin: 0 0 6px;
  }
  .md :global(p:last-child) {
    margin-bottom: 0;
  }
  .md :global(ul),
  .md :global(ol) {
    margin: 2px 0 6px;
    padding-inline-start: 18px;
  }
  .md :global(code) {
    font-family: var(--mono);
    font-size: 11.5px;
    padding: 1px 5px;
    border-radius: 5px;
    background: rgba(0, 0, 0, 0.35);
    direction: ltr;
    unicode-bidi: isolate;
  }
  .plan-card {
    width: 100%;
    padding: 13px 14px;
    border-radius: 14px;
    background: rgba(14, 18, 30, 0.72);
    border: 1px solid color-mix(in srgb, var(--accent-text) 32%, rgba(255, 255, 255, 0.1));
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .plan-card.installed {
    border-color: rgba(52, 211, 153, 0.45);
  }
  .plan-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
  }
  .plan-head b {
    display: block;
    font-size: 13.5px;
  }
  .plan-head small {
    display: block;
    margin-top: 2px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .cmd-box {
    padding: 7px 10px;
    border-radius: 9px;
    background: rgba(0, 0, 0, 0.42);
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--text-muted);
    overflow-x: auto;
  }
  .prereq-warn {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 10px;
    background: rgba(251, 191, 36, 0.1);
    border: 1px solid rgba(251, 191, 36, 0.25);
    color: #fde68a;
    font-size: 12px;
  }
  .prereq-actions {
    display: flex;
    gap: 6px;
  }
  .install-btn {
    background: rgba(251, 191, 36, 0.22);
    color: #fef3c7;
  }
  .fields {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .field-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .field-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    flex-wrap: wrap;
  }
  .field-top label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    font-weight: 700;
    color: var(--text-muted);
  }
  .saved-pill {
    font-size: 10.5px;
    font-weight: 600;
    padding: 1px 7px;
    border-radius: 999px;
    color: #a7f3d0;
    background: rgba(52, 211, 153, 0.12);
  }
  .link-btn {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent-text);
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
  }
  .link-btn:hover {
    text-decoration: underline;
  }
  .field-help {
    font-size: 11.5px;
    color: var(--text-dim);
    line-height: 1.45;
  }
  .secret-row,
  .file-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .input {
    width: 100%;
    padding: 8px 11px;
    border-radius: 9px;
    border: 1px solid var(--border);
    background: rgba(0, 0, 0, 0.35);
    color: var(--text);
    font-size: 12.5px;
    outline: none;
  }
  .input:focus {
    border-color: var(--accent-text);
  }
  .mono {
    font-family: var(--mono);
  }
  .file-pick {
    position: relative;
    display: inline-flex;
    align-items: center;
    padding: 7px 12px;
    border-radius: 9px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }
  .file-pick input {
    position: absolute;
    inset: 0;
    opacity: 0;
    cursor: pointer;
  }
  .plan-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    flex-wrap: wrap;
    padding-top: 4px;
  }
  .sec-note {
    font-size: 11px;
    color: var(--text-dim);
  }
  .sec-note code {
    font-family: var(--mono);
  }
  .progress-pill {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.06);
    font-size: 12px;
    color: var(--text-muted);
  }
  .composer {
    display: flex;
    align-items: flex-end;
    gap: 8px;
  }
  .chat-input {
    flex: 1;
    padding: 9px 12px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: rgba(0, 0, 0, 0.32);
    color: var(--text);
    font-size: 13px;
    resize: none;
    outline: none;
    transition: border-color var(--fast), box-shadow var(--fast);
  }
  .chat-input:focus {
    border-color: var(--accent-text);
    box-shadow: 0 0 0 3px var(--glow-soft);
  }
  .send-btn {
    height: 40px;
  }
  .err-box {
    padding: 7px 10px;
    border-radius: 9px;
    font-size: 12px;
    color: #fecaca;
    background: rgba(248, 113, 113, 0.1);
    word-break: break-word;
  }
  .mini {
    padding: 5px 10px;
    border: none;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-muted);
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }
  .mini:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.15);
    color: var(--text);
  }
  .mini:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 8px 16px;
    border-radius: 10px;
    border: none;
    font-size: 12.5px;
    font-weight: 700;
    cursor: pointer;
    white-space: nowrap;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .primary {
    background: #fff;
    color: #000;
  }
  .spin {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.25);
    border-top-color: #fff;
    animation: spin 0.7s linear infinite;
  }
  .spin.dark {
    border-color: rgba(0, 0, 0, 0.2);
    border-top-color: #000;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>