<script lang="ts">
  // The settings tab: what the PC's assistant may read on the phone, the last crash (to copy and report), remote
  // access help, and forgetting the PC.
  import { slide } from 'svelte/transition';
  import { invoke } from '../lib/ipc';
  import type { LinkStatus } from '../lib/types';
  import Icon from './Icon.svelte';
  import PageHead from './PageHead.svelte';
  import Sheet from './Sheet.svelte';
  import { goTab, haptic, loadAccess, setSpeakAnswers, toast, ui } from './store.svelte';
  import { faError } from './text';

  let confirmUnpair = $state(false);
  let copied = $state(false);

  async function grant(kind: 'notifications' | 'sms') {
    haptic();
    await invoke('grant', { kind }).catch(() => {});
    await loadAccess();
  }

  async function copyCrash() {
    const text = ui.crash ?? '';
    haptic();
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Older WebViews: copy through a hidden field.
      const area = document.createElement('textarea');
      area.value = text;
      document.body.append(area);
      area.select();
      document.execCommand('copy');
      area.remove();
    }
    copied = true;
    setTimeout(() => (copied = false), 1800);
  }

  async function getUpdate() {
    if (!ui.update) return;
    haptic();
    try {
      await invoke('open_release', { url: ui.update.url });
      toast('دانلود شروع شد؛ وقتی تمام شد روی فایل بزن تا نصب شود', 'ok', 6000);
    } catch (e) {
      toast(faError(e), 'error');
    }
  }

  async function unpair() {
    confirmUnpair = false;
    haptic(20);
    try {
      ui.link = (await invoke<LinkStatus>('unpair')) ?? ui.link;
      ui.messages = [];
      ui.transfers = [];
      goTab('chat');
    } catch (e) {
      toast(faError(e), 'error');
    }
  }
</script>

<div class="page-scroll">
  <PageHead title="تنظیمات" />

  <div class="body">
    <section class="card glass">
      <h2>دسترسی دستیار به گوشی</h2>
      <p class="sub">دستیار کامپیوتر وقتی ازش بپرسی، این‌ها را از گوشی می‌خواند.</p>
      <div class="row">
        <span class="ic"><Icon name="bell" size={19} /></span>
        <span class="what">اعلان‌ها</span>
        {#if ui.access?.notificationAccess}
          <span class="ok"><Icon name="check" size={16} stroke={2.4} />روشن</span>
        {:else}
          <button class="small-btn" onclick={() => grant('notifications')}>اجازه بده</button>
        {/if}
      </div>
      <div class="row">
        <span class="ic"><Icon name="sms" size={19} /></span>
        <span class="what">پیامک‌ها</span>
        {#if ui.access?.smsAccess}
          <span class="ok"><Icon name="check" size={16} stroke={2.4} />روشن</span>
        {:else}
          <button class="small-btn" onclick={() => grant('sms')}>اجازه بده</button>
        {/if}
      </div>
    </section>

    <section class="card glass">
      <h2><Icon name="volUp" size={17} />خواندن جواب با صدا</h2>
      <div class="row">
        <span class="what">جواب‌های دستیار را با صدای گوشی بخوان</span>
        <button class="small-btn" class:on={ui.speakAnswers} onclick={() => (haptic(), setSpeakAnswers(!ui.speakAnswers))} aria-pressed={ui.speakAnswers}>
          {ui.speakAnswers ? 'روشن' : 'خاموش'}
        </button>
      </div>
      <p class="sub">صدا روی کامپیوتر ساخته می‌شود و به گوشی می‌آید، پس گوشی VPN لازم ندارد. اگر نشد، صدای خود گوشی می‌خواند.</p>
    </section>

    <section class="card glass">
      <h2><Icon name="globe" size={17} />بیرون از خانه</h2>
      <p class="sub">
        Tailscale را روی گوشی و کامپیوتر نصب و روشن کن (با یک حساب). برنامه کامپیوتر را خودش پیدا می‌کند؛ کار دیگری
        لازم نیست.
      </p>
    </section>

    {#if ui.crash}
      <section class="card glass crash" transition:slide={{ duration: 220 }}>
        <h2><Icon name="bug" size={17} />آخرین خطای برنامه</h2>
        <p class="sub">برنامه دفعه قبل بسته شد. این متن را کپی کن و برای سازنده بفرست.</p>
        <pre dir="ltr">{ui.crash}</pre>
        <button class="btn ghost" onclick={copyCrash}>
          <Icon name={copied ? 'check' : 'copy'} size={18} />{copied ? 'کپی شد' : 'کپی متن'}
        </button>
      </section>
    {/if}

    <section class="card glass">
      <h2><Icon name="pc" size={17} />کامپیوتر</h2>
      <p class="sub">جفت با «{ui.link.pc || 'کامپیوتر'}».</p>
      <button class="btn danger" onclick={() => (haptic(), (confirmUnpair = true))}>
        <Icon name="unlink" size={18} />قطع جفت‌شدن
      </button>
    </section>

    {#if ui.update}
      <section class="card glass" transition:slide={{ duration: 220 }}>
        <h2><Icon name="download" size={17} />نسخه جدید</h2>
        <p class="sub">نسخه {ui.update.version} آمده (این گوشی: {ui.update.current}). دانلود کن و روی فایل بزن؛ روی همین برنامه نصب می‌شود و جفت‌شدن می‌ماند.</p>
        <button class="btn primary" onclick={getUpdate}><Icon name="download" size={18} />دانلود و نصب</button>
      </section>
    {/if}

    <p class="foot">Cloudy برای گوشی{ui.version ? ` · نسخه ${ui.version}` : ''}</p>
  </div>
</div>

<Sheet open={confirmUnpair} title="قطع جفت‌شدن؟" onclose={() => (confirmUnpair = false)}>
  <p class="sheet-text">گوشی «{ui.link.pc || 'کامپیوتر'}» را فراموش می‌کند. برای اتصال دوباره باید کد را از نو بزنی.</p>
  <div class="sheet-buttons">
    <button class="btn danger solid" onclick={unpair}>قطع کن</button>
    <button class="btn ghost" onclick={() => (confirmUnpair = false)}>نه</button>
  </div>
</Sheet>

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 4px 12px 16px;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0;
    font-size: 15px;
    font-weight: 800;
  }
  .sub {
    margin: 0;
    font-size: 13px;
    line-height: 1.9;
    color: var(--text-muted);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 0;
  }
  .ic {
    width: 36px;
    height: 36px;
    display: grid;
    place-items: center;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-muted);
  }
  .what {
    flex: 1;
    font-size: 14.5px;
  }
  .ok {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 13px;
    color: #6ee7b7;
  }
  .small-btn {
    padding: 8px 14px;
    border: none;
    border-radius: 12px;
    background: #fff;
    color: #000;
    font: inherit;
    font-size: 13px;
    font-weight: 750;
  }
  .small-btn.on {
    background: #22d3ee;
    box-shadow: 0 0 14px -2px rgba(34, 211, 238, 0.7);
  }
  pre {
    margin: 0;
    max-height: 180px;
    overflow: auto;
    padding: 10px 12px;
    border-radius: 14px;
    background: rgba(0, 0, 0, 0.35);
    color: #fecaca;
    font-family: var(--mono);
    font-size: 11px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    user-select: text;
  }
  .foot {
    margin: 4px 0 0;
    text-align: center;
    font-size: 12px;
    color: var(--text-dim);
  }
  .sheet-text {
    margin: 0;
    font-size: 14px;
    line-height: 1.9;
    color: var(--text-muted);
  }
  .sheet-buttons {
    display: flex;
    gap: 10px;
  }
  .sheet-buttons :global(.btn) {
    flex: 1;
  }
</style>