<script lang="ts">
  // Live tester for a single MCP tool: builds inputs from its JSON Schema (or raw JSON) and shows output + timing.
  import { slide } from 'svelte/transition';
  import { L, num } from '@anytask/ui';
  import { invoke } from '../lib/ipc';
  import type { McpToolInfo, McpToolTestReport } from '../lib/types';

  interface Props {
    server: string;
    tool: McpToolInfo;
    onclose: () => void;
  }

  let { server, tool, onclose }: Props = $props();

  interface PropField {
    key: string;
    kind: 'string' | 'number' | 'boolean' | 'enum' | 'json';
    required: boolean;
    description: string;
    options?: string[];
  }

  function asRecord(v: unknown): Record<string, unknown> {
    return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
  }

  function schemaFields(schema: Record<string, unknown>): PropField[] {
    const props = asRecord(schema.properties);
    const reqList = Array.isArray(schema.required) ? schema.required.filter((r): r is string => typeof r === 'string') : [];
    return Object.entries(props).map(([key, raw]) => {
      const def = asRecord(raw);
      const rawType = Array.isArray(def.type) ? def.type.find((t) => t !== 'null') : def.type;
      const enums = Array.isArray(def.enum) ? def.enum.map((e) => String(e)) : undefined;
      const kind: PropField['kind'] = enums?.length
        ? 'enum'
        : rawType === 'boolean'
          ? 'boolean'
          : rawType === 'integer' || rawType === 'number'
            ? 'number'
            : rawType === 'object' || rawType === 'array'
              ? 'json'
              : 'string';
      return {
        key,
        kind,
        required: reqList.includes(key),
        description: typeof def.description === 'string' ? def.description : '',
        options: enums
      };
    });
  }

  const fields = $derived(schemaFields(tool.schema ?? {}));

  let rawMode = $state(false);
  let rawJson = $state('{}');
  let values = $state<Record<string, string | boolean>>({});
  let busy = $state(false);
  let parseError = $state('');
  let report = $state<McpToolTestReport | null>(null);
  let copied = $state(false);

  // Reset form state whenever the user switches to a different tool.
  $effect(() => {
    const _key = `${server}:${tool.name}`;
    void _key;
    const init: Record<string, string | boolean> = {};
    for (const f of schemaFields(tool.schema ?? {})) {
      init[f.key] = f.kind === 'boolean' ? false : f.kind === 'enum' && f.options?.[0] ? f.options[0] : '';
    }
    values = init;
    rawJson = '{}';
    rawMode = schemaFields(tool.schema ?? {}).length === 0;
    report = null;
    parseError = '';
  });

  function buildArgs(): Record<string, unknown> {
    if (rawMode) {
      const parsed: unknown = JSON.parse(rawJson.trim() || '{}');
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        throw new Error(L('Arguments must be a JSON object.', 'آرگومان‌ها باید یک آبجکت JSON باشند.'));
      }
      return parsed as Record<string, unknown>;
    }
    const out: Record<string, unknown> = {};
    for (const f of fields) {
      const v = values[f.key];
      if (f.kind === 'boolean') {
        out[f.key] = Boolean(v);
        continue;
      }
      const str = typeof v === 'string' ? v.trim() : '';
      if (!str) continue;
      if (f.kind === 'number') {
        const n = Number(str);
        if (Number.isNaN(n)) throw new Error(`${f.key}: ${L('must be a number', 'باید عدد باشد')}`);
        out[f.key] = n;
      } else if (f.kind === 'json') {
        out[f.key] = JSON.parse(str);
      } else {
        out[f.key] = str;
      }
    }
    return out;
  }

  async function runTool() {
    parseError = '';
    let args: Record<string, unknown>;
    try {
      args = buildArgs();
    } catch (e) {
      parseError = String(e);
      return;
    }
    busy = true;
    try {
      const res = await invoke<McpToolTestReport>('mcp_test_tool', { server, tool: tool.name, args });
      if (res) report = res;
    } catch (e) {
      parseError = String(e);
    } finally {
      busy = false;
    }
  }

  function copyOut() {
    if (!report) return;
    void invoke('copy_text', { text: report.ok ? report.text : (report.error ?? '') });
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

<div class="tester" transition:slide={{ duration: 200 }}>
  <div class="tester-head">
    <div class="title">
      <code dir="ltr">{tool.name}</code>
      {#if tool.description}<small>{tool.description}</small>{/if}
    </div>
    <div class="head-btns">
      <button class="mini" onclick={() => (rawMode = !rawMode)}>
        {rawMode ? L('Form', 'فرم') : 'JSON'}
      </button>
      <button class="mini" onclick={onclose}>{L('Close', 'بستن')}</button>
    </div>
  </div>

  {#if rawMode}
    <textarea class="field mono" rows="4" dir="ltr" bind:value={rawJson} spellcheck="false" placeholder={'{"key": "value"}'}></textarea>
  {:else}
    <div class="form-grid">
      {#each fields as f (f.key)}
        <div class="f-row">
          <label for={`t-${tool.name}-${f.key}`} dir="ltr">
            <code>{f.key}</code>
            {#if f.required}<span class="req">*</span>{/if}
          </label>
          {#if f.description}<small>{f.description}</small>{/if}
          {#if f.kind === 'boolean'}
            <label class="chk">
              <input id={`t-${tool.name}-${f.key}`} type="checkbox" checked={Boolean(values[f.key])} onchange={(e) => (values[f.key] = (e.currentTarget as HTMLInputElement).checked)} />
              <span>{values[f.key] ? 'true' : 'false'}</span>
            </label>
          {:else if f.kind === 'enum' && f.options}
            <select id={`t-${tool.name}-${f.key}`} class="field mono" dir="ltr" value={String(values[f.key] ?? '')} onchange={(e) => (values[f.key] = (e.currentTarget as HTMLSelectElement).value)}>
              {#each f.options as opt (opt)}<option value={opt}>{opt}</option>{/each}
            </select>
          {:else if f.kind === 'json'}
            <textarea
              id={`t-${tool.name}-${f.key}`}
              class="field mono"
              rows="2"
              dir="ltr"
              placeholder={'{} or []'}
              value={String(values[f.key] ?? '')}
              oninput={(e) => (values[f.key] = (e.currentTarget as HTMLTextAreaElement).value)}
            ></textarea>
          {:else}
            <input
              id={`t-${tool.name}-${f.key}`}
              class="field mono"
              dir="ltr"
              type={f.kind === 'number' ? 'number' : 'text'}
              value={String(values[f.key] ?? '')}
              oninput={(e) => (values[f.key] = (e.currentTarget as HTMLInputElement).value)}
            />
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  {#if parseError}
    <div class="err" dir="auto">✕ {parseError}</div>
  {/if}

  <div class="run-bar">
    <button class="btn primary" disabled={busy} onclick={() => void runTool()}>
      {#if busy}<span class="spin dark"></span>{L('Running…', 'در حال اجرا…')}{:else}{L('Run test', 'اجرای تست')}{/if}
    </button>
    {#if report}
      <span class="badge" class:ok={report.ok} class:bad={!report.ok}>
        {report.ok ? L('Success', 'موفق') : L('Failed', 'خطا')} · {num(report.ms)} ms
      </span>
      <button class="mini" onclick={copyOut}>{copied ? L('Copied ✓', 'کپی شد ✓') : L('Copy output', 'کپی خروجی')}</button>
    {/if}
  </div>

  {#if report}
    <pre class="output" class:bad={!report.ok} dir="auto">{report.ok ? (report.text || L('(empty output)', '(خروجی خالی)')) : (report.error ?? '')}</pre>
  {/if}
</div>

<style>
  .tester {
    grid-column: 1 / -1;
    margin-top: 6px;
    padding: 12px;
    border-radius: 12px;
    background: rgba(0, 0, 0, 0.34);
    border: 1px solid rgba(255, 255, 255, 0.09);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .tester-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
  }
  .title code {
    font-family: var(--mono);
    font-size: 12.5px;
    font-weight: 700;
    color: var(--accent-text);
  }
  .title small {
    display: block;
    margin-top: 2px;
    font-size: 11.5px;
    color: var(--text-dim);
  }
  .head-btns {
    display: flex;
    gap: 6px;
  }
  .form-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 10px;
  }
  .f-row {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .f-row label {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
  }
  .f-row code {
    font-family: var(--mono);
    font-size: 12px;
  }
  .f-row small {
    font-size: 11px;
    color: var(--text-dim);
  }
  .req {
    color: #fca5a5;
  }
  .chk {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-family: var(--mono);
    font-size: 12px;
    color: var(--text-muted);
  }
  .field {
    width: 100%;
    padding: 7px 10px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: rgba(0, 0, 0, 0.36);
    color: var(--text);
    font-size: 12px;
    outline: none;
  }
  .field:focus {
    border-color: var(--accent-text);
  }
  .mono {
    font-family: var(--mono);
  }
  .run-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 6px 14px;
    border-radius: 9px;
    border: none;
    font-size: 12px;
    font-weight: 700;
    cursor: pointer;
  }
  .btn:disabled {
    opacity: 0.5;
  }
  .primary {
    background: #fff;
    color: #000;
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
  }
  .mini:hover {
    background: rgba(255, 255, 255, 0.14);
    color: var(--text);
  }
  .badge {
    font-size: 11px;
    font-weight: 700;
    padding: 3px 9px;
    border-radius: 999px;
  }
  .badge.ok {
    color: #a7f3d0;
    background: rgba(52, 211, 153, 0.14);
  }
  .badge.bad {
    color: #fecaca;
    background: rgba(248, 113, 113, 0.14);
  }
  .output {
    margin: 0;
    padding: 10px;
    max-height: 220px;
    overflow: auto;
    border-radius: 9px;
    background: rgba(0, 0, 0, 0.45);
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    user-select: text;
    color: var(--text-muted);
  }
  .output.bad {
    color: #fecaca;
  }
  .err {
    font-size: 12px;
    color: #fecaca;
  }
  .spin.dark {
    width: 11px;
    height: 11px;
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