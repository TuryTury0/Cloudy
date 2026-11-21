<script lang="ts">
  // The island window: follows the assistant through core events (thinking, live tool steps, answer card),
  // asks before risky steps, and reports its clickable surfaces so everything else stays click-through.
  import { onMount } from 'svelte';
  import { fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
  import { Island, L, applyMotion, applyTheme, autoReturnMs, friendlyError, normalizeTheme, setLanguage, type AssistantState, type IslandStep } from '@anytask/ui';
  import { inTauri, invoke, listen } from '../lib/ipc';
  import { trackHitRegions } from '../lib/hit-regions';
  import type { ConfirmRequest, CoreEvent, HotkeyProblem, Settings } from '../lib/types';
  import { DEFAULT_SETTINGS } from '../lib/types';

  interface Toast {
    id: number;
    text: string;
    kind: 'info' | 'error';
  }

  let mode = $state<AssistantState>('idle');
  let detail = $state('');
  let steps = $state<IslandStep[]>([]);
  let answer = $state('');
  /** What the user asked in the running (or just answered) request. */
  let request = $state('');
  let confirm = $state<ConfirmRequest | null>(null);
  let busy = $state(false);
  let level = $state(0);
  /** A voice conversation is open (microphone on). */
  let voiceOn = $state(false);
  let settings = $state<Settings>(DEFAULT_SETTINGS);
  let toasts = $state<Toast[]>([]);

  let hovered = false;
  let returnTimer: ReturnType<typeof setTimeout> | undefined;
  let demoRun = 0;
  let toastId = 0;

  function setState(next: AssistantState, nextDetail = '') {
    mode = next;
    detail = nextDetail;
    if (next !== 'working' && next !== 'thinking') steps = [];
    // A new request begins: the last one's text must not show above it.
    if (next === 'listening' || next === 'text-input') request = '';
    scheduleReturn();
    // Only the text box needs the keyboard; give mouse and focus back otherwise.
    if (next !== 'text-input') void invoke('island_capture', { capture: false });
  }

  /** Transient states go back to idle by themselves, but never while the pointer is on the island. */
  function scheduleReturn(delay?: number) {
    clearTimeout(returnTimer);
    const back = delay ?? autoReturnMs(mode, mode === 'answer' ? answer : detail);
    if (back && !hovered && !confirm) returnTimer = setTimeout(() => setState('idle'), back);
  }

  function onHover(value: boolean) {
    hovered = value;
    if (value) clearTimeout(returnTimer);
    else if (autoReturnMs(mode)) scheduleReturn(2200);
  }

  function toast(text: string, kind: Toast['kind'] = 'info', ms = 4200) {
    const id = ++toastId;
    toasts = [...toasts, { id, text, kind }];
    setTimeout(() => (toasts = toasts.filter((t) => t.id !== id)), ms);
  }

  function applySettings(next: Settings) {
    settings = next;
    applyTheme(normalizeTheme(next.theme));
    applyMotion(next.reduceMotion);
    setLanguage(next.language);
  }

  /** When the chat window is in front, the answer is read there; the island only nods. */
  async function chatInFront(): Promise<boolean> {
    if (!inTauri) return false;
    try {
      const chat = await WebviewWindow.getByLabel('chat');
      return !!chat && (await chat.isVisible()) && (await chat.isFocused());
    } catch {
      return false;
    }
  }

  async function onCore(event: CoreEvent) {
    switch (event.kind) {
      case 'turnStarted':
        demoRun++;
        busy = true;
        steps = [];
        answer = '';
        request = event.user;
        break;
      case 'toolStarted':
        steps = [...steps, { id: `${event.turnId}:${event.callId}`, label: event.label, status: 'running' }];
        break;
      case 'toolProgress': {
        // Status lines (ending in …) update the line under the title; actions become sub-steps.
        if (event.text.endsWith('…')) {
          if (mode === 'working') detail = event.text;
          break;
        }
        const parent = `${event.turnId}:${event.callId}`;
        const done = steps.map((s) => (s.id.startsWith(`${parent}:`) && s.status === 'running' ? { ...s, status: 'done' as const } : s));
        steps = [...done, { id: `${parent}:${done.length}`, label: event.text, status: 'running' }];
        break;
      }
      case 'toolFinished': {
        const id = `${event.turnId}:${event.callId}`;
        const status = event.ok ? 'done' : 'failed';
        steps = steps.map((s) => (s.id === id || (s.id.startsWith(`${id}:`) && s.status === 'running') ? { ...s, status } : s));
        break;
      }
      case 'answer':
        answer = event.text;
        break;
      case 'state':
        if (mode === 'text-input') {
          // The user is typing their next request: the running one's states must not close the text box under
          // their fingers (11-08: a second request typed while the first was answered was lost). Its answer still
          // reaches the chat; the island shows the new request once it is sent.
          if (event.state === 'answer' || event.state === 'error' || event.state === 'idle') busy = false;
          if (event.state === 'answer') answer = event.detail ?? answer;
          break;
        }
        if (event.state === 'answer') {
          busy = false;
          answer = event.detail ?? answer;
          if (await chatInFront()) setState('success', '');
          else setState('answer');
        } else {
          if (event.state === 'error' || event.state === 'idle') busy = false;
          setState(event.state, event.state === 'error' ? friendlyError(event.detail ?? '').text : (event.detail ?? ''));
        }
        break;
    }
  }

  // Design work in a plain browser: ?view=island&preview=<state>&lang=fa shows one state with sample content.
  function preview() {
    const params = new URLSearchParams(location.search);
    const state = params.get('preview');
    setLanguage(params.get('lang'));
    if (!state) return;
    const ask = L('What is the dollar price today?', 'قیمت دلار امروز چنده؟');
    const sampleSteps: IslandStep[] = [
      { id: 'p1', label: L('Searching the web', 'جستجو در وب'), status: 'done' },
      { id: 'p2', label: L('Reading tgju.org', 'خواندن tgju.org'), status: 'done' },
      { id: 'p3', label: L('Checking the result', 'بررسی نتیجه'), status: 'running' }
    ];
    hovered = true;
    switch (state) {
      case 'hover':
        break;
      case 'steps':
        request = ask;
        steps = sampleSteps;
        setState('working');
        steps = sampleSteps;
        break;
      case 'answer':
        request = ask;
        answer = L('The dollar is **1,125,400** rials right now, 0.4% above yesterday.\nThe day high was 1,131,000.', 'دلار الان **۱٬۱۲۵٬۴۰۰** ریال است، ۰٫۴٪ بالاتر از دیروز.\nبیشترین قیمت امروز ۱٬۱۳۱٬۰۰۰ بود.');
        setState('answer');
        break;
      case 'confirm':
        setState('working');
        confirm = { id: 1, title: 'Overwrite this file?', detail: 'C:/Users/me/Desktop/notes.txt' } as ConfirmRequest;
        break;
      case 'error':
        setState('error', friendlyError('Gemini: HTTP 429: quota').text);
        break;
      case 'listening':
      case 'speaking':
        level = 0.6;
        setState(state, params.get('text') ?? '');
        break;
      default:
        setState(state as AssistantState, state === 'thinking' ? ask : '');
    }
  }

  const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

  // Walks through every state so the animations can be seen (tray menu > Animation demo).
  async function demo() {
    const run = ++demoRun;
    const alive = () => run === demoRun;
    const levelTimer = setInterval(() => (level = 0.25 + 0.5 * Math.abs(Math.sin(Date.now() / 260)) * Math.random()), 80);
    try {
      setState('listening', L('Play the new Shakira song', 'ahang-e jadid-e Shakira ro play kon'));
      await sleep(2600);
      if (!alive()) return;
      request = detail;
      setState('thinking');
      await sleep(1600);
      if (!alive()) return;
      setState('working', 'Spotify');
      const labels = ['Opening Spotify', 'Searching "Shakira"', 'Clicking #12 "Play"', 'Checking the result'];
      for (const [i, label] of labels.entries()) {
        steps = [...steps.map((s) => ({ ...s, status: 'done' as const })), { id: `demo:${i}`, label, status: 'running' }];
        await sleep(900);
        if (!alive()) return;
      }
      steps = steps.map((s) => ({ ...s, status: 'done' }));
      await sleep(500);
      setState('speaking', L('Playing it now', 'alan dare play mishe'));
      await sleep(2400);
      if (!alive()) return;
      answer = L('**Waka Waka** by Shakira is now playing in Spotify.\nSay "next" for the next song.', 'آهنگ **Waka Waka** از Shakira الان در Spotify پخش می‌شود.\nبرای آهنگ بعدی بگو «بعدی».');
      setState('answer');
    } finally {
      clearInterval(levelTimer);
      level = 0;
    }
  }

  // A job runs (a tool, a screen task): the Stop button shows under the island.
  const working = $derived(steps.some((s) => s.status === 'running'));
  // A tool's label is both the working line and its step: shown once, as the step ("خواندن www.tgju.org" twice).
  const shownDetail = $derived(mode === 'working' && steps.some((s) => s.label === detail) ? '' : detail);

  function stopTask() {
    void invoke('stop_task');
  }

  function stopAll() {
    demoRun++;
    if (busy || voiceOn) void invoke('stop_turn');
    setState('idle');
  }

  function onAction(action: string) {
    switch (action) {
      case 'type':
        demoRun++;
        setState('text-input');
        break;
      case 'stop':
        stopAll();
        break;
      case 'demo':
        void demo();
        break;
    }
  }

  function submit(text: string) {
    setState('thinking');
    request = text;
    void invoke('ask', { text }).catch(() => {
      // Failures arrive as core events (error state with the reason); nothing more to do here.
    });
  }

  function activate() {
    if (mode === 'idle') onAction('type');
    // A click while a task runs only shows it; stopping is Esc or the Stop shortcut, so a look never ends the work.
    else if (voiceOn) stopAll();
    else if (busy) return;
    else setState('idle');
  }

  function answerConfirm(allow: boolean) {
    if (!confirm) return;
    void invoke('answer_confirm', { id: confirm.id, allow });
    confirm = null;
    scheduleReturn();
  }

  onMount(() => {
    // The window is shown only once this page has loaded (see window_ready in commands.rs).
    void invoke('window_ready');
    const stopTracking = trackHitRegions();
    const unlisten: Array<() => void> = [];
    const on = <T,>(name: string, handler: (payload: T) => void) => void listen<T>(name, handler).then((u) => unlisten.push(u));
    on<string>('action', onAction);
    on<Settings>('settings-changed', applySettings);
    on<string>('notice', (text) => toast(text, 'error', 7000));
    on<string>('reminder', (text) => toast(text, 'info', 20000));
    on<boolean>('voice-session', (on) => {
      demoRun++;
      voiceOn = on;
      if (!on) level = 0;
    });
    on<{ mic: number; speaker: number }>('voice-level', (l) => {
      if (voiceOn) level = mode === 'speaking' ? l.speaker : l.mic;
    });
    on<CoreEvent>('core-event', (e) => void onCore(e));
    on<ConfirmRequest>('confirm-request', (request) => {
      clearTimeout(returnTimer);
      confirm = request;
    });
    on<number>('confirm-closed', (id) => {
      if (confirm?.id === id) {
        confirm = null;
        scheduleReturn();
      }
    });
    on<HotkeyProblem[]>('hotkey-problems', (problems) => {
      for (const p of problems) toast(`${p.shortcut}: ${p.reason}`, 'error', 7000);
    });
    void invoke<Settings>('get_settings').then((s) => s && applySettings(s));
    applyTheme(normalizeTheme(settings.theme));
    if (!inTauri) preview();

    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') stopAll();
    };
    // While the text box is open the whole (transparent) island window takes the mouse. Close it as soon as the
    // user goes to another window or clicks beside the island, so the screen under it never stays blocked.
    const closeTextBox = () => {
      if (mode === 'text-input') setState('idle');
    };
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Element && event.target.closest('[data-hit]'))) closeTextBox();
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('blur', closeTextBox);
    window.addEventListener('pointerdown', onPointerDown);
    return () => {
      stopTracking();
      unlisten.forEach((u) => u());
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('blur', closeTextBox);
      window.removeEventListener('pointerdown', onPointerDown);
    };
  });
</script>

<div class="stage">
  <Island
    {mode}
    name={settings.assistantName}
    detail={shownDetail}
    {steps}
    {level}
    {answer}
    {request}
    {confirm}
    colors={settings.theme.colors}
    onsubmit={submit}
    oncancel={() => setState('idle')}
    onactivate={activate}
    onconfirm={answerConfirm}
    ondismiss={() => setState('idle')}
    onhover={onHover}
    onfollowup={() => onAction('type')}
  />

  {#if working}
    <button class="stop" data-hit onclick={stopTask} in:fly={{ y: -8, duration: 220, easing: cubicOut }} out:fly={{ y: -6, duration: 160 }}>
      <span class="square"></span>{L('Stop', 'توقف')}
    </button>
  {/if}

  <div class="toasts">
    {#each toasts as t (t.id)}
      <div class="toast {t.kind}" data-hit dir="auto" in:fly={{ y: -10, duration: 260, easing: cubicOut }} out:fly={{ y: -6, duration: 180 }}>
        {t.text}
      </div>
    {/each}
  </div>
</div>

<style>
  .stage {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }
  .stop,
  .toast {
    background: rgba(2, 2, 4, 0.94);
    box-shadow: 0 10px 28px -10px rgba(0, 0, 0, 0.7), inset 0 0 0 1px rgba(255, 255, 255, 0.09), inset 0 1px 0 rgba(255, 255, 255, 0.08);
  }
  .stop {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 15px;
    border: none;
    border-radius: 999px;
    font: inherit;
    font-size: 12px;
    font-weight: 700;
    color: #fecaca;
    cursor: pointer;
    transition: transform var(--medium) var(--ease-spring), box-shadow var(--fast);
  }
  .stop:hover {
    transform: scale(1.06);
    box-shadow: 0 10px 28px -10px rgba(0, 0, 0, 0.7), inset 0 0 0 1px rgba(248, 113, 113, 0.7), 0 0 22px -6px rgba(248, 113, 113, 0.6);
  }
  .stop:active {
    transform: scale(0.96);
  }
  .square {
    width: 9px;
    height: 9px;
    border-radius: 2.5px;
    background: #f87171;
    box-shadow: 0 0 10px rgba(248, 113, 113, 0.7);
    animation: pulse 1.4s ease-in-out infinite;
  }
  .toasts {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
  }
  .toast {
    max-width: 520px;
    padding: 9px 16px;
    border-radius: 16px;
    font-size: 12.5px;
    font-weight: 600;
    line-height: 1.5;
    color: rgba(255, 255, 255, 0.92);
  }
  .toast.error {
    color: #fecaca;
    box-shadow: 0 10px 28px -10px rgba(0, 0, 0, 0.7), inset 0 0 0 1px rgba(248, 113, 113, 0.45);
  }
  @keyframes pulse {
    50% {
      opacity: 0.55;
    }
  }
</style>
