<script lang="ts">
  // Settings window: frameless glass window with a sidebar. Changes are kept as a draft and saved together;
  // shortcuts are applied at once and any that could not be registered are shown next to them.
  import { onMount } from 'svelte';
  import { Spring } from 'svelte/motion';
  import { fade, fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { Aurora, Island, L, LANGUAGES, PRESET_THEMES, applyMotion, applyTheme, dir, normalizeTheme, setLanguage, trackShine, ASSISTANT_STATES, type AssistantState, type IslandStep, type Theme } from '@anytask/ui';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import HotkeyRecorder from '../components/HotkeyRecorder.svelte';
  import HomePage from '../components/HomePage.svelte';
  import ModelsPage from '../components/ModelsPage.svelte';
  import LogsPage from '../components/LogsPage.svelte';
  import ToolsPage from '../components/ToolsPage.svelte';
  import WakePanel from '../components/WakePanel.svelte';
  import PhonePage from '../components/PhonePage.svelte';
  import V1Import from '../components/V1Import.svelte';
  import PcCard from '../components/PcCard.svelte';
  import LearnPage from '../components/LearnPage.svelte';
  import Toggle from '../components/Toggle.svelte';
  import { inTauri, invoke, listen } from '../lib/ipc';
  import { DEFAULT_SETTINGS, type AppInfo, type HotkeyProblem, type Hotkeys, type SaveReport, type Settings, type UpdateStatus } from '../lib/types';

  type Page = 'home' | 'general' | 'hotkeys' | 'appearance' | 'models' | 'tools' | 'learn' | 'phone' | 'logs' | 'about';
  const PAGES: Array<{ id: Page; en: string; fa: string; icon: string; soon?: string }> = [
    { id: 'home', en: 'Home', fa: 'خانه', icon: 'M3 11l9-7 9 7M5 9.5V20h5v-6h4v6h5V9.5' },
    { id: 'general', en: 'General', fa: 'عمومی', icon: 'M12 3l8 4v6c0 4-3.5 7-8 8-4.5-1-8-4-8-8V7z' },
    { id: 'hotkeys', en: 'Shortcuts', fa: 'میانبرها', icon: 'M4 7h16v10H4zM7 10h1M10 10h1M13 10h1M16 10h1M8 14h8' },
    { id: 'appearance', en: 'Appearance', fa: 'ظاهر', icon: 'M12 3a9 9 0 100 18c1 0 1.5-.8 1.5-1.6 0-1.2-1-1.4-1-2.4 0-1 .8-1.6 1.8-1.6H17a4 4 0 004-4c0-4.7-4-8.4-9-8.4z' },
    { id: 'models', en: 'Models', fa: 'مدل‌ها', icon: 'M12 2l9 5v10l-9 5-9-5V7zM12 22V12M21 7l-9 5-9-5' },
    { id: 'tools', en: 'Tools & MCP', fa: 'ابزارها و MCP', icon: 'M14 6l4 4-8 8H6v-4zM16 4l4 4' },
    { id: 'learn', en: 'Learning', fa: 'آموزش', icon: 'M12 3l9 5-9 5-9-5zM5 10.5V16c0 1.5 3 3.5 7 3.5s7-2 7-3.5v-5.5M21 8v6' },
    { id: 'phone', en: 'Phone', fa: 'گوشی', icon: 'M8 2h8a2 2 0 012 2v16a2 2 0 01-2 2H8a2 2 0 01-2-2V4a2 2 0 012-2zM11 18h2' },
    { id: 'logs', en: 'Logs', fa: 'لاگ‌ها', icon: 'M5 4h14v16H5zM8 8h8M8 12h8M8 16h5' },
    { id: 'about', en: 'About', fa: 'درباره', icon: 'M12 8h.01M11 12h1v5h1M12 3a9 9 0 110 18 9 9 0 010-18z' }
  ];

  /** Label and help are [English, Persian]. */
  const HOTKEY_ROWS: Array<{ key: keyof Hotkeys; action: string; label: [string, string]; help: [string, string] }> = [
    { key: 'talk', action: 'talk', label: ['Talk to the assistant', 'صحبت با دستیار'], help: ['Opens or closes the microphone', 'میکروفون را باز یا بسته می‌کند'] },
    { key: 'typeText', action: 'type', label: ['Type a request', 'نوشتن درخواست'], help: ['The text box on the island', 'جعبه متن روی جزیره'] },
    { key: 'toggleIsland', action: 'toggle-island', label: ['Show / hide the island', 'نمایش / پنهان جزیره'], help: ['', ''] },
    { key: 'stop', action: 'stop', label: ['Stop everything', 'توقف همه چیز'], help: ['Stops whatever is running', 'کارهای در حال اجرا را متوقف می‌کند'] },
    { key: 'openSettings', action: 'settings', label: ['Open Settings', 'باز کردن تنظیمات'], help: ['', ''] },
    { key: 'openChat', action: 'chat', label: ['Open the chat', 'باز کردن گفتگو'], help: ['The chat window with its history', 'پنجره چت با تاریخچه'] }
  ];

  const PAGE_IDS = PAGES.map((p) => p.id) as string[];
  const VOICES = ['Aoede', 'Kore', 'Leda', 'Zephyr', 'Puck', 'Charon', 'Fenrir', 'Orus'];
  let voiceKey = $state(false);
  let voiceKeyInput = $state('');

  async function saveVoiceKey(key: string) {
    voiceKey = (await invoke<boolean>('set_voice_key', { key })) ?? voiceKey;
    voiceKeyInput = '';
  }
  const startPage = new URLSearchParams(location.search).get('page') ?? '';
  const PREVIEW_STEPS: IslandStep[] = [
    { id: '1', label: 'Opening Spotify', status: 'done' },
    { id: '2', label: 'Clicking #12 "Play"', status: 'running' }
  ];

  // Without a start page the window opens on Home (an overview of every part); an explicit page is kept.
  let page = $state<Page>(PAGE_IDS.includes(startPage) ? (startPage as Page) : 'home');
  let saved = $state<Settings>(structuredClone(DEFAULT_SETTINGS));
  let draft = $state<Settings>(structuredClone(DEFAULT_SETTINGS));
  let problems = $state<HotkeyProblem[]>([]);
  let saving = $state(false);
  let notice = $state('');
  let info = $state<AppInfo | null>(null);
  let update = $state<UpdateStatus | null>(null);
  const updateBusy = $derived(['checking', 'downloading', 'installing'].includes(update?.state ?? ''));

  function updateText(u: UpdateStatus | null): string {
    const v = u?.version ?? '';
    switch (u?.state) {
      case 'checking':
        return L('Looking on GitHub…', 'در حال بررسی در GitHub…');
      case 'latest':
        return L('You have the newest version', 'آخرین نسخه را داری');
      case 'downloading':
        return u.progress != null ? L(`Downloading version ${v}… ${u.progress}%`, `در حال دانلود نسخه ${v}… ${u.progress}٪`) : L(`Downloading version ${v}…`, `در حال دانلود نسخه ${v}…`);
      case 'ready':
        return saved.autoUpdate
          ? L(`Version ${v} is ready; it installs when the assistant is free`, `نسخه ${v} آماده است؛ وقتی دستیار بیکار شد نصب می‌شود`)
          : L(`Version ${v} is downloaded`, `نسخه ${v} دانلود شد`);
      case 'installing':
        return L('Installing; Cloudy opens again by itself', 'در حال نصب؛ Cloudy خودش دوباره باز می‌شود');
      case 'failed':
        return L(`Could not update: ${u.error ?? ''}`, `به‌روزرسانی نشد: ${u.error ?? ''}`);
      default:
        return L('Checked a minute after start, then every 6 hours', 'یک دقیقه بعد از شروع و بعد هر ۶ ساعت بررسی می‌شود');
    }
  }

  async function checkUpdate() {
    update = (await invoke<UpdateStatus>('update_check')) ?? update;
  }

  async function installUpdate() {
    try {
      await invoke('update_install');
    } catch (e) {
      if (update) update = { ...update, state: 'failed', error: String(e) };
    }
  }
  let previewState = $state<AssistantState>('working');

  const dirty = $derived(JSON.stringify(draft) !== JSON.stringify(saved));
  // The glass capsule behind the open page's name: it springs to its new place and stretches on the way, like a drop.
  const indicator = new Spring(0, { stiffness: 0.17, damping: 0.6 });
  $effect(() => {
    indicator.target = PAGES.findIndex((p) => p.id === page) * 44;
  });
  const stretch = $derived(Math.min(0.5, Math.abs(indicator.target - indicator.current) / 110));
  $effect(() => {
    setLanguage(draft.language);
  });
  $effect(() => {
    applyTheme(normalizeTheme(draft.theme));
  });
  $effect(() => {
    applyMotion(draft.reduceMotion);
  });

  function problemFor(action: string) {
    return problems.find((p) => p.action === action)?.reason ?? '';
  }

  async function save() {
    saving = true;
    notice = '';
    const report = await invoke<SaveReport>('save_settings', { settings: $state.snapshot(draft) });
    saving = false;
    if (!inTauri) {
      saved = structuredClone($state.snapshot(draft));
      notice = L('Saved (preview mode)', 'ذخیره شد (حالت پیش‌نمایش)');
      return;
    }
    if (!report?.saved) {
      notice = L(`Not saved: ${report?.error ?? 'unknown error'}`, `ذخیره نشد: ${report?.error ?? 'خطای ناشناخته'}`);
      return;
    }
    problems = report.problems;
    saved = structuredClone($state.snapshot(draft));
    notice = problems.length ? L('Saved, but some shortcuts could not be used', 'ذخیره شد، اما بعضی میانبرها استفاده نشدند') : L('Saved ✓', 'ذخیره شد ✓');
    setTimeout(() => (notice = ''), 3500);
  }

  function discard() {
    draft = structuredClone($state.snapshot(saved));
    problems = [];
  }

  /** Model changes are saved at once by their own commands; keep any other unsaved edits in the draft. */
  function onModels(next: Settings) {
    saved = structuredClone(next);
    draft.models = structuredClone(next.models);
    draft.activeModel = next.activeModel;
    draft.decision = structuredClone(next.decision);
  }

  function chooseTheme(theme: Theme) {
    draft.theme = { name: theme.name, colors: [...theme.colors], weights: [...theme.weights] };
  }

  onMount(() => {
    void invoke<boolean>('voice_key_saved').then((v) => (voiceKey = !!v));
    // The window is shown only once this page has loaded (see window_ready in commands.rs).
    void invoke('window_ready');
    void invoke<Settings>('get_settings').then((s) => {
      if (!s) return;
      saved = structuredClone(s);
      draft = structuredClone(s);
    });
    void invoke<AppInfo>('app_info').then((i) => (info = i ?? null));
    void invoke<UpdateStatus>('update_status').then((u) => u && (update = u));
    let stopUpdates = () => {};
    void listen<UpdateStatus>('update-status', (u) => (update = u)).then((u) => (stopUpdates = u));
    let stopListening = () => {};
    void listen<string>('settings-page', (p) => PAGE_IDS.includes(p) && (page = p as Page)).then((u) => (stopListening = u));
    let i = 0;
    const cycle = setInterval(() => {
      i = (i + 1) % ASSISTANT_STATES.length;
      previewState = ASSISTANT_STATES[i] === 'text-input' ? 'idle' : ASSISTANT_STATES[i];
    }, 2200);
    const stopShine = trackShine();
    if (!inTauri) {
      // Design work in a plain browser: ?view=settings&page=appearance&lang=fa.
      const lang = new URLSearchParams(location.search).get('lang') === 'fa' ? 'fa' : 'en';
      draft.language = lang;
      saved.language = lang;
    }
    return () => {
      clearInterval(cycle);
      stopListening();
      stopUpdates();
      stopShine();
    };
  });

  const win = inTauri ? getCurrentWindow() : null;
</script>

<div class="app-window" dir={dir()}>
  <div class="backdrop"><Aurora colors={draft.theme.colors} intensity={0.55} speed={0.6} /></div>
  <header class="titlebar" data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region>
      <span class="logo"></span>
      <span data-tauri-drag-region>{L(`${draft.assistantName} Settings`, `تنظیمات ${draft.assistantName}`)}</span>
    </div>
    <div class="controls">
      <button class="ctl" aria-label="minimize" onclick={() => win?.minimize()}>
        <svg viewBox="0 0 12 12"><path d="M2 6h8" /></svg>
      </button>
      <button class="ctl close" aria-label="close" onclick={() => win?.close()}>
        <svg viewBox="0 0 12 12"><path d="M3 3l6 6M9 3l-6 6" /></svg>
      </button>
    </div>
  </header>

  <div class="body">
    <nav class="sidebar">
      <span class="active-pill" style="transform: translateY({indicator.current}px) scaleY({1 + stretch}) scaleX({1 - stretch * 0.08})"></span>
      {#each PAGES as p (p.id)}
        <button class="nav" class:active={page === p.id} onclick={() => (page = p.id)}>
          <svg viewBox="0 0 24 24"><path d={p.icon} /></svg>
          <span>{L(p.en, p.fa)}</span>
          {#if p.soon}<em>{p.soon}</em>{/if}
        </button>
      {/each}
    </nav>

    <main class="content">
      {#key page}
        <section in:fly={{ y: 14, duration: 320, easing: cubicOut }} out:fade={{ duration: 90 }}>
          {#if page === 'home'}
            <HomePage onopen={(p) => (page = p)} />
          {:else if page === 'general'}
            <h1>{L('General', 'عمومی')}</h1>
            <div class="card">
              <label class="row">
                <div><b>{L('Language', 'زبان')}</b><small>{L('The language of the app', 'زبان برنامه')}</small></div>
                <select class="field" bind:value={draft.language}>
                  {#each LANGUAGES as l (l.id)}<option value={l.id}>{l.label}</option>{/each}
                </select>
              </label>
              <label class="row">
                <div><b>{L('Assistant name', 'اسم دستیار')}</b><small>{L('Shown on the island', 'روی جزیره نمایش داده می‌شود')}</small></div>
                <input class="field" maxlength="40" bind:value={draft.assistantName} />
              </label>
              <label class="row">
                <div><b>{L('Your name', 'اسم شما')}</b><small>{L('The assistant calls you by this name', 'دستیار شما را با این اسم صدا می‌کند')}</small></div>
                <input class="field" maxlength="40" bind:value={draft.userName} />
              </label>
              <div class="row">
                <div><b>{L('Show the island at start', 'نمایش جزیره هنگام شروع')}</b><small>{L('When off, a shortcut or the tray icon opens it', 'اگر خاموش باشد با میانبر یا آیکون سینی باز می‌شود')}</small></div>
                <Toggle checked={draft.showIsland} onchange={(v) => (draft.showIsland = v)} label="show island" />
              </div>
              <div class="row">
                <div><b>{L('Start with Windows', 'شروع همراه ویندوز')}</b><small>{L('Cloudy runs when you sign in, so the phone finds it without opening the app', 'وقتی وارد ویندوز می‌شوی Cloudy خودش اجرا می‌شود تا گوشی بدون باز کردن برنامه پیدایش کند')}</small></div>
                <Toggle checked={draft.startWithWindows} onchange={(v) => (draft.startWithWindows = v)} label="start with windows" />
              </div>
              <div class="row">
                <div><b>{L('Update by itself', 'به‌روزرسانی خودکار')}</b><small>{L('A new version comes from GitHub and installs when the assistant is free', 'نسخه جدید از GitHub می‌آید و وقتی دستیار بیکار است نصب می‌شود')}</small></div>
                <Toggle checked={draft.autoUpdate} onchange={(v) => (draft.autoUpdate = v)} label="update by itself" />
              </div>
            </div>
            <h2 class="sub">{L('Voice', 'صدا')}</h2>
            <div class="card">
              <label class="row">
                <div><b>{L('Voice model', 'مدل صدا')}</b><small>{L('Gemini Live; its key comes from a Gemini model in Models', 'Gemini Live؛ کلیدش از مدل Gemini در بخش مدل‌ها می‌آید')}</small></div>
                <input class="field" dir="ltr" maxlength="80" bind:value={draft.voice.model} />
              </label>
              <label class="row">
                <div><b>{L("The assistant's voice", 'صدای دستیار')}</b><small>{L("One of Gemini's ready-made voices", 'یکی از صداهای آماده Gemini')}</small></div>
                <select class="field" dir="ltr" bind:value={draft.voice.voice}>
                  {#each VOICES as v (v)}<option value={v}>{v}</option>{/each}
                </select>
              </label>
              <label class="row">
                <div><b>{L('Close the microphone after silence (seconds)', 'بستن میکروفون بعد از سکوت (ثانیه)')}</b><small>{L('When nobody speaks, the microphone closes by itself', 'اگر کسی صحبت نکند، میکروفون خودش بسته می‌شود')}</small></div>
                <input class="field" type="number" min="5" max="300" bind:value={draft.voice.quietSeconds} />
              </label>
              <div class="row">
                <div><b>{L('Read written answers aloud too', 'جواب‌های نوشتاری را هم با صدا بخوان')}</b><small>{L('When you type, the answer is spoken too and its text stays in the chat (the steps of a task are not spoken)', 'وقتی تایپ می‌کنی، جواب هم گفته می‌شود و متنش در گفتگو می‌ماند (قدم‌های کار گفته نمی‌شوند)')}</small></div>
                <Toggle checked={draft.voice.speakAnswers} onchange={(v) => (draft.voice.speakAnswers = v)} label="speak answers" />
              </div>
              <div class="row">
                <div>
                  <b>{L('Voice API key', 'کلید API صدا')} <span class="badge" class:has={voiceKey}>{voiceKey ? L('saved', 'ذخیره شده') : L('from the Gemini model', 'از مدل Gemini')}</span></b>
                  <small>{L("Optional; when empty, the Gemini model's key is used", 'اختیاری؛ اگر خالی باشد کلید مدل Gemini استفاده می‌شود')}</small>
                </div>
                <div class="key-row">
                  <input class="field small" type="password" dir="ltr" autocomplete="off" placeholder="AIza…" bind:value={voiceKeyInput} />
                  <button class="mini" disabled={!voiceKeyInput.trim()} onclick={() => saveVoiceKey(voiceKeyInput)}>{L('Save', 'ذخیره')}</button>
                  {#if voiceKey}<button class="mini" onclick={() => saveVoiceKey('')}>{L('Remove', 'حذف')}</button>{/if}
                </div>
              </div>
            </div>
            <WakePanel />
            <PcCard />
          {:else if page === 'hotkeys'}
            <h1>{L('Shortcuts', 'میانبرها')}</h1>
            <p class="lead">{L('To change one, click it and press the new keys. Shortcuts work in every app.', 'برای تغییر، روی میانبر کلیک کن و کلیدهای جدید را فشار بده. میانبرها در همه برنامه‌ها کار می‌کنند.')}</p>
            <div class="card">
              {#each HOTKEY_ROWS as row (row.key)}
                <div class="row">
                  <div><b>{L(...row.label)}</b>{#if row.help[0]}<small>{L(...row.help)}</small>{/if}</div>
                  <div class="recorder-cell">
                    <HotkeyRecorder value={draft.hotkeys[row.key]} problem={problemFor(row.action)} onchange={(v) => (draft.hotkeys[row.key] = v)} />
                  </div>
                </div>
              {/each}
            </div>
            <button class="link" onclick={() => (draft.hotkeys = { ...DEFAULT_SETTINGS.hotkeys })}>{L('Back to the defaults', 'بازگشت به پیش‌فرض')}</button>
          {:else if page === 'appearance'}
            <h1>{L('Appearance', 'ظاهر')}</h1>
            <div class="preview">
              <Island
                mode={previewState}
                name={draft.assistantName}
                detail={previewState === 'listening' || previewState === 'speaking' ? L('Live preview', 'پیش‌نمایش زنده') : ''}
                request={L('Play my music', 'آهنگ بذار')}
                steps={PREVIEW_STEPS}
                answer={L('This is a preview of the **answer card**; the text types itself in.', 'این پیش‌نمایش **کارت جواب** است؛ متن با انیمیشن تایپ می‌شود.')}
                level={0.4}
                colors={draft.theme.colors}
              />
            </div>
            <div class="themes">
              {#each PRESET_THEMES as theme (theme.name)}
                <button class="theme" class:selected={draft.theme.name === theme.name} onclick={() => chooseTheme(theme)}>
                  <span class="swatch" style="background: linear-gradient(110deg, {theme.colors.join(', ')})"></span>
                  <span>{theme.name}</span>
                </button>
              {/each}
            </div>
            <div class="card">
              <div class="row">
                <div><b>{L('Reduce motion', 'کاهش حرکت')}</b><small>{L('Stops all animations (for slow computers, or if motion bothers you)', 'همه انیمیشن‌ها را متوقف می‌کند (برای کامپیوترهای ضعیف یا اگر حرکت اذیت می‌کند)')}</small></div>
                <Toggle checked={draft.reduceMotion} onchange={(v) => (draft.reduceMotion = v)} label="reduce motion" />
              </div>
              <div class="row">
                <div><b>{L('Custom colours', 'رنگ‌های دلخواه')}</b><small>{L('Up to three colours; the first is the main one', 'تا سه رنگ؛ اولی رنگ اصلی است')}</small></div>
                <div class="pickers">
                  {#each draft.theme.colors as color, i (i)}
                    <input
                      type="color"
                      value={color}
                      oninput={(e) => {
                        draft.theme.colors[i] = (e.currentTarget as HTMLInputElement).value;
                        draft.theme.name = 'Custom';
                      }}
                    />
                  {/each}
                </div>
              </div>
            </div>
          {:else if page === 'models'}
            <ModelsPage settings={saved} onsettings={onModels} />
          {:else if page === 'learn'}
            <LearnPage />
          {:else if page === 'phone'}
            <PhonePage />
          {:else if page === 'tools'}
            <ToolsPage />
          {:else if page === 'logs'}
            <LogsPage />
          {:else if page === 'about'}
            <h1>{L('About', 'درباره')}</h1>
            <div class="card about">
              <div class="row"><b>{L('Version', 'نسخه')}</b><span class="mono">{info?.version ?? '-'}</span></div>
              <div class="row">
                <div><b>{L('Updates', 'به‌روزرسانی')}</b><small>{updateText(update)}</small></div>
                {#if update?.state === 'ready'}
                  <button class="mini" onclick={installUpdate}>{L('Install now', 'نصب الان')}</button>
                {:else}
                  <button class="mini" disabled={updateBusy} onclick={checkUpdate}>{L('Check for updates', 'بررسی به‌روزرسانی')}</button>
                {/if}
              </div>
              <div class="row"><b>{L('Settings file', 'فایل تنظیمات')}</b><span class="mono small">{info?.settingsPath ?? '-'}</span></div>
              <div class="row"><b>{L('Built with', 'ساخته شده با')}</b><span>Tauri + Rust + Svelte</span></div>
            </div>
            <V1Import />
          {:else}
            <h1>{L(PAGES.find((p) => p.id === page)?.en ?? '', PAGES.find((p) => p.id === page)?.fa ?? '')}</h1>
            <div class="card soon">
              <div class="soon-orb"></div>
              <b>{L('Coming soon', 'به زودی')}</b>
            </div>
          {/if}
        </section>
      {/key}
    </main>
  </div>

  {#if dirty || notice}
    <footer class="savebar" transition:fly={{ y: 40, duration: 320, easing: cubicOut }}>
      <span>{notice || L('Unsaved changes', 'تغییرات ذخیره نشده')}</span>
      {#if dirty}
        <div class="actions">
          <button class="btn ghost" onclick={discard}>{L('Discard', 'لغو')}</button>
          <button class="btn primary" disabled={saving} onclick={save}>{saving ? L('Saving…', 'در حال ذخیره…') : L('Save', 'ذخیره')}</button>
        </div>
      {/if}
    </footer>
  {/if}
</div>

<style>
  .titlebar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 46px;
    padding: 0 14px;
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.06), transparent);
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    font-weight: 700;
  }
  .logo {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: radial-gradient(circle at 35% 30%, #fff, #b8c3d6);
    box-shadow: 0 0 12px var(--glow);
  }
  .controls {
    display: flex;
    gap: 6px;
    direction: ltr;
  }
  .ctl {
    width: 30px;
    height: 26px;
    border: none;
    border-radius: 8px;
    background: transparent;
    cursor: pointer;
  }
  .ctl svg {
    width: 12px;
    height: 12px;
    stroke: var(--text-muted);
    stroke-width: 1.4;
    fill: none;
  }
  .ctl:hover {
    background: rgba(255, 255, 255, 0.08);
  }
  .close:hover {
    background: #e11d48;
  }
  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .sidebar {
    position: relative;
    width: 210px;
    padding: 14px 10px;
    border-inline-end: 1px solid rgba(255, 255, 255, 0.06);
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.035), rgba(255, 255, 255, 0.01));
    backdrop-filter: var(--glass-blur);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .active-pill {
    position: absolute;
    top: 14px;
    right: 10px;
    left: 10px;
    height: 40px;
    border-radius: 13px;
    background: var(--shine), linear-gradient(-75deg, rgba(255, 255, 255, 0.05), rgba(255, 255, 255, 0.16), rgba(255, 255, 255, 0.05));
    box-shadow: inset 0 1px 1px rgba(255, 255, 255, 0.4), inset 0 -1px 1px rgba(0, 0, 0, 0.35), inset 0 0 0 1px rgba(255, 255, 255, 0.07),
      0 6px 16px -8px rgba(0, 0, 0, 0.6), 0 0 22px -4px var(--glow-soft);
    backdrop-filter: blur(6px) saturate(180%);
    transform-origin: 50% 50%;
  }
  .nav {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    height: 40px;
    padding: 0 12px;
    border: none;
    border-radius: 12px;
    background: transparent;
    color: var(--text-muted);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: color var(--fast), transform var(--medium) var(--ease-spring);
  }
  .nav:hover,
  .nav.active {
    color: var(--text);
  }
  .nav:active {
    transform: scale(0.97);
  }
  .nav svg {
    width: 18px;
    height: 18px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .nav.active svg {
    stroke: var(--accent-text);
  }
  .nav em {
    margin-inline-start: auto;
    font-style: normal;
    font-size: 10px;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1px 5px;
  }
  .content {
    flex: 1;
    overflow-y: auto;
    padding: 26px 30px 90px;
    display: grid;
  }
  .content > section {
    grid-area: 1 / 1;
  }
  h1 {
    margin: 0 0 14px;
    font-size: 22px;
    font-weight: 800;
  }
  .lead {
    margin: -6px 0 16px;
    color: var(--text-dim);
    font-size: 13px;
  }
  .card {
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
    padding: 6px 18px;
    margin-bottom: 16px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 18px;
    padding: 14px 0;
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  }
  .row:last-child {
    border-bottom: none;
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
  .recorder-cell {
    text-align: end;
  }
  .key-row {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .field.small {
    width: 170px;
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
  .mini:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .badge {
    font-size: 10.5px;
    font-weight: 600;
    padding: 1px 8px;
    border-radius: 999px;
    color: var(--text-dim);
    background: rgba(255, 255, 255, 0.06);
  }
  .badge.has {
    color: #a7f3d0;
    background: rgba(52, 211, 153, 0.1);
  }
  .field {
    width: 240px;
    padding: 9px 12px;
    border-radius: 11px;
    border: 1px solid var(--border);
    background: rgba(0, 0, 0, 0.28);
    box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.45), 0 1px 0 rgba(255, 255, 255, 0.06);
    color: var(--text);
    font-size: 13px;
    outline: none;
    transition: border-color var(--fast), box-shadow var(--fast);
  }
  .field:focus {
    border-color: var(--accent-text);
    box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.45), 0 0 0 3px var(--glow-soft);
  }
  select.field option {
    background: #1b1b22;
  }
  .sub {
    margin: 4px 4px 10px;
    font-size: 15px;
    font-weight: 700;
    color: var(--text-muted);
  }
  .link {
    border: none;
    background: none;
    color: var(--accent-text);
    cursor: pointer;
    font-size: 12.5px;
  }
  .preview {
    position: relative;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    height: 190px;
    margin-bottom: 16px;
    border-radius: 16px;
    overflow: hidden;
    background: radial-gradient(90% 130% at 50% 0%, color-mix(in srgb, var(--accent) 16%, transparent), transparent 70%),
      linear-gradient(135deg, rgba(255, 255, 255, 0.07), rgba(255, 255, 255, 0.02));
    box-shadow: var(--card-shadow);
    direction: ltr;
  }
  .themes {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
    margin-bottom: 16px;
  }
  .theme {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px;
    border-radius: 14px;
    border: 1px solid transparent;
    background: var(--surface);
    box-shadow: var(--card-shadow);
    backdrop-filter: var(--glass-blur);
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: transform var(--medium) var(--ease-spring), border-color var(--fast), color var(--fast), box-shadow var(--fast);
  }
  .theme:hover {
    transform: translateY(-3px) scale(1.02);
    color: var(--text);
  }
  .theme.selected {
    border-color: var(--accent-text);
    color: var(--text);
    box-shadow: var(--card-shadow), 0 0 0 3px var(--glow-soft), 0 14px 30px -16px var(--glow);
  }
  .swatch {
    height: 36px;
    border-radius: 10px;
  }
  .pickers {
    display: flex;
    gap: 8px;
  }
  .pickers input {
    width: 38px;
    height: 32px;
    border: none;
    border-radius: 8px;
    background: none;
    cursor: pointer;
  }
  .mono {
    font-family: var(--mono);
    direction: ltr;
  }
  .small {
    font-size: 11px;
    color: var(--text-dim);
    max-width: 420px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .soon {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 46px 18px;
    gap: 6px;
  }
  .soon-orb {
    width: 54px;
    height: 54px;
    border-radius: 50%;
    margin-bottom: 12px;
    background: conic-gradient(from 0deg, var(--accent), var(--accent-2), var(--accent-3), var(--accent));
    filter: blur(1px);
    animation: spin 6s linear infinite;
    box-shadow: 0 0 30px var(--glow);
  }
  .savebar {
    position: absolute;
    bottom: 16px;
    left: 50%;
    translate: -50% 0;
    z-index: 5;
    display: flex;
    align-items: center;
    gap: 18px;
    padding: 10px 12px 10px 18px;
    border-radius: 18px;
    background: var(--shine), linear-gradient(180deg, rgba(60, 66, 92, 0.72), rgba(16, 18, 28, 0.82));
    box-shadow: 0 24px 48px -16px rgba(0, 0, 0, 0.85), inset 0 1px 0 rgba(255, 255, 255, 0.3), inset 0 0 0 1px rgba(255, 255, 255, 0.08);
    backdrop-filter: blur(26px) saturate(180%);
    font-size: 13px;
    font-weight: 600;
  }
  .actions {
    display: flex;
    gap: 8px;
  }
  .btn {
    padding: 8px 18px;
    border-radius: 11px;
    border: none;
    font-size: 13px;
    font-weight: 700;
    cursor: pointer;
    transition: transform var(--medium) var(--ease-spring), opacity var(--fast), box-shadow var(--fast);
  }
  .btn:hover:not(:disabled) {
    transform: translateY(-1px);
  }
  .btn:active {
    transform: scale(0.96);
  }
  .btn:disabled {
    opacity: 0.5;
  }
  .primary {
    background: linear-gradient(180deg, #fff, #dfe5f0);
    color: #000;
    box-shadow: 0 8px 20px -10px var(--glow), inset 0 -1px 0 rgba(0, 0, 0, 0.15);
  }
  .ghost {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>