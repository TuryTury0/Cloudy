<script lang="ts">
  // Chat window: the whole conversation (from memory), live progress of the running request, and a text box.
  // Requests sent from the island show up here too, because every window reads the same core events.
  import { onMount, tick } from 'svelte';
  import { fade, fly, scale } from 'svelte/transition';
  import { backOut, cubicOut } from 'svelte/easing';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { Aurora, BotFace, L, applyMotion, applyTheme, dir, friendlyError, normalizeTheme, setLanguage, trackShine, type IslandStep } from '@anytask/ui';
  import ChatBubble from '../components/ChatBubble.svelte';
  import { inTauri, invoke, listen } from '../lib/ipc';
  import { DEFAULT_SETTINGS, type BubbleStatus, type CoreEvent, type Settings, type Turn } from '../lib/types';

  interface Item {
    key: string;
    role: 'user' | 'assistant';
    text: string;
    status: BubbleStatus;
    steps: IslandStep[];
    animate: boolean;
    at: number;
    turnId?: number;
    needsModel?: boolean;
    /** The technical reason under a friendly error. */
    detail?: string;
  }

  function errorFields(message: string): Pick<Item, 'text' | 'detail' | 'needsModel'> {
    const f = friendlyError(message);
    return { text: f.text, detail: f.detail !== f.text ? f.detail : '', needsModel: f.needsModel };
  }

  const SUGGESTIONS = $derived([
    L("What's the weather today?", 'قیمت دلار امروز چنده؟'),
    L('Show the files on my Desktop', 'فایل‌های دسکتاپ رو نشون بده'),
    L('Next song', 'آهنگ بعدی'),
    L('Remember that my editor is VS Code', 'یادت باشه ویرایشگر من VS Code ـه')
  ]);

  let settings = $state<Settings>(DEFAULT_SETTINGS);
  let items = $state<Item[]>([]);
  let draft = $state('');
  let busyTurn = $state<number | null>(null);
  let model = $state<{ label: string; ok: boolean } | null>(null);
  let loaded = $state(false);
  let scroller = $state<HTMLDivElement>();
  let content = $state<HTMLDivElement>();
  let box = $state<HTMLTextAreaElement>();
  let stickToBottom = true;

  const busy = $derived(busyTurn !== null);
  const activeModel = $derived(settings.models.find((m) => m.id === settings.activeModel) ?? settings.models[0]);
  const win = inTauri ? getCurrentWindow() : null;

  function fromHistory(turns: Turn[]): Item[] {
    return turns.flatMap((t) => {
      const failed = t.assistant.startsWith('(failed:');
      const stopped = t.assistant === '(stopped by the user)';
      const steps: IslandStep[] = t.actions.map((label, i) => ({
        id: `h${t.id}:${i}`,
        label: label.replace(/ \(failed\)$/, ''),
        status: label.endsWith('(failed)') ? 'failed' : 'done'
      }));
      return [
        { key: `u${t.id}`, role: 'user', text: t.user, status: 'done', steps: [], animate: false, at: t.at },
        {
          key: `a${t.id}`,
          role: 'assistant',
          text: stopped ? L('Stopped', 'متوقف شد') : t.assistant,
          ...(failed ? errorFields(t.assistant.slice(9, -1)) : {}),
          status: failed ? 'error' : stopped ? 'stopped' : 'done',
          steps,
          animate: false,
          at: t.at
        }
      ] satisfies Item[];
    });
  }

  /** Loads the conversation from memory (also used when a request started before this window opened). */
  async function loadHistory() {
    const turns = (await invoke<Turn[]>('conversation', { limit: 80 })) ?? [];
    // Keep live turns that memory does not have (still running, or failed without being stored) and loose errors.
    const stored = new Set(turns.map((t) => t.user));
    const unsaved = new Set(items.filter((i) => i.role === 'user' && i.turnId !== undefined && !stored.has(i.text)).map((i) => i.turnId));
    const keep = items.filter((i) => (i.turnId !== undefined ? unsaved.has(i.turnId) : i.key.startsWith('e')));
    items = [...fromHistory(turns), ...keep];
    loaded = true;
    await tick();
    scroller?.scrollTo({ top: scroller.scrollHeight });
  }

  function pendingFor(turnId: number): Item | undefined {
    return items.find((i) => i.turnId === turnId && i.role === 'assistant');
  }

  function onCore(event: CoreEvent) {
    switch (event.kind) {
      case 'turnStarted':
        busyTurn = event.turnId;
        items.push(
          { key: `lu${event.turnId}`, role: 'user', text: event.user, status: 'done', steps: [], animate: false, at: Date.now(), turnId: event.turnId },
          { key: `la${event.turnId}`, role: 'assistant', text: '', status: 'pending', steps: [], animate: true, at: Date.now(), turnId: event.turnId }
        );
        stickToBottom = true;
        break;
      case 'toolStarted':
        pendingFor(event.turnId)?.steps.push({ id: event.callId, label: event.label, status: 'running' });
        break;
      case 'toolProgress': {
        const item = pendingFor(event.turnId);
        if (!item || event.text.endsWith('…')) break;
        for (const s of item.steps) if (s.id.startsWith(`${event.callId}:`) && s.status === 'running') s.status = 'done';
        item.steps.push({ id: `${event.callId}:${item.steps.length}`, label: event.text, status: 'running' });
        break;
      }
      case 'toolFinished': {
        const item = pendingFor(event.turnId);
        const status = event.ok ? 'done' : 'failed';
        for (const s of item?.steps ?? []) if (s.id === event.callId || (s.id.startsWith(`${event.callId}:`) && s.status === 'running')) s.status = status;
        break;
      }
      case 'answer': {
        const item = pendingFor(event.turnId);
        if (item) {
          item.status = 'done';
          item.text = event.text;
          item.at = Date.now();
        } else {
          // The request began before this window opened: the answer is in memory now.
          void loadHistory();
        }
        if (busyTurn === event.turnId) busyTurn = null;
        break;
      }
      case 'error': {
        const item = event.turnId === null ? undefined : pendingFor(event.turnId);
        if (item) {
          item.status = 'error';
          Object.assign(item, errorFields(event.message));
        } else {
          items.push({ key: `e${Date.now()}`, role: 'assistant', status: 'error', steps: [], animate: false, at: Date.now(), ...errorFields(event.message) });
        }
        if (event.turnId === null || busyTurn === event.turnId) busyTurn = null;
        stickToBottom = true;
        break;
      }
      case 'cancelled': {
        const item = pendingFor(event.turnId);
        if (item) {
          item.status = 'stopped';
          item.text = L('Stopped', 'متوقف شد');
        }
        if (busyTurn === event.turnId) busyTurn = null;
        break;
      }
      case 'modelUsed':
        model = { label: event.label, ok: event.ok };
        break;
    }
  }

  async function send(text = draft) {
    const value = text.trim();
    if (!value) return;
    // A new message joins the conversation (Live, or the brain, which replaces a running request by itself).
    draft = '';
    await tick();
    autosize();
    void invoke('ask', { text: value }).catch(() => {
      // The reason arrives as an error event and is shown as a bubble.
    });
  }

  function onKey(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      void send();
    } else if (event.key === 'Escape' && busy) {
      void invoke('stop_turn');
    }
  }

  function autosize() {
    if (!box) return;
    box.style.height = 'auto';
    box.style.height = `${Math.min(140, box.scrollHeight)}px`;
  }

  function onScroll() {
    if (!scroller) return;
    stickToBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 60;
  }

  function applySettings(next: Settings) {
    settings = next;
    applyTheme(normalizeTheme(next.theme));
    applyMotion(next.reduceMotion);
    setLanguage(next.language);
  }

  // Design work in a plain browser: ?view=chat&preview=1&lang=fa shows a sample conversation.
  function preview() {
    const params = new URLSearchParams(location.search);
    setLanguage(params.get('lang'));
    loaded = true;
    if (!params.get('preview')) return;
    const at = Date.now();
    const base = { animate: false, at, steps: [] as IslandStep[] };
    items = [
      { ...base, key: 'p1', role: 'user', status: 'done', text: L('Play Believer on Spotify', 'Believer رو تو Spotify پخش کن') },
      {
        ...base,
        key: 'p2',
        role: 'assistant',
        status: 'done',
        text: L('**Believer** by Imagine Dragons is playing now.', '**Believer** از Imagine Dragons الان پخش می‌شود.'),
        steps: [
          { id: 's1', label: L('Opening Spotify', 'باز کردن Spotify'), status: 'done' },
          { id: 's2', label: L('Playing the song', 'پخش آهنگ'), status: 'done' }
        ]
      },
      { ...base, key: 'p3', role: 'user', status: 'done', text: L('What is the dollar price today?', 'قیمت دلار امروز چنده؟') },
      { ...base, key: 'p4', role: 'assistant', status: 'pending', text: '', steps: [{ id: 's3', label: L('Reading tgju.org', 'خواندن tgju.org'), status: 'running' }] }
    ];
    busyTurn = 1;
  }

  onMount(() => {
    // The window is shown only once this page has loaded (see window_ready in commands.rs).
    void invoke('window_ready');
    const unlisten: Array<() => void> = [];
    const on = <T,>(name: string, handler: (payload: T) => void) => void listen<T>(name, handler).then((u) => unlisten.push(u));
    on<CoreEvent>('core-event', onCore);
    on<Settings>('settings-changed', applySettings);
    void invoke<Settings>('get_settings').then((s) => s && applySettings(s));
    applyTheme(normalizeTheme(settings.theme));
    if (inTauri) void loadHistory();
    else preview();

    // Keep the newest message in view while answers type in, unless the user scrolled up to read.
    const observer = new ResizeObserver(() => {
      if (stickToBottom && scroller) scroller.scrollTo({ top: scroller.scrollHeight, behavior: 'smooth' });
    });
    if (content) observer.observe(content);
    box?.focus();
    const stopShine = trackShine();
    return () => {
      stopShine();
      observer.disconnect();
      unlisten.forEach((u) => u());
    };
  });
</script>

<div class="app-window" dir={dir()}>
  <div class="glow" class:busy><Aurora colors={settings.theme.colors} intensity={busy ? 0.55 : 0.22} speed={busy ? 2 : 0.8} /></div>

  <header class="titlebar" data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region>
      <BotFace mode={busy ? 'thinking' : 'idle'} size={22} />
      <span data-tauri-drag-region>{settings.assistantName}</span>
    </div>
    <div class="model" title={model?.label ?? activeModel?.modelId ?? ''}>
      <span class="led" class:ok={!!activeModel && model?.ok !== false} class:bad={model?.ok === false}></span>
      <span>{activeModel ? activeModel.name : L('No model', 'بدون مدل')}</span>
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

  <div class="scroller" bind:this={scroller} onscroll={onScroll}>
    <div class="list" bind:this={content}>
      {#if loaded && items.length === 0}
        <div class="empty" in:fade={{ duration: 400 }}>
          <div class="orb" in:scale={{ start: 0.5, duration: 700, easing: backOut }}>
            <BotFace mode="idle" size={64} />
          </div>
          <h2 in:fly={{ y: 10, duration: 400, delay: 150 }}>{L('Hi', 'سلام')}{settings.userName ? ` ${settings.userName}` : ''}!</h2>
          <p in:fly={{ y: 10, duration: 400, delay: 220 }}>{L('What can I do? Ask anything or give me a task.', 'چی کار کنم؟ هر چیزی بپرس یا یه کار بخواه.')}</p>
          <div class="suggestions">
            {#each SUGGESTIONS as s, i (s)}
              <button class="suggestion" onclick={() => send(s)} in:fly={{ y: 12, duration: 420, delay: 300 + i * 70, easing: cubicOut }}>{s}</button>
            {/each}
          </div>
        </div>
      {/if}
      {#each items as item (item.key)}
        <ChatBubble
          role={item.role}
          text={item.text}
          status={item.status}
          steps={item.steps}
          animate={item.animate}
          at={item.at}
          detail={item.detail}
          onopenmodels={item.needsModel ? () => void invoke('run_action', { action: 'settings:models' }) : undefined}
        />
      {/each}
    </div>
  </div>

  <footer class="composer">
    <div class="field" class:busy>
      <textarea
        bind:this={box}
        bind:value={draft}
        rows="1"
        dir="auto"
        placeholder={L('Write… (Enter to send, Shift+Enter for a new line)', 'بنویس… (Enter برای ارسال، Shift+Enter خط جدید)')}
        oninput={autosize}
        onkeydown={onKey}
      ></textarea>
      {#key busy && !draft.trim()}
        {#if busy && !draft.trim()}
          <button class="send stop" aria-label="stop" onclick={() => invoke('stop_turn')} in:scale={{ start: 0.5, duration: 260, easing: backOut }}>
            <span class="square"></span>
          </button>
        {:else}
          <button class="send" aria-label="send" disabled={!draft.trim()} onclick={() => send()} in:scale={{ start: 0.5, duration: 260, easing: backOut }}>
            <svg viewBox="0 0 24 24"><path d="M12 19V5M5 12l7-7 7 7" /></svg>
          </button>
        {/if}
      {/key}
    </div>
  </footer>
</div>

<style>
  .glow {
    position: absolute;
    inset: 0 0 auto;
    height: 240px;
    z-index: -1;
    opacity: 0.6;
    -webkit-mask: linear-gradient(#000, transparent);
    mask: linear-gradient(#000, transparent);
    transition: opacity var(--slow);
    pointer-events: none;
  }
  .glow.busy {
    opacity: 0.9;
  }
  .titlebar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 48px;
    padding: 0 12px 0 10px;
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.06), transparent);
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: 13.5px;
    font-weight: 800;
  }
  .model {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-inline-start: auto;
    padding: 4px 10px;
    border-radius: 999px;
    background: var(--surface);
    box-shadow: var(--card-shadow);
    backdrop-filter: var(--glass-blur);
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    max-width: 180px;
    white-space: nowrap;
    overflow: hidden;
  }
  .led {
    width: 7px;
    height: 7px;
    flex-shrink: 0;
    border-radius: 50%;
    background: var(--text-dim);
  }
  .led.ok {
    background: var(--success);
    box-shadow: 0 0 8px var(--success);
  }
  .led.bad {
    background: var(--danger);
    box-shadow: 0 0 8px var(--danger);
  }
  .controls {
    display: flex;
    gap: 4px;
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
  .scroller {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 18px 16px 24px;
    min-height: 100%;
  }
  .empty {
    margin: auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 6px;
    padding: 20px;
  }
  .orb {
    display: grid;
    place-items: center;
    width: 112px;
    height: 112px;
    margin-bottom: 10px;
    border-radius: 50%;
    background: radial-gradient(circle, var(--glow-soft), transparent 70%);
    animation: float 4s ease-in-out infinite;
  }
  .empty h2 {
    margin: 0;
    font-size: 20px;
    font-weight: 800;
  }
  .empty p {
    margin: 0 0 16px;
    color: var(--text-dim);
    font-size: 13px;
  }
  .suggestions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
    max-width: 380px;
  }
  .suggestion {
    padding: 8px 14px;
    border-radius: 999px;
    border: none;
    background: var(--surface);
    box-shadow: var(--card-shadow);
    backdrop-filter: var(--glass-blur);
    color: var(--text-muted);
    font-size: 12.5px;
    cursor: pointer;
    transition: transform var(--medium) var(--ease-spring), background var(--fast), color var(--fast), box-shadow var(--fast);
  }
  .suggestion:hover {
    transform: translateY(-2px) scale(1.03);
    background: var(--surface-hover);
    color: var(--text);
    box-shadow: var(--card-shadow), 0 0 0 1px color-mix(in srgb, var(--accent-text) 55%, transparent), 0 10px 26px -12px var(--glow);
  }
  .composer {
    padding: 10px 14px 14px;
  }
  .field {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 6px 6px 6px 6px;
    padding-inline-start: 14px;
    border-radius: 22px;
    background: var(--surface);
    border: 1px solid transparent;
    box-shadow: var(--card-shadow), inset 0 0 0 1px rgba(255, 255, 255, 0.07);
    backdrop-filter: var(--glass-blur);
    transition: border-color var(--fast), box-shadow var(--medium);
  }
  .field:focus-within {
    border-color: color-mix(in srgb, var(--accent-text) 60%, transparent);
    box-shadow: var(--card-shadow), 0 0 0 4px var(--glow-soft), 0 12px 30px -16px var(--glow);
  }
  .field.busy {
    animation: breathe 2.4s ease-in-out infinite;
  }
  textarea {
    flex: 1;
    min-height: 36px;
    max-height: 140px;
    padding: 8px 0;
    border: none;
    outline: none;
    resize: none;
    background: transparent;
    color: var(--text);
    font-size: 13.5px;
    line-height: 1.6;
  }
  textarea::placeholder {
    color: var(--text-dim);
  }
  .send {
    width: 36px;
    height: 36px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    border: none;
    border-radius: 50%;
    background: #fff;
    cursor: pointer;
    transition: transform var(--fast) var(--ease-spring), opacity var(--fast), box-shadow var(--fast);
  }
  .send:hover:not(:disabled) {
    transform: scale(1.06);
    box-shadow: 0 0 18px var(--glow);
  }
  .send:disabled {
    opacity: 0.25;
    cursor: default;
  }
  .send svg {
    width: 18px;
    height: 18px;
    fill: none;
    stroke: #000;
    stroke-width: 2.4;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .send.stop {
    background: rgba(255, 255, 255, 0.12);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.2);
  }
  .square {
    width: 12px;
    height: 12px;
    border-radius: 3px;
    background: var(--text);
  }
  @keyframes float {
    0%,
    100% {
      transform: translateY(0);
    }
    50% {
      transform: translateY(-6px);
    }
  }
  @keyframes breathe {
    0%,
    100% {
      box-shadow: 0 0 0 0 var(--glow-soft);
    }
    50% {
      box-shadow: 0 0 0 5px var(--glow-soft);
    }
  }
</style>
