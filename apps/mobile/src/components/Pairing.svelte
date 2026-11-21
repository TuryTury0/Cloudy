<script lang="ts">
  // Pairing with the PC, once: type the 6-digit code the PC shows (main way), scan its QR code, or paste the code's
  // text. On success the link status says "paired" and the app shows its tabs.
  //
  // The code way checks who answers before pairing: only one computer on the Wi-Fi may be waiting for a code, and
  // the user confirms that its key's short fingerprint matches the one on the PC's screen (another device on the
  // network cannot pose as the PC).
  import { tick } from 'svelte';
  import { fly, slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { BotFace } from '@anytask/ui';
  import { Format, checkPermissions, requestPermissions, scan } from '@tauri-apps/plugin-barcode-scanner';
  import { inTauri, invoke } from '../lib/ipc';
  import type { CodePc, LinkStatus } from '../lib/types';
  import Icon from './Icon.svelte';
  import Sheet from './Sheet.svelte';
  import { haptic, ui } from './store.svelte';
  import { asciiDigits, faError } from './text';

  /** The camera scanner exists on phones only. */
  const canScan = inTauri && /Android|iPhone|iPad/i.test(navigator.userAgent);

  let digits = $state('');
  let field = $state<HTMLInputElement>();
  let focused = $state(false);
  let pairing = $state(false);
  let finding = $state(false);
  /** The one computer that answered, waiting for the user to compare fingerprints. */
  let found = $state<CodePc | null>(null);
  let scanning = $state(false);
  let pairError = $state('');
  let shake = $state(false);
  let showText = $state(false);
  let code = $state('');

  function failed(e: unknown) {
    pairError = faError(e);
    shake = true;
    haptic(40);
    setTimeout(() => (shake = false), 450);
  }

  /** Step 1: finds the computer waiting for a code; exactly one may answer. */
  async function findWithDigits() {
    if (digits.length !== 6 || pairing || finding || found) return;
    pairError = '';
    finding = true;
    field?.blur();
    try {
      const pcs = (await invoke<CodePc[]>('find_code_pc')) ?? [];
      if (pcs.length > 1) failed('بیش از یک کامپیوتر جواب داد؛ برای امنیت با اسکن QR وصل شو.');
      else if (pcs.length === 0) failed('No computer on this network is waiting for a code.');
      else {
        found = pcs[0];
        haptic(15);
      }
    } catch (e) {
      failed(e);
    } finally {
      finding = false;
    }
  }

  /** Step 2: the user said the fingerprints match; pairs with that computer only. */
  async function pairWithDigits() {
    const pc = found;
    if (!pc || pairing) return;
    pairError = '';
    pairing = true;
    try {
      const status = await invoke<LinkStatus>('pair_code', { code: digits, address: pc.address, key: pc.key, name: pc.name });
      found = null;
      if (status) ui.link = status;
      haptic(20);
    } catch (e) {
      found = null;
      failed(e);
      // A wrong code is typed again; a network problem keeps it for another try.
      if (/not right|expired/i.test(String(e))) {
        digits = '';
        await tick();
        field?.focus();
      }
    } finally {
      pairing = false;
    }
  }

  function notThisPc() {
    if (pairing) return;
    haptic();
    found = null;
  }

  function onDigits(e: Event & { currentTarget: HTMLInputElement }) {
    const clean = asciiDigits(e.currentTarget.value).replace(/\D/g, '').slice(0, 6);
    e.currentTarget.value = clean;
    if (clean.length > digits.length) haptic(6);
    digits = clean;
    pairError = '';
    if (clean.length === 6) void findWithDigits();
  }

  async function pairWithText(text = code) {
    pairError = '';
    pairing = true;
    (document.activeElement as HTMLElement | null)?.blur();
    try {
      const status = await invoke<LinkStatus>('pair', { code: text });
      if (status) ui.link = status;
      code = '';
    } catch (e) {
      failed(e);
    } finally {
      pairing = false;
    }
  }

  async function scanCode() {
    pairError = '';
    scanning = true;
    haptic();
    try {
      let permission = await checkPermissions();
      if (permission !== 'granted') permission = await requestPermissions();
      if (permission !== 'granted') {
        pairError = 'برای اسکن، اجازه دوربین لازم است.';
        return;
      }
      const scanned = await scan({ windowed: false, formats: [Format.QRCode] });
      await pairWithText(scanned.content);
    } catch (e) {
      failed(e);
    } finally {
      scanning = false;
    }
  }
</script>

<section class="pair" in:fly={{ y: 18, duration: 420, easing: cubicOut }}>
  <div class="hero">
    <div class="orb"><BotFace mode={pairing || finding ? 'thinking' : 'idle'} level={0} size={64} /></div>
  </div>
  <h1>اتصال به کامپیوتر</h1>
  <p class="lead">در Cloudy کامپیوتر: تنظیمات ← گوشی ← «افزودن گوشی». کد ۶ رقمی‌اش را اینجا بزن.</p>

  <div class="code" class:shake class:busy={pairing || finding}>
    <input
      bind:this={field}
      value={digits}
      oninput={onDigits}
      onfocus={() => (focused = true)}
      onblur={() => (focused = false)}
      inputmode="numeric"
      autocomplete="one-time-code"
      dir="ltr"
      aria-label="کد ۶ رقمی"
      disabled={pairing || finding}
    />
    <div class="boxes" dir="ltr" aria-hidden="true">
      {#each [0, 1, 2, 3, 4, 5] as i (i)}
        <span class:filled={i < digits.length} class:caret={focused && i === Math.min(digits.length, 5)}>
          {#if digits[i]}<b in:fly={{ y: 8, duration: 180 }}>{digits[i]}</b>{/if}
        </span>
      {/each}
    </div>
  </div>
  <p class="note"><Icon name="wifi" size={15} />گوشی و کامپیوتر باید روی یک وای‌فای باشند.</p>

  {#if pairError}<p class="error" transition:slide={{ duration: 200 }}>{pairError}</p>{/if}

  <button class="btn primary" disabled={digits.length !== 6 || pairing || finding} onclick={findWithDigits}>
    {#if finding}<span class="spin dark"></span>در حال پیدا کردن کامپیوتر…{:else if pairing}<span class="spin dark"></span>در حال اتصال…{:else}اتصال{/if}
  </button>

  {#if canScan}
    <button class="btn ghost" disabled={scanning || pairing || finding} onclick={scanCode}>
      <Icon name="qr" size={19} />اسکن کد QR
    </button>
  {/if}

  <button class="link" onclick={() => (showText = !showText)} aria-expanded={showText}>
    <Icon name="paste" size={15} />{showText ? 'بستن' : 'چسباندن متن کد'}
  </button>
  {#if showText}
    <div class="text" transition:slide={{ duration: 220 }}>
      <textarea bind:value={code} rows="3" dir="ltr" placeholder={'{"v":1,"k":"…"}'} spellcheck="false" aria-label="متن کد"></textarea>
      <button class="btn ghost" disabled={!code.trim() || pairing} onclick={() => pairWithText()}>اتصال با متن</button>
    </div>
  {/if}
</section>

<Sheet open={found !== null} title={found ? `وصل شدن به «${found.name}»؟` : ''} onclose={notThisPc}>
  {#if found}
    <p class="print" dir="ltr">{found.fingerprint}</p>
    <p class="same">این کد باید با کدی که روی صفحه کامپیوتر کنار کد ۶ رقمی نوشته شده یکی باشد.</p>
    <div class="answers">
      <button class="btn primary" onclick={pairWithDigits} disabled={pairing}>
        {#if pairing}<span class="spin dark"></span>{/if}بله، یکی است
      </button>
      <button class="btn ghost" onclick={notThisPc} disabled={pairing}>نه</button>
    </div>
  {/if}
</Sheet>

<style>
  .pair {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 20px 20px 28px;
    overflow-y: auto;
  }
  .hero {
    display: grid;
    place-items: center;
    padding: 18px 0 4px;
  }
  .orb {
    width: 132px;
    height: 132px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: radial-gradient(circle at 50% 30%, rgba(255, 255, 255, 0.16), rgba(255, 255, 255, 0.03) 70%);
    border: 1px solid rgba(255, 255, 255, 0.14);
    backdrop-filter: blur(20px);
    -webkit-backdrop-filter: blur(20px);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.25),
      0 20px 60px -20px var(--glow);
    animation: float 5s ease-in-out infinite;
  }
  h1 {
    margin: 4px 0 0;
    font-size: 25px;
    font-weight: 850;
    text-align: center;
  }
  .lead {
    margin: 0;
    color: var(--text-muted);
    font-size: 14px;
    line-height: 1.9;
    text-align: center;
  }
  .code {
    position: relative;
    margin-top: 6px;
  }
  .code input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    opacity: 0;
    border: none;
    font-size: 16px;
    z-index: 1;
  }
  .boxes {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    gap: 8px;
  }
  .boxes span {
    height: 62px;
    display: grid;
    place-items: center;
    border-radius: 18px;
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.09), rgba(255, 255, 255, 0.03));
    border: 1px solid rgba(255, 255, 255, 0.1);
    backdrop-filter: blur(18px);
    -webkit-backdrop-filter: blur(18px);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.12);
    font-family: var(--font-latin);
    font-size: 28px;
    font-weight: 800;
    transition:
      border-color 200ms ease,
      box-shadow 200ms ease,
      transform 200ms var(--ease-spring);
  }
  .boxes span.filled {
    border-color: rgba(255, 255, 255, 0.22);
  }
  .boxes span.caret {
    border-color: var(--accent-text);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.12),
      0 0 0 3px color-mix(in srgb, var(--accent) 30%, transparent);
    transform: translateY(-2px);
  }
  .boxes span.caret:not(.filled)::after {
    content: '';
    width: 2px;
    height: 26px;
    border-radius: 2px;
    background: var(--text);
    animation: blink 1s steps(2, start) infinite;
  }
  .busy .boxes span {
    animation: wave 1s ease-in-out infinite;
  }
  .busy .boxes span:nth-child(2) {
    animation-delay: 0.08s;
  }
  .busy .boxes span:nth-child(3) {
    animation-delay: 0.16s;
  }
  .busy .boxes span:nth-child(4) {
    animation-delay: 0.24s;
  }
  .busy .boxes span:nth-child(5) {
    animation-delay: 0.32s;
  }
  .busy .boxes span:nth-child(6) {
    animation-delay: 0.4s;
  }
  .shake {
    animation: shake 0.42s ease;
  }
  .note {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    margin: 0;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .link {
    align-self: center;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border: none;
    background: none;
    color: var(--text-dim);
    font: inherit;
    font-size: 13px;
  }
  .text {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  textarea {
    font-family: var(--mono);
    font-size: 12px;
  }
  .print {
    margin: 4px 0 0;
    padding: 14px 10px;
    border-radius: 18px;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    font-family: var(--mono);
    font-size: 28px;
    font-weight: 700;
    letter-spacing: 2px;
    text-align: center;
    color: var(--text);
    user-select: text;
  }
  .same {
    margin: 0;
    font-size: 13.5px;
    line-height: 1.9;
    color: var(--text-muted);
  }
  .answers {
    display: flex;
    gap: 10px;
  }
  .answers .btn:first-child {
    flex: 2;
  }
  .answers .btn:last-child {
    flex: 1;
  }
  @keyframes float {
    50% {
      transform: translateY(-6px);
    }
  }
  @keyframes blink {
    to {
      visibility: hidden;
    }
  }
  @keyframes wave {
    50% {
      transform: translateY(-5px);
      border-color: var(--accent-text);
    }
  }
  @keyframes shake {
    20%,
    60% {
      transform: translateX(-8px);
    }
    40%,
    80% {
      transform: translateX(8px);
    }
  }
</style>