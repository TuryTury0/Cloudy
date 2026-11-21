<script lang="ts">
  // Settings > Models > Second check: decision models (Liquid's d1 family) that look again before a screen job's
  // "done" and before a click that is hard to undo. Add one, test it (English, Persian, Finglish and screenshots),
  // then turn the check on if the test says it is good. Keys go to the OS keychain only.
  import { onMount } from 'svelte';
  import { fade, fly, scale, slide } from 'svelte/transition';
  import { backOut, cubicOut } from 'svelte/easing';
  import { L } from '@anytask/ui';
  import { inTauri, invoke } from '../lib/ipc';
  import type { DecisionModel, DecisionTestReport, Settings } from '../lib/types';
  import Toggle from './Toggle.svelte';

  interface Props {
    settings: Settings;
    onsettings: (next: Settings) => void;
  }

  let { settings, onsettings }: Props = $props();

  const LIQUID: DecisionModel = { id: '', name: 'Liquid d1-ultra', baseUrl: 'https://api.liquid.ai/decisions/v1/systemone', modelId: 'd1-ultra' };
  const GROUPS: Array<{ id: string; en: string; fa: string }> = [
    { id: 'en', en: 'English', fa: 'انگلیسی' },
    { id: 'fa', en: 'Persian', fa: 'فارسی' },
    { id: 'fin', en: 'Finglish', fa: 'فینگلیش' },
    { id: 'image', en: 'Screenshots', fa: 'اسکرین‌شات' }
  ];

  let keys = $state<string[]>([]);
  let form = $state<DecisionModel | null>(null);
  let keyInput = $state('');
  let busy = $state('');
  let error = $state('');
  let confirmDelete = $state('');
  /** Test reports by model id ('' = the form). */
  let reports = $state<Record<string, DecisionTestReport | 'testing'>>({});
  let open = $state<Record<string, boolean>>({});

  const decision = $derived(settings.decision);
  const activeId = $derived(decision.models.some((m) => m.id === decision.active) ? decision.active : (decision.models[0]?.id ?? ''));

  async function refreshKeys() {
    keys = (await invoke<string[]>('decision_keys')) ?? keys;
  }

  async function call(name: string, args: Record<string, unknown>) {
    error = '';
    try {
      const next = await invoke<Settings>(name, args);
      if (next) onsettings(next);
      await refreshKeys();
      return true;
    } catch (e) {
      error = String(e);
      return false;
    }
  }

  function startAdd() {
    form = { ...LIQUID };
    keyInput = '';
    delete reports[''];
  }

  function startEdit(model: DecisionModel) {
    form = { ...model };
    keyInput = '';
  }

  async function save() {
    if (!form) return;
    busy = 'save';
    const ok = await call('save_decision_model', { model: $state.snapshot(form), apiKey: keyInput || null });
    busy = '';
    if (ok) {
      if (reports['']) {
        const saved = settings.decision.models.find((m) => m.name === form!.name && m.modelId === form!.modelId);
        if (saved) reports[saved.id] = reports[''];
      }
      form = null;
      keyInput = '';
    }
  }

  async function test(model: DecisionModel, slot: string) {
    reports[slot] = 'testing';
    open[slot] = false;
    error = '';
    try {
      const report = await invoke<DecisionTestReport>('test_decision_model', { model: $state.snapshot(model), apiKey: slot === '' ? keyInput || null : null });
      if (report) reports[slot] = report;
      else delete reports[slot];
    } catch (e) {
      delete reports[slot];
      error = String(e);
    }
  }

  async function remove(id: string) {
    if (confirmDelete !== id) {
      confirmDelete = id;
      setTimeout(() => confirmDelete === id && (confirmDelete = ''), 3000);
      return;
    }
    confirmDelete = '';
    delete reports[id];
    await call('delete_decision_model', { id });
  }

  const setEnabled = (on: boolean) => call('set_decision', { enabled: on, active: activeId });
  const makeActive = (id: string) => call('set_decision', { enabled: decision.enabled, active: id });

  function tally(report: DecisionTestReport, lang: string) {
    const cases = report.cases.filter((c) => c.lang === lang);
    return { right: cases.filter((c) => c.ok).length, all: cases.length };
  }

  function verdictText(report: DecisionTestReport): string {
    if (report.verdict === 'failed') return L(`No answer: ${report.error ?? 'unknown error'}`, `جوابی نیامد: ${report.error ?? 'خطای ناشناخته'}`);
    if (!report.imagesWork) return L('It does not take screenshots, so it cannot do the second check.', 'اسکرین‌شات قبول نمی‌کند، پس برای بررسی دوم به کار نمی‌آید.');
    if (report.verdict === 'good') return L('Good: it understands and reads screenshots. You can turn the second check on.', 'خوب است: منظور را می‌فهمد و اسکرین‌شات را می‌خواند. می‌توانی بررسی دوم را روشن کنی.');
    return L('It answers, but makes too many mistakes for the second check.', 'جواب می‌دهد، ولی برای بررسی دوم اشتباهش زیاد است.');
  }

  function host(url: string): string {
    try {
      return new URL(url).host;
    } catch {
      return url;
    }
  }

  onMount(() => {
    void refreshKeys();
    if (!inTauri && new URLSearchParams(location.search).get('preview')) {
      // Design work in a plain browser: a saved model and a finished test.
      settings = { ...settings, decision: { enabled: false, active: 'd1', models: [{ ...LIQUID, id: 'd1' }] } };
      keys = ['d1'];
      const sample = (lang: 'en' | 'fa' | 'fin' | 'image', text: string, ok = true) => ({
        group: lang === 'image' ? 'screen' : 'words',
        lang,
        text,
        want: true,
        probability: ok ? 0.97 : 0.16,
        ok,
        ms: 1000,
        error: null
      });
      reports.d1 = {
        cases: [sample('en', 'stop'), sample('fa', 'قطعش کن'), sample('fin', 'ghat kon', false), sample('image', 'Spotify: Believer playing')],
        passed: 23,
        total: 24,
        imagesWork: true,
        medianMs: 1018,
        verdict: 'good',
        error: null
      } as DecisionTestReport;
    }
  });
</script>

{#snippet report(slot: string)}
  {@const r = reports[slot]}
  {#if r === 'testing'}
    <div class="report testing" transition:slide={{ duration: 220 }}>
      <span class="spin"></span>{L('Testing: 24 questions in English, Persian, Finglish and about screenshots…', 'در حال تست: ۲۴ سؤال به انگلیسی، فارسی، فینگلیش و درباره اسکرین‌شات…')}
    </div>
  {:else if r}
    <div class="report {r.verdict}" class:noimg={!r.imagesWork} transition:slide={{ duration: 260, easing: cubicOut }}>
      <p class="verdict" dir="auto">{r.verdict === 'good' ? '✓' : r.verdict === 'weak' ? '!' : '✕'} {verdictText(r)}</p>
      {#if r.verdict !== 'failed'}
        <div class="tallies">
          {#each GROUPS as g, i (g.id)}
            {@const t = tally(r, g.id)}
            {#if t.all}
              <span class="tally" class:full={t.right === t.all} in:scale={{ start: 0.7, duration: 320, delay: i * 60, easing: backOut }}>
                {L(g.en, g.fa)} <b>{t.right}/{t.all}</b>
              </span>
            {/if}
          {/each}
          <span class="tally time">{L(`about ${(r.medianMs / 1000).toFixed(1)} s each`, `هر کدام حدود ${(r.medianMs / 1000).toFixed(1)} ثانیه`)}</span>
        </div>
        <button class="link" onclick={() => (open[slot] = !open[slot])}>{open[slot] ? L('Hide the answers', 'پنهان کردن جواب‌ها') : L('Show every answer', 'نمایش همه جواب‌ها')}</button>
        {#if open[slot]}
          <ul class="cases" transition:slide={{ duration: 220 }}>
            {#each r.cases as c, i (i)}
              <li class:bad={!c.ok}>
                <span class="mark">{c.ok ? '✓' : '✕'}</span>
                <span class="text" dir="auto">{c.text}</span>
                <span class="got">
                  {L('wanted', 'باید')} {c.want ? L('yes', 'بله') : L('no', 'نه')} ·
                  {c.probability === null ? (c.error ?? L('no answer', 'بی‌جواب')) : `${Math.round(c.probability * 100)}% ${L('yes', 'بله')}`}
                </span>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </div>
  {/if}
{/snippet}

<h2>{L('Second check', 'بررسی دوم')}</h2>
<p class="lead">
  {L(
    'Optional. A decision model looks at the screen again before the assistant says a screen job is done, and asks you before a click that is hard to undo (delete, send, buy). Any model served with the Liquid decisions API works, such as d1-ultra; it must accept screenshots.',
    'اختیاری. یک مدل تصمیم‌گیر قبل از اینکه دستیار بگوید کار روی صفحه تمام شد دوباره صفحه را نگاه می‌کند، و قبل از کلیکی که برگشت ندارد (حذف، ارسال، خرید) از تو می‌پرسد. هر مدلی که با API تصمیم Liquid کار کند (مثل d1-ultra) قبول است؛ باید اسکرین‌شات قبول کند.'
  )}
</p>

<div class="card toggle-card">
  <div class="row">
    <div>
      <b>{L('Use the second check', 'استفاده از بررسی دوم')}</b>
      <small>
        {#if !decision.models.length}{L('Add a decision model below first.', 'اول پایین یک مدل تصمیم‌گیر اضافه کن.')}
        {:else if decision.enabled}{L('On: screen jobs are checked by', 'روشن؛ کارهای روی صفحه را این مدل بررسی می‌کند:')} {decision.models.find((m) => m.id === activeId)?.name}
        {:else}{L('Off: screen jobs work as before.', 'خاموش؛ کارهای روی صفحه مثل قبل انجام می‌شوند.')}{/if}
      </small>
    </div>
    {#if decision.models.length}
      <Toggle checked={decision.enabled} onchange={setEnabled} label="second check" />
    {/if}
  </div>
</div>

<div class="models">
  {#each decision.models as model (model.id)}
    {@const active = model.id === activeId}
    <div class="model-card" class:active in:fly={{ y: 10, duration: 300, easing: cubicOut }} out:scale={{ start: 0.95, duration: 180 }}>
      <button class="select" aria-label="use this decision model" onclick={() => makeActive(model.id)}>
        <span class="radio">
          {#if active}<span class="radio-dot" in:scale={{ start: 0.2, duration: 360, easing: backOut }}></span>{/if}
        </span>
        <span class="info">
          <span class="name">{model.name}{#if active && decision.enabled}<em in:fade>{L('in use', 'فعال')}</em>{/if}</span>
          <span class="meta"><code>{model.modelId}</code> · {host(model.baseUrl)}</span>
        </span>
      </button>
      <span class="key" class:has={keys.includes(model.id)}>{keys.includes(model.id) ? L('key saved', 'کلید ذخیره شده') : L('no key', 'بدون کلید')}</span>
      <div class="card-actions">
        <button class="mini accent" onclick={() => test(model, model.id)} disabled={reports[model.id] === 'testing'}>{L('Test', 'تست')}</button>
        <button class="mini" onclick={() => startEdit(model)}>{L('Edit', 'ویرایش')}</button>
        <button class="mini danger" class:sure={confirmDelete === model.id} onclick={() => remove(model.id)}>
          {confirmDelete === model.id ? L('Sure?', 'مطمئنی؟') : L('Remove', 'حذف')}
        </button>
      </div>
      <div class="report-slot">{@render report(model.id)}</div>
    </div>
  {/each}
</div>

{#if form}
  <div class="card form" transition:slide={{ duration: 320, easing: cubicOut }}>
    <div class="grid">
      <label>
        <span>{L('Name', 'اسم')}</span>
        <input class="field" bind:value={form.name} maxlength="40" />
      </label>
      <label>
        <span>{L('Model id', 'شناسه مدل')}</span>
        <input class="field mono" bind:value={form.modelId} list="decision-examples" dir="ltr" spellcheck="false" />
        <datalist id="decision-examples"><option value="d1-ultra"></option><option value="d1"></option></datalist>
      </label>
      <label class="wide">
        <span>{L('Address (decisions endpoint)', 'آدرس (endpoint تصمیم)')}</span>
        <input class="field mono" bind:value={form.baseUrl} dir="ltr" spellcheck="false" placeholder="https://…" />
      </label>
      <label class="wide">
        <span>{L('API key', 'کلید API')} <small>(console.liquid.ai)</small></span>
        <input
          class="field mono"
          type="password"
          bind:value={keyInput}
          dir="ltr"
          autocomplete="off"
          spellcheck="false"
          placeholder={form.id && keys.includes(form.id) ? L('Saved; paste a new key to change it', 'ذخیره شده — برای تغییر، کلید جدید را بچسبان') : L('Paste the key here', 'کلید را اینجا بچسبان')}
        />
      </label>
    </div>
    {@render report('')}
    <div class="form-actions">
      <button class="btn ghost" onclick={() => (form = null)}>{L('Cancel', 'انصراف')}</button>
      <button class="btn ghost" disabled={busy !== '' || reports[''] === 'testing'} onclick={() => form && test(form, '')}>{L('Test', 'تست')}</button>
      <button class="btn primary" disabled={busy !== ''} onclick={save}>
        {#if busy === 'save'}<span class="spin dark"></span>{/if}{L('Save', 'ذخیره')}
      </button>
    </div>
  </div>
{:else}
  <button class="add" onclick={startAdd} in:fly={{ y: 8, duration: 260 }}>
    <svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14" /></svg>
    {L('Add a decision model', 'افزودن مدل تصمیم‌گیر')}
  </button>
{/if}
{#if error}<p class="error" dir="auto" transition:slide={{ duration: 200 }}>✕ {error}</p>{/if}

<style>
  h2 {
    margin: 28px 0 10px;
    font-size: 16px;
    font-weight: 800;
  }
  .lead {
    margin: -4px 0 14px;
    color: var(--text-dim);
    font-size: 13px;
    line-height: 1.6;
  }
  .card {
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
    padding: 6px 18px;
    margin-bottom: 12px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 18px;
    padding: 12px 0;
  }
  .row b {
    display: block;
    font-size: 13.5px;
  }
  .row small {
    display: block;
    margin-top: 3px;
    color: var(--text-dim);
    font-size: 12px;
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
    box-shadow: var(--card-shadow);
    backdrop-filter: var(--glass-blur);
    transition: border-color var(--medium), box-shadow var(--medium);
  }
  .model-card.active {
    border-color: color-mix(in srgb, var(--accent-text) 55%, transparent);
    box-shadow: var(--card-shadow), 0 0 0 3px var(--glow-soft);
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
  .mini.accent {
    color: var(--accent-text);
  }
  .mini.danger:hover,
  .mini.sure {
    background: rgba(225, 29, 72, 0.85);
    color: #fff;
  }
  .report-slot {
    grid-column: 1 / -1;
  }
  .report-slot:empty {
    display: none;
  }
  .report {
    margin-top: 4px;
    padding: 12px 14px;
    border-radius: 13px;
    font-size: 12.5px;
    background: rgba(255, 255, 255, 0.04);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }
  .form .report {
    margin-top: 14px;
  }
  .report.testing {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-muted);
  }
  .report.good {
    background: rgba(52, 211, 153, 0.07);
    box-shadow: inset 0 0 0 1px rgba(52, 211, 153, 0.25), 0 10px 30px -18px rgba(52, 211, 153, 0.6);
  }
  .report.weak,
  .report.noimg {
    background: rgba(251, 191, 36, 0.07);
    box-shadow: inset 0 0 0 1px rgba(251, 191, 36, 0.25);
  }
  .report.failed {
    background: rgba(248, 113, 113, 0.08);
    box-shadow: inset 0 0 0 1px rgba(248, 113, 113, 0.3);
  }
  .verdict {
    margin: 0;
    font-weight: 700;
    line-height: 1.5;
  }
  .good .verdict {
    color: #a7f3d0;
  }
  .weak .verdict,
  .noimg .verdict {
    color: #fde68a;
  }
  .failed .verdict {
    color: #fecaca;
    overflow-wrap: anywhere;
  }
  .tallies {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
  }
  .tally {
    padding: 3px 10px;
    border-radius: 999px;
    font-size: 11.5px;
    color: var(--text-muted);
    background: rgba(255, 255, 255, 0.06);
  }
  .tally b {
    color: #fde68a;
    font-family: var(--font-latin);
  }
  .tally.full b {
    color: #6ee7b7;
  }
  .tally.time {
    background: none;
    color: var(--text-dim);
  }
  .link {
    margin-top: 8px;
    padding: 0;
    border: none;
    background: none;
    color: var(--accent-text);
    font-size: 12px;
    cursor: pointer;
  }
  .cases {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .cases li {
    display: grid;
    grid-template-columns: 16px 1fr auto;
    gap: 8px;
    align-items: center;
    font-size: 12px;
  }
  .cases .mark {
    color: #6ee7b7;
    font-weight: 800;
  }
  .cases li.bad .mark,
  .cases li.bad .got {
    color: #fca5a5;
  }
  .cases .text {
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cases .got {
    color: var(--text-dim);
    font-size: 11px;
    white-space: nowrap;
  }
  .form {
    padding: 16px 18px;
    border-color: color-mix(in srgb, var(--accent-text) 30%, transparent);
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px 14px;
  }
  .grid label {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-muted);
  }
  .grid small {
    font-weight: 500;
    color: var(--text-dim);
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
  .add {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    width: 100%;
    padding: 13px;
    border-radius: 14px;
    border: 1px dashed var(--border);
    background: transparent;
    color: var(--text-muted);
    font-size: 13px;
    font-weight: 700;
    cursor: pointer;
    transition: background var(--fast), color var(--fast), border-color var(--fast);
  }
  .add:hover {
    background: var(--surface-hover);
    color: var(--text);
    border-color: var(--accent-text);
  }
  .add svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2.2;
    stroke-linecap: round;
  }
  .spin {
    display: inline-block;
    width: 13px;
    height: 13px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.25);
    border-top-color: var(--accent-text);
    animation: spin 0.7s linear infinite;
  }
  .spin.dark {
    border-color: rgba(0, 0, 0, 0.2);
    border-top-color: #000;
  }
  .error {
    margin: 8px 0 0;
    color: #fecaca;
    font-size: 12.5px;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
