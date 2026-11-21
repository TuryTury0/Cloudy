<script lang="ts">
  // Settings > Models: add a model from a preset, paste its key (kept in the OS keychain only), test it, and pick
  // which one answers. Also the optional search API keys.
  import { onMount } from 'svelte';
  import { fade, fly, scale, slide } from 'svelte/transition';
  import { backOut, cubicOut } from 'svelte/easing';
  import { L } from '@anytask/ui';
  import { inTauri, invoke } from '../lib/ipc';
  import type { ModelConfig, ModelTest, Provider, ProxySettings, SearchKeys, Settings, Thinking } from '../lib/types';
  import Toggle from './Toggle.svelte';
  import DecisionPanel from './DecisionPanel.svelte';

  interface Props {
    settings: Settings;
    onsettings: (next: Settings) => void;
  }

  let { settings, onsettings }: Props = $props();

  interface Preset {
    id: string;
    label: string;
    provider: Provider;
    baseUrl: string;
    examples: string[];
    keyHint: string;
    /** Persian label, where it differs from `label`. */
    fa?: string;
  }

  const PRESETS: Preset[] = [
    { id: 'gemini', label: 'Gemini', provider: 'gemini', baseUrl: 'https://generativelanguage.googleapis.com/v1beta', examples: ['gemini-3.8-flash', 'gemini-2.5-flash', 'gemini-2.5-pro'], keyHint: 'aistudio.google.com' },
    { id: 'openai', label: 'OpenAI', provider: 'openai', baseUrl: 'https://api.openai.com/v1', examples: ['gpt-5-mini', 'gpt-5'], keyHint: 'platform.openai.com' },
    { id: 'openrouter', label: 'OpenRouter', provider: 'openai', baseUrl: 'https://openrouter.ai/api/v1', examples: ['google/gemini-2.5-flash', 'openai/gpt-5-mini'], keyHint: 'openrouter.ai' },
    { id: 'gapgpt', label: 'GapGPT', provider: 'openai', baseUrl: 'https://api.gapgpt.app/v1', examples: ['gpt-5-mini', 'gemini-2.5-flash'], keyHint: 'gapgpt.app' },
    { id: 'local', label: 'Ollama (local)', fa: 'Ollama (محلی)', provider: 'openai', baseUrl: 'http://localhost:11434/v1', examples: ['qwen3:8b', 'llama3.1'], keyHint: 'any text' },
    { id: 'custom', label: 'Custom', fa: 'دلخواه', provider: 'openai', baseUrl: '', examples: [], keyHint: '' }
  ];

  const THINKING: Array<{ id: Thinking; en: string; fa: string }> = [
    { id: 'default', en: 'Short (recommended)', fa: 'کوتاه (پیشنهادی)' },
    { id: 'low', en: 'None', fa: 'بدون فکر' },
    { id: 'medium', en: 'Medium', fa: 'متوسط' },
    { id: 'high', en: 'High', fa: 'زیاد' }
  ];

  const SEARCH_PROVIDERS: Array<{ id: 'tavily' | 'brave'; label: string; hint: string }> = [
    { id: 'tavily', label: 'Tavily', hint: 'tavily.com' },
    { id: 'brave', label: 'Brave Search', hint: 'brave.com/search/api' }
  ];

  type Result = { ok: boolean; text: string };

  let keys = $state<Record<string, boolean>>({});
  let search = $state<SearchKeys>({ tavily: false, brave: false });
  let form = $state<ModelConfig | null>(null);
  let preset = $state('gemini');
  let keyInput = $state('');
  let showKey = $state(false);
  let busy = $state<'' | 'save' | 'test'>('');
  let formResult = $state<Result | null>(null);
  let cardResults = $state<Record<string, Result | 'testing'>>({});
  let confirmDelete = $state('');
  let searchInputs = $state({ tavily: '', brave: '' });
  let searchNote = $state('');

  const currentPreset = $derived(PRESETS.find((p) => p.id === preset) ?? PRESETS[0]);
  const activeId = $derived(settings.models.some((m) => m.id === settings.activeModel) ? settings.activeModel : settings.models[0]?.id);

  async function refreshKeys() {
    keys = (await invoke<Record<string, boolean>>('model_keys')) ?? {};
  }

  function host(url: string) {
    try {
      return new URL(url).host;
    } catch {
      return url;
    }
  }

  function startAdd() {
    preset = 'gemini';
    form = { id: '', name: 'Gemini', provider: 'gemini', baseUrl: PRESETS[0].baseUrl, modelId: PRESETS[0].examples[0], thinking: 'default' };
    keyInput = '';
    formResult = null;
  }

  function startEdit(model: ModelConfig) {
    preset = PRESETS.find((p) => p.baseUrl && model.baseUrl.startsWith(p.baseUrl))?.id ?? 'custom';
    form = { ...model };
    keyInput = '';
    formResult = null;
  }

  function choosePreset(p: Preset) {
    if (!form) return;
    preset = p.id;
    form.provider = p.provider;
    form.baseUrl = p.baseUrl;
    if (p.examples[0]) form.modelId = p.examples[0];
    if (!form.id || PRESETS.some((x) => x.label === form!.name || x.fa === form!.name)) form.name = L(p.label, p.fa ?? p.label);
    formResult = null;
  }

  async function testForm() {
    if (!form) return;
    busy = 'test';
    formResult = null;
    try {
      const r = await invoke<ModelTest>('test_model', { model: $state.snapshot(form), apiKey: keyInput || null });
      formResult = r
        ? { ok: true, text: L(`Connected in ${r.ms} ms: "${r.reply}"`, `وصل شد در ${r.ms} میلی‌ثانیه — «${r.reply}»`) }
        : { ok: true, text: L('Preview mode', 'حالت پیش‌نمایش') };
    } catch (e) {
      formResult = { ok: false, text: String(e) };
    } finally {
      busy = '';
    }
  }

  async function saveForm() {
    if (!form) return;
    busy = 'save';
    formResult = null;
    try {
      const next = await invoke<Settings>('save_model', { model: $state.snapshot(form), apiKey: keyInput || null });
      if (next) onsettings(next);
      else if (!inTauri) onsettings({ ...settings, models: [...settings.models, { ...$state.snapshot(form), id: `m${Date.now()}` }] });
      form = null;
      keyInput = '';
      await refreshKeys();
    } catch (e) {
      formResult = { ok: false, text: String(e) };
    } finally {
      busy = '';
    }
  }

  async function testCard(model: ModelConfig) {
    cardResults[model.id] = 'testing';
    try {
      const r = await invoke<ModelTest>('test_model', { model, apiKey: null });
      cardResults[model.id] = { ok: true, text: r ? `${r.ms} ms — «${r.reply}»` : 'OK' };
    } catch (e) {
      cardResults[model.id] = { ok: false, text: String(e) };
    }
  }

  async function makeActive(id: string) {
    if (id === activeId) return;
    try {
      const next = await invoke<Settings>('set_active_model', { id });
      onsettings(next ?? { ...settings, activeModel: id });
    } catch (e) {
      cardResults[id] = { ok: false, text: String(e) };
    }
  }

  // Proxy (VPN): the address is saved when the field is left; switches save at once.
  const proxy = $derived<ProxySettings>(settings.proxy ?? { address: '127.0.0.1:10808', models: [], voice: false, web: true });
  let proxyAddress = $state('');
  let proxyNote = $state('');
  $effect(() => {
    proxyAddress = proxy.address;
  });

  async function saveProxy(change: Partial<ProxySettings>) {
    const next = { ...proxy, ...change };
    try {
      const saved = await invoke<Settings>('set_proxy', { proxy: next });
      onsettings(saved ?? { ...settings, proxy: next });
      proxyNote = '';
    } catch (e) {
      proxyNote = String(e);
    }
  }

  function proxyForModel(id: string, on: boolean) {
    const models = proxy.models.filter((m) => m !== id);
    void saveProxy({ models: on ? [...models, id] : models });
  }

  async function setBackup(id: string) {
    try {
      const next = await invoke<Settings>('set_backup_model', { id });
      onsettings(next ?? { ...settings, backupModel: id });
    } catch (e) {
      cardResults[id] = { ok: false, text: String(e) };
    }
  }

  async function remove(id: string) {
    if (confirmDelete !== id) {
      confirmDelete = id;
      setTimeout(() => confirmDelete === id && (confirmDelete = ''), 3000);
      return;
    }
    confirmDelete = '';
    const next = await invoke<Settings>('delete_model', { id });
    onsettings(next ?? { ...settings, models: settings.models.filter((m) => m.id !== id) });
    await refreshKeys();
  }

  async function saveSearchKey(provider: 'tavily' | 'brave', remove = false) {
    try {
      const next = await invoke<SearchKeys>('set_search_key', { provider, key: remove ? '' : searchInputs[provider] });
      if (next) search = next;
      searchInputs[provider] = '';
      searchNote = remove ? L('Key removed', 'کلید حذف شد') : L('Key saved ✓', 'کلید ذخیره شد ✓');
    } catch (e) {
      searchNote = String(e);
    }
    setTimeout(() => (searchNote = ''), 3000);
  }

  onMount(() => {
    void refreshKeys();
    void invoke<SearchKeys>('search_keys').then((s) => s && (search = s));
  });
</script>

<h1>{L('Models', 'مدل‌ها')}</h1>
<p class="lead">{L('Choose the model that answers. Keys are kept only in Windows Credential Manager: not in the settings file, not in the logs.', 'مدلی که جواب می‌دهد را انتخاب کن. کلیدها فقط در Windows Credential Manager ذخیره می‌شوند؛ نه در فایل تنظیمات، نه در لاگ.')}</p>

<div class="models">
  {#each settings.models as model (model.id)}
    {@const active = model.id === activeId}
    {@const result = cardResults[model.id]}
    <div class="model-card" class:active in:fly={{ y: 10, duration: 300, easing: cubicOut }} out:scale={{ start: 0.95, duration: 180 }}>
      <button class="select" aria-label="use this model" onclick={() => makeActive(model.id)}>
        <span class="radio">
          {#if active}<span class="radio-dot" in:scale={{ start: 0.2, duration: 360, easing: backOut }}></span>{/if}
        </span>
        <span class="info">
          <span class="name">{model.name}{#if active}<em in:fade>{L('active', 'فعال')}</em>{/if}</span>
          <span class="meta"><code>{model.modelId}</code> · {host(model.baseUrl)}</span>
        </span>
      </button>
      <span class="key" class:has={keys[model.id]}>{keys[model.id] ? L('key saved', 'کلید ذخیره شده') : L('no key', 'بدون کلید')}</span>
      <div class="card-actions">
        <button class="mini" onclick={() => testCard(model)} disabled={result === 'testing'}>{result === 'testing' ? '…' : L('Test', 'تست')}</button>
        <button class="mini" onclick={() => startEdit(model)}>{L('Edit', 'ویرایش')}</button>
        <button class="mini danger" class:sure={confirmDelete === model.id} onclick={() => remove(model.id)}>{confirmDelete === model.id ? L('Sure?', 'مطمئنی؟') : L('Remove', 'حذف')}</button>
      </div>
      {#if result && result !== 'testing'}
        <div class="result" class:ok={result.ok} dir="auto" transition:slide={{ duration: 220 }}>{result.ok ? '✓' : '✕'} {result.text}</div>
      {/if}
    </div>
  {:else}
    {#if !form}
      <div class="empty" in:fade>
        <div class="empty-orb"></div>
        <b>{L('No model added yet', 'هنوز مدلی اضافه نشده')}</b>
        <small>{L('Add a model to start; Gemini works with a free key.', 'برای شروع یک مدل اضافه کن؛ Gemini با یک کلید رایگان کار می‌کند.')}</small>
      </div>
    {/if}
  {/each}
</div>

{#if form}
  <div class="card form" transition:slide={{ duration: 320, easing: cubicOut }}>
    <div class="presets">
      {#each PRESETS as p (p.id)}
        <button class="preset" class:on={preset === p.id} onclick={() => choosePreset(p)}>{L(p.label, p.fa ?? p.label)}</button>
      {/each}
    </div>

    <div class="grid">
      <label>
        <span>{L('Name', 'اسم')}</span>
        <input class="field" bind:value={form.name} maxlength="40" />
      </label>
      <label>
        <span>{L('Model id', 'شناسه مدل')}</span>
        <input class="field mono" bind:value={form.modelId} list="model-examples" dir="ltr" spellcheck="false" />
        <datalist id="model-examples">
          {#each currentPreset.examples as ex (ex)}<option value={ex}></option>{/each}
        </datalist>
      </label>
      <label class="wide">
        <span>{L('Address (Base URL)', 'آدرس (Base URL)')}</span>
        <input class="field mono" bind:value={form.baseUrl} dir="ltr" spellcheck="false" placeholder="https://…" />
      </label>
      <div class="wide">
        <span class="label">{L('API type', 'نوع API')}</span>
        <div class="segmented">
          <button class:on={form.provider === 'gemini'} onclick={() => form && (form.provider = 'gemini')}>Gemini</button>
          <button class:on={form.provider === 'openai'} onclick={() => form && (form.provider = 'openai')}>{L('OpenAI compatible', 'سازگار با OpenAI')}</button>
        </div>
      </div>
      <div class="wide">
        <span class="label">{L('Thinking before the answer', 'فکر کردن قبل از جواب')}</span>
        <div class="segmented">
          {#each THINKING as t (t.id)}
            <button class:on={form.thinking === t.id} onclick={() => form && (form.thinking = t.id)}>{L(t.en, t.fa)}</button>
          {/each}
        </div>
      </div>
      <label class="wide">
        <span>{L('API key', 'کلید API')} {#if currentPreset.keyHint}<small>({currentPreset.keyHint})</small>{/if}</span>
        <div class="key-field">
          <input
            class="field mono"
            type={showKey ? 'text' : 'password'}
            bind:value={keyInput}
            dir="ltr"
            spellcheck="false"
            autocomplete="off"
            placeholder={form.id && keys[form.id] ? L('Saved; paste a new key to change it', 'ذخیره شده — برای تغییر، کلید جدید را بچسبان') : L('Paste the key here', 'کلید را اینجا بچسبان')}
          />
          <button class="eye" aria-label="show key" onclick={() => (showKey = !showKey)}>
            <svg viewBox="0 0 24 24">
              {#if showKey}<path d="M3 3l18 18M10.6 10.6a2 2 0 002.8 2.8M9.9 5.1A9.8 9.8 0 0112 5c6 0 9.5 7 9.5 7a17 17 0 01-3.3 4.2M6.1 6.1C3.8 7.8 2.5 12 2.5 12S6 19 12 19a9.5 9.5 0 004.2-1" />{:else}<path d="M2.5 12S6 5 12 5s9.5 7 9.5 7-3.5 7-9.5 7-9.5-7-9.5-7z" /><circle cx="12" cy="12" r="2.5" />{/if}
            </svg>
          </button>
        </div>
      </label>
    </div>

    {#if formResult}
      <div class="result" class:ok={formResult.ok} dir="auto" transition:slide={{ duration: 220 }}>{formResult.ok ? '✓' : '✕'} {formResult.text}</div>
    {/if}

    <div class="form-actions">
      <button class="btn ghost" onclick={() => (form = null)}>{L('Cancel', 'انصراف')}</button>
      <button class="btn ghost" disabled={busy !== ''} onclick={testForm}>
        {#if busy === 'test'}<span class="spin"></span>{/if}{L('Test the connection', 'تست اتصال')}
      </button>
      <button class="btn primary" disabled={busy !== ''} onclick={saveForm}>
        {#if busy === 'save'}<span class="spin dark"></span>{/if}{L('Save', 'ذخیره')}
      </button>
    </div>
  </div>
{:else}
  <button class="add" onclick={startAdd} in:fly={{ y: 8, duration: 260 }}>
    <svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14" /></svg>
    {L('Add a model', 'افزودن مدل')}
  </button>
{/if}

{#if settings.models.length > 1}
  <div class="card backup" in:fly={{ y: 8, duration: 260 }}>
    <div class="row">
      <div>
        <b>{L('Backup model', 'مدل پشتیبان')}</b>
        <small>{L('When the main model is busy (for example a 503 from Google), this one answers and the log says so.', 'وقتی مدل اصلی شلوغ است (مثلاً خطای 503 گوگل)، این مدل جواب می‌دهد و در لاگ گفته می‌شود.')}</small>
      </div>
      <select class="field mono small-field" value={settings.backupModel ?? ''} onchange={(e) => setBackup((e.currentTarget as HTMLSelectElement).value)}>
        <option value="">{L('No backup', 'بدون پشتیبان')}</option>
        {#each settings.models.filter((m) => m.id !== activeId) as m (m.id)}
          <option value={m.id}>{m.name}</option>
        {/each}
      </select>
    </div>
  </div>
{/if}

<DecisionPanel {settings} {onsettings} />

<h2>{L('Proxy (VPN)', 'پروکسی (VPN)')}</h2>
<p class="lead">{L('Some services answer only through a VPN and some only without one. Choose for each model whether it goes through the proxy.', 'بعضی سرویس‌ها فقط با VPN جواب می‌دهند و بعضی فقط بدون آن. برای هر مدل جدا انتخاب کن که از پروکسی برود یا نه.')}</p>
<div class="card">
  <div class="row">
    <div>
      <b>{L('Proxy address', 'آدرس پروکسی')}</b>
      <small>{L('For example 127.0.0.1:10808 (the SOCKS port of apps like v2rayN) or http://127.0.0.1:10809. Empty = no proxy.', 'مثلاً 127.0.0.1:10808 (SOCKS برنامه‌هایی مثل v2rayN) یا http://127.0.0.1:10809. خالی = بدون پروکسی.')}</small>
    </div>
    <input
      class="field mono small-field"
      dir="ltr"
      placeholder="127.0.0.1:10808"
      bind:value={proxyAddress}
      onchange={() => proxyAddress.trim() !== proxy.address && saveProxy({ address: proxyAddress.trim() })}
    />
  </div>
  {#each settings.models as m (m.id)}
    <div class="row">
      <div>
        <b>{m.name}</b>
        <small>{L("Send this model's requests through the proxy.", 'درخواست‌های این مدل از پروکسی برود.')}</small>
      </div>
      <Toggle checked={proxy.models.includes(m.id)} onchange={(on) => proxyForModel(m.id, on)} label={m.name} />
    </div>
  {/each}
  <div class="row">
    <div>
      <b>{L('Voice (Gemini Live)', 'صدا (Gemini Live)')}</b>
      <small>{L('Voice conversations and spoken answers go through the proxy.', 'گفتگوی صوتی و خواندن جواب‌ها از پروکسی برود.')}</small>
    </div>
    <Toggle checked={proxy.voice} onchange={(on) => saveProxy({ voice: on })} label="voice" />
  </div>
  <div class="row">
    <div>
      <b>{L('Search and web pages', 'جستجو و صفحه‌های وب')}</b>
      <small>{L('Without the proxy first; if a site does not open, it is tried once through the proxy.', 'اول بدون پروکسی؛ اگر سایت باز نشد، یک بار هم از پروکسی امتحان شود.')}</small>
    </div>
    <Toggle checked={proxy.web} onchange={(on) => saveProxy({ web: on })} label="web" />
  </div>
  {#if proxyNote}<div class="note" transition:slide={{ duration: 200 }}>{proxyNote}</div>{/if}
</div>

<h2>{L('Web search', 'جستجوی وب')}</h2>
<p class="lead">{L('Free search works without a key. With a Tavily or Brave key the results are better and faster.', 'بدون کلید هم جستجوی رایگان کار می‌کند. با کلید Tavily یا Brave نتیجه‌ها دقیق‌تر و سریع‌ترند.')}</p>
<div class="card">
  {#each SEARCH_PROVIDERS as p (p.id)}
    <div class="row">
      <div>
        <b>{p.label} <span class="key" class:has={search[p.id]}>{search[p.id] ? L('saved', 'ذخیره شده') : L('none', 'ندارد')}</span></b>
        <small>{p.hint}</small>
      </div>
      <div class="search-key">
        <input class="field mono small-field" type="password" dir="ltr" autocomplete="off" placeholder={L('Key', 'کلید')} bind:value={searchInputs[p.id]} />
        <button class="mini" disabled={!searchInputs[p.id].trim()} onclick={() => saveSearchKey(p.id)}>{L('Save', 'ذخیره')}</button>
        {#if search[p.id]}<button class="mini danger" onclick={() => saveSearchKey(p.id, true)}>{L('Remove', 'حذف')}</button>{/if}
      </div>
    </div>
  {/each}
  {#if searchNote}<div class="note" transition:slide={{ duration: 200 }}>{searchNote}</div>{/if}
</div>

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
  .models {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-bottom: 12px;
  }
  .model-card {
    display: grid;
    grid-template-columns: 1fr auto auto;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
    transition: border-color var(--medium), box-shadow var(--medium), background var(--medium);
  }
  .model-card.active {
    border-color: color-mix(in srgb, var(--accent-text) 55%, transparent);
    background: linear-gradient(90deg, rgba(255, 255, 255, 0.02), color-mix(in srgb, var(--accent) 10%, transparent));
    box-shadow: 0 0 0 3px var(--glow-soft), 0 10px 30px -18px var(--glow);
  }
  .select {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    border: none;
    background: none;
    color: inherit;
    text-align: start;
    cursor: pointer;
    padding: 0;
  }
  .radio {
    width: 20px;
    height: 20px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    border-radius: 50%;
    border: 2px solid var(--border);
    transition: border-color var(--medium);
  }
  .active .radio {
    border-color: var(--accent-text);
  }
  .radio-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--accent-text);
    box-shadow: 0 0 10px var(--glow);
  }
  .info {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
    font-weight: 700;
  }
  .name em {
    font-style: normal;
    font-size: 10.5px;
    color: var(--accent-text);
    border: 1px solid currentColor;
    border-radius: 999px;
    padding: 0 7px;
  }
  .meta {
    margin-top: 2px;
    font-size: 11.5px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    direction: ltr;
    text-align: left;
  }
  :global([dir='rtl']) .meta {
    text-align: right;
  }
  .meta code {
    font-family: var(--mono);
  }
  .key {
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
  .card-actions {
    display: flex;
    gap: 6px;
  }
  .mini {
    padding: 5px 11px;
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
  .mini.danger:hover,
  .mini.sure {
    background: rgba(225, 29, 72, 0.85);
    color: #fff;
  }
  .result {
    grid-column: 1 / -1;
    padding: 8px 12px;
    border-radius: 10px;
    font-size: 12px;
    color: #fecaca;
    background: rgba(248, 113, 113, 0.08);
    word-break: break-word;
  }
  .result.ok {
    color: #a7f3d0;
    background: rgba(52, 211, 153, 0.08);
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 34px 18px;
    border-radius: 16px;
    border: 1px dashed var(--border);
  }
  .empty small {
    color: var(--text-dim);
  }
  .empty-orb {
    width: 46px;
    height: 46px;
    margin-bottom: 10px;
    border-radius: 50%;
    background: conic-gradient(from 0deg, var(--accent), var(--accent-2), var(--accent-3), var(--accent));
    box-shadow: 0 0 30px var(--glow);
    animation: spin 6s linear infinite;
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
    transition: background var(--medium) var(--ease-out), color var(--fast), box-shadow var(--medium);
  }
  .segmented button.on {
    background: rgba(255, 255, 255, 0.12);
    color: var(--text);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.1);
  }
  .key-field {
    position: relative;
  }
  .eye {
    position: absolute;
    inset-inline-end: 6px;
    top: 50%;
    translate: 0 -50%;
    width: 28px;
    height: 28px;
    display: grid;
    place-items: center;
    border: none;
    border-radius: 8px;
    background: transparent;
    cursor: pointer;
  }
  .eye:hover {
    background: rgba(255, 255, 255, 0.08);
  }
  .eye svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: var(--text-dim);
    stroke-width: 1.7;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  /* The key is always written left to right; the eye button sits at the end of the page's direction. */
  .key-field .field {
    padding-right: 40px;
  }
  :global([dir='rtl']) .key-field .field {
    padding-right: 12px;
    padding-left: 40px;
  }
  .form .result {
    margin-top: 12px;
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
  .spin {
    width: 13px;
    height: 13px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.25);
    border-top-color: #fff;
    animation: spin 0.7s linear infinite;
  }
  .spin.dark {
    border-color: rgba(0, 0, 0, 0.2);
    border-top-color: #000;
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
    font-size: 13.5px;
  }
  .row small {
    display: block;
    margin-top: 3px;
    color: var(--text-dim);
    font-size: 12px;
    /* Hints may be an address (left to right) inside a Persian page: each keeps its own direction, on the page's side. */
    unicode-bidi: plaintext;
    text-align: left;
  }
  :global([dir='rtl']) .row small {
    text-align: right;
  }
  .search-key {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .backup {
    margin-top: 12px;
  }
  select.field option {
    background: #1b1b22;
  }
  .note {
    padding: 0 0 12px;
    font-size: 12px;
    color: var(--accent-text);
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
