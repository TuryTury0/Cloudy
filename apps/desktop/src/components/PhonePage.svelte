<script lang="ts">
  // Settings > Phone: turn the phone connection on, pair a phone with a QR code, see and remove paired phones.
  import { onMount } from 'svelte';
  import { fade, fly, scale, slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { L, num } from '@anytask/ui';
  import { invoke, listen } from '../lib/ipc';
  import type { PairingView, PhoneStatus } from '../lib/types';
  import Toggle from './Toggle.svelte';

  let status = $state<PhoneStatus | null>(null);
  let pairing = $state<PairingView | null>(null);
  let left = $state(0);
  let error = $state('');
  let busy = $state(false);
  let copied = $state(false);
  let confirmForget = $state('');
  let timer: ReturnType<typeof setInterval> | undefined;

  const ago = (ms: number) => {
    const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
    if (s < 60) return L('just now', 'همین الان');
    if (s < 3600) return L(`${Math.round(s / 60)} minutes ago`, `${num(Math.round(s / 60))} دقیقه پیش`);
    if (s < 86400) return L(`${Math.round(s / 3600)} hours ago`, `${num(Math.round(s / 3600))} ساعت پیش`);
    return L(`${Math.round(s / 86400)} days ago`, `${num(Math.round(s / 86400))} روز پیش`);
  };

  async function setEnabled(enabled: boolean) {
    busy = true;
    error = '';
    status = (await invoke<PhoneStatus>('phone_set_enabled', { enabled })) ?? status;
    busy = false;
    if (!enabled) closePairing();
  }

  async function showPairing() {
    error = '';
    try {
      pairing = (await invoke<PairingView>('phone_pairing')) ?? null;
      left = (pairing?.minutes ?? 10) * 60;
      clearInterval(timer);
      timer = setInterval(() => {
        left -= 1;
        if (left <= 0) closePairing();
      }, 1000);
    } catch (e) {
      error = String(e);
    }
  }

  function closePairing() {
    clearInterval(timer);
    if (pairing) void invoke('phone_close_pairing');
    pairing = null;
  }

  async function forget(key: string) {
    if (confirmForget !== key) {
      confirmForget = key;
      setTimeout(() => confirmForget === key && (confirmForget = ''), 3000);
      return;
    }
    confirmForget = '';
    status = (await invoke<PhoneStatus>('phone_forget', { key })) ?? status;
  }

  // Sending a file to the phone: read here in pieces, written to a temp file by the app, then offered to the phone,
  // which downloads it into its Downloads folder.
  const PIECE = 384 * 1024;
  let sending = $state<{ name: string; done: number; size: number } | null>(null);
  let sendNote = $state('');
  let picker = $state<HTMLInputElement>();
  let ringing = $state(false);

  function toBase64(bytes: Uint8Array): string {
    let text = '';
    for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    return btoa(text);
  }

  async function sendFile(file: File) {
    if (!status?.devices.some((d) => d.connected)) {
      sendNote = `✕ ${L('No phone is connected; open Cloudy on the phone.', 'گوشی وصل نیست؛ Cloudy را روی گوشی باز کن.')}`;
      if (picker) picker.value = '';
      return;
    }
    const id = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
    sending = { name: file.name, done: 0, size: file.size };
    sendNote = '';
    try {
      let offset = 0;
      do {
        const piece = new Uint8Array(await file.slice(offset, offset + PIECE).arrayBuffer());
        const last = offset + piece.length >= file.size;
        await invoke('phone_send_piece', { id, name: file.name, offset, data: toBase64(piece), last });
        offset += piece.length;
        sending = { name: file.name, done: offset, size: file.size };
        if (last) break;
      } while (offset < file.size);
      sendNote = L(`"${file.name}" is on its way to the phone (Downloads).`, `«${file.name}» در حال رفتن به گوشی است (پوشه Downloads).`);
    } catch (e) {
      sendNote = `✕ ${String(e)}`;
    } finally {
      sending = null;
      if (picker) picker.value = '';
    }
  }

  let ringTimer: ReturnType<typeof setTimeout> | undefined;
  let ringBusy = $state(false);

  async function ring() {
    if (ringBusy) return;
    const stop = ringing;
    ringBusy = true;
    clearTimeout(ringTimer);
    try {
      await invoke('phone_ring', { stop });
      ringing = !stop;
      // The phone stops by itself after 30 s.
      if (ringing) ringTimer = setTimeout(() => (ringing = false), 30_000);
    } catch (e) {
      sendNote = `✕ ${String(e)}`;
    } finally {
      ringBusy = false;
    }
  }

  function copy() {
    if (!pairing) return;
    void invoke('copy_text', { text: pairing.text });
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  onMount(() => {
    void invoke<PhoneStatus>('phone_status').then((s) => s && (status = s));
    let stop = () => {};
    void listen<PhoneStatus>('phone-changed', (s) => {
      const before = status?.devices.length ?? 0;
      status = s;
      // A new phone paired: the QR code has done its job.
      if (pairing && s.devices.length > before) closePairing();
    }).then((u) => (stop = u));
    return () => {
      stop();
      closePairing();
    };
  });
</script>

<h1>{L('Phone', 'گوشی')}</h1>
<p class="lead">
  {L(
    'With the Cloudy app on your phone (on the same Wi-Fi network) you can watch what the assistant does and send requests from a distance. The connection is encrypted and only paired phones can connect.',
    'با اپ Cloudy روی گوشی (در همان شبکه Wi-Fi) ببین دستیار چه می‌کند و از راه دور درخواست بفرست. ارتباط رمزنگاری‌شده است و فقط گوشی‌های جفت‌شده وصل می‌شوند.'
  )}
</p>

<div class="card">
  <div class="row">
    <div>
      <b>{L('Phone connection', 'اتصال گوشی')}</b>
      <small>
        {#if status?.enabled}{L("On. This computer's address:", 'روشن — آدرس این کامپیوتر:')} <code dir="ltr">{status.address || L('no network', 'بدون شبکه')}</code>
        {:else}{L('The first time, Windows asks about network access; allow "Private networks".', 'بار اول Windows برای دسترسی شبکه سؤال می‌کند؛ «Private networks» را تأیید کن.')}{/if}
      </small>
    </div>
    <Toggle checked={status?.enabled ?? false} onchange={setEnabled} label="phone" />
  </div>
  {#if status?.error}<div class="result" transition:slide={{ duration: 200 }} dir="auto">✕ {status.error}</div>{/if}
  {#if status?.enabled}
    <small class="away" transition:slide={{ duration: 200 }}>
      {L(
        'From outside home: install Tailscale on this computer and on the phone (same account). The phone finds the computer by itself; no port is opened on your router.',
        'از بیرون خانه: Tailscale را روی همین کامپیوتر و روی گوشی نصب کن (با یک حساب). گوشی خودش کامپیوتر را پیدا می‌کند و هیچ پورتی روی مودم باز نمی‌شود.'
      )}
    </small>
  {/if}
</div>

{#if status?.enabled}
  {#if pairing}
    <div class="card pair" transition:slide={{ duration: 320, easing: cubicOut }}>
      <div class="qr" in:scale={{ start: 0.9, duration: 360, easing: cubicOut }}>{@html pairing.svg}</div>
      <div class="steps">
        <b>{L('Pair a phone', 'جفت کردن گوشی')}</b>
        <ol>
          <li>{L('Open the Cloudy app on the phone.', 'اپ Cloudy را روی گوشی باز کن.')}</li>
          <li>{L('Tap "Connect to computer" and scan this code.', '«اتصال به کامپیوتر» را بزن و این کد را اسکن کن.')}</li>
          <li>{L('The phone and the computer must be on the same network.', 'گوشی و کامپیوتر باید در یک شبکه باشند.')}</li>
        </ol>
        {#if pairing.code}
          <div class="code-line">
            <span>{L('Or type this code on the phone:', 'یا این کد را در گوشی تایپ کن:')}</span>
            <code class="pair-code" dir="ltr">{pairing.code.slice(0, 3)} {pairing.code.slice(3)}</code>
          </div>
          <small class="fingerprint">
            {L('The phone will show this to check it is this computer:', 'گوشی این را نشان می‌دهد تا مطمئن شوی همین کامپیوتر است:')}
            <code dir="ltr">{pairing.fingerprint}</code>
          </small>
        {/if}
        <small>
          {L(
            `This code works for one phone only, for another ${Math.floor(left / 60)}:${(left % 60).toString().padStart(2, '0')}.`,
            `این کد فقط برای یک گوشی و تا ${num(Math.floor(left / 60))}:${(left % 60).toString().padStart(2, '0')} دیگر معتبر است.`
          )}
        </small>
        <div class="buttons">
          <button class="mini" onclick={copy}>{copied ? L('Copied ✓', 'کپی شد ✓') : L('Copy the code as text', 'کپی متن کد')}</button>
          <button class="mini" onclick={closePairing}>{L('Close', 'بستن')}</button>
        </div>
      </div>
    </div>
  {:else}
    <button class="add" onclick={showPairing} disabled={busy} in:fly={{ y: 8, duration: 260 }}>
      <svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14" /></svg>
      {L('Add a phone', 'افزودن گوشی')}
    </button>
  {/if}
{/if}

{#if error}<div class="result" transition:slide={{ duration: 200 }} dir="auto">✕ {error}</div>{/if}

{#if status?.devices.length}
  <h2>{L('Paired phones', 'گوشی‌های جفت‌شده')}</h2>
  <div class="devices">
    {#each status.devices as d (d.key)}
      <div class="device" in:fly={{ y: 8, duration: 260 }} out:fade={{ duration: 150 }}>
        <span class="dot" class:on={d.connected}></span>
        <div class="info">
          <b>{d.name}</b>
          <small>{d.connected ? L('Connected', 'وصل است') : L(`Last connected: ${ago(d.lastSeen)}`, `آخرین اتصال: ${ago(d.lastSeen)}`)}</small>
        </div>
        {#if d.connected}
          <button class="mini" onclick={() => picker?.click()} disabled={sending !== null}>{L('Send a file', 'فرستادن فایل')}</button>
          <button class="mini" class:on={ringing} onclick={ring} disabled={ringBusy}>{ringing ? L('Stop ringing', 'قطع زنگ') : L('Ring it', 'زنگ بزن')}</button>
        {/if}
        <button class="mini danger" class:sure={confirmForget === d.key} onclick={() => forget(d.key)}>{confirmForget === d.key ? L('Sure?', 'مطمئنی؟') : L('Remove', 'حذف')}</button>
      </div>
    {/each}
  </div>
  <input type="file" class="hidden" bind:this={picker} onchange={(e) => { const f = (e.currentTarget as HTMLInputElement).files?.[0]; if (f) void sendFile(f); }} />
  {#if sending}
    <div class="sending" transition:slide={{ duration: 200 }}>
      <small dir="auto">{sending.done >= sending.size ? L(`${sending.name}: waiting for the phone…`, `${sending.name}: منتظر گوشی…`) : sending.name}</small>
      <div class="bar"><i style="width: {sending.size ? Math.round((sending.done / sending.size) * 100) : 100}%"></i></div>
    </div>
  {/if}
  {#if sendNote}<div class="result" transition:slide={{ duration: 200 }} dir="auto">{sendNote}</div>{/if}
{/if}

<style>
  h1 {
    margin: 0 0 14px;
    font-size: 22px;
    font-weight: 800;
  }
  h2 {
    margin: 24px 0 10px;
    font-size: 16px;
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
    margin-bottom: 14px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 18px;
    padding: 14px 0;
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
  code {
    font-family: var(--mono);
    font-size: 12px;
  }
  .pair {
    display: flex;
    gap: 20px;
    align-items: center;
    padding: 18px;
    border-color: color-mix(in srgb, var(--accent-text) 30%, transparent);
  }
  .qr {
    flex-shrink: 0;
    width: 200px;
    height: 200px;
    padding: 8px;
    border-radius: 14px;
    background: #fff;
    box-shadow: 0 0 30px var(--glow-soft);
  }
  .away {
    display: block;
    margin-top: 10px;
    color: var(--text-dim);
    font-size: 12px;
    line-height: 1.5;
  }
  .code-line {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    margin: 6px 0 8px;
    color: var(--text-dim);
    font-size: 12.5px;
  }
  .fingerprint code {
    font-weight: 600;
    letter-spacing: 0.06em;
    color: var(--text);
  }
  .pair-code {
    font-size: 22px;
    font-weight: 700;
    letter-spacing: 0.12em;
    color: var(--text);
    padding: 4px 12px;
    border-radius: 10px;
    background: color-mix(in srgb, var(--accent-text) 12%, transparent);
  }
  .qr :global(svg) {
    width: 100%;
    height: 100%;
  }
  .steps b {
    font-size: 14px;
  }
  .steps ol {
    margin: 8px 0;
    padding-inline-start: 18px;
    color: var(--text-muted);
    font-size: 12.5px;
    line-height: 1.9;
  }
  .steps small {
    color: var(--text-dim);
    font-size: 12px;
  }
  .buttons {
    display: flex;
    gap: 6px;
    margin-top: 12px;
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
    transition: border-color var(--fast), color var(--fast), background var(--fast);
  }
  .add:hover {
    border-color: var(--accent-text);
    color: var(--text);
    background: var(--surface);
  }
  .add svg {
    width: 18px;
    height: 18px;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    fill: none;
  }
  .hidden {
    display: none;
  }
  .sending {
    margin-top: 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .sending small {
    color: var(--text-dim);
    font-size: 12px;
  }
  .bar {
    height: 6px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    background: var(--accent-text);
    transition: width 200ms ease;
  }
  .mini.on {
    background: color-mix(in srgb, var(--accent-text) 30%, transparent);
  }
  .devices {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .device {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    border-radius: 14px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
  }
  .device .info {
    flex: 1;
  }
  .device b {
    display: block;
    font-size: 13.5px;
  }
  .device small {
    color: var(--text-dim);
    font-size: 12px;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--text-dim);
  }
  .dot.on {
    background: #34d399;
    box-shadow: 0 0 10px rgba(52, 211, 153, 0.7);
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
  }
  .mini:hover {
    background: rgba(255, 255, 255, 0.13);
    color: var(--text);
  }
  .mini.danger:hover,
  .mini.sure {
    background: rgba(225, 29, 72, 0.85);
    color: #fff;
  }
  .result {
    margin: 0 0 12px;
    padding: 8px 12px;
    border-radius: 10px;
    font-size: 12.5px;
    color: #fecaca;
    background: rgba(248, 113, 113, 0.08);
  }
</style>