<script lang="ts">
  // Settings > Learning (train mode): the assistant reuses what worked and learns a rule from every mistake or
  // correction. Shows what it learned; anything can be removed.
  import { onMount } from 'svelte';
  import { fly, slide } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { L, day, num } from '@anytask/ui';
  import { invoke } from '../lib/ipc';
  import type { LearnSummary, Skill } from '../lib/types';
  import Toggle from './Toggle.svelte';

  let learn = $state<LearnSummary | null>(null);
  let open = $state<Record<string, boolean>>({});
  let confirmClear = $state(false);

  async function setEnabled(enabled: boolean) {
    learn = (await invoke<LearnSummary>('learn_set_enabled', { enabled })) ?? learn;
  }

  async function forget(args: Record<string, unknown>) {
    learn = (await invoke<LearnSummary>('learn_forget', args)) ?? learn;
  }

  function clearAll() {
    if (!confirmClear) {
      confirmClear = true;
      setTimeout(() => (confirmClear = false), 3000);
      return;
    }
    confirmClear = false;
    void forget({ all: true });
  }

  let skills = $state<Skill[]>([]);
  let openSkill = $state<Record<string, boolean>>({});
  /** The list of about a hundred tips stays folded until asked for. */
  let showSkills = $state(false);

  async function loadSkills() {
    skills = (await invoke<Skill[]>('skills_list')) ?? skills;
  }

  async function openSkillsFolder() {
    await invoke('open_skills_folder');
  }

  onMount(() => {
    void invoke<LearnSummary>('learn_summary').then((s) => s && (learn = s));
    void loadSkills();
    // A file added or edited in the folder shows up when the window comes back.
    const refresh = () => void loadSkills();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  });
</script>

<h1>{L('Learning', 'آموزش')}</h1>
<p class="lead">
  {L(
    'The assistant learns from the tasks it does: it reuses the ways that worked, and from every mistake or every time you correct it ("no, play it in the app") it writes a short lesson and follows it next time. Everything stays on this computer.',
    'دستیار از کارهایی که انجام می‌دهد یاد می‌گیرد: راه‌هایی که جواب داد را دوباره به کار می‌برد، و از هر اشتباه یا هر بار که اصلاحش می‌کنی («نه، توی اپ پخش کن») یک درس کوتاه می‌نویسد و دفعه بعد رعایتش می‌کند. همه‌چیز فقط روی همین کامپیوتر می‌ماند.'
  )}
</p>

<div class="card">
  <div class="row">
    <div>
      <b>{L('Learning mode', 'حالت آموزش')}</b>
      <small>{learn?.enabled ? L('On: every finished task is recorded', 'روشن — هر کار تمام‌شده ثبت می‌شود') : L('Off: nothing is recorded or used', 'خاموش — چیزی ثبت یا استفاده نمی‌شود')}</small>
    </div>
    <Toggle checked={learn?.enabled ?? true} onchange={setEnabled} label="train mode" />
  </div>
</div>

{#if learn}
  <div class="stats">
    <div class="stat"><small>{L('Recorded requests', 'درخواست‌های ثبت‌شده')}</small><b>{num(learn.requests)}</b><em>{L(`${learn.worked} worked`, `${num(learn.worked)} موفق`)}</em></div>
    <div class="stat"><small>{L('Learned ways', 'روش‌های یادگرفته')}</small><b>{num(learn.playbooks.length)}</b><em>{L('tasks on the screen', 'کارهای روی صفحه')}</em></div>
    <div class="stat"><small>{L('Lessons', 'درس‌ها')}</small><b>{num(learn.lessons.length)}</b><em>{L('from mistakes and corrections', 'از اشتباه‌ها و اصلاح‌ها')}</em></div>
  </div>

  <h2>{L('App tips', 'نکته‌های برنامه‌ها')}</h2>
  <p class="lead">
    {L(
      'Short notes about one app or site. The screen agent gets them only in jobs about that app, so they cost nothing elsewhere. Add your own as .md files in the folder; a file with a built-in name replaces it.',
      'یادداشت‌های کوتاه درباره یک برنامه یا سایت. فقط در کارهای مربوط به همان برنامه به کار می‌روند، پس جای دیگر هزینه‌ای ندارند. نکته‌های خودت را به شکل فایل .md در پوشه بگذار؛ فایلی با اسم یک نکته‌ی آماده، جایش را می‌گیرد.'
    )}
  </p>
  <div class="tips-bar">
    <span class="count">
      {L(`${skills.filter((s) => s.builtin).length} built in`, `${num(skills.filter((s) => s.builtin).length)} آماده`)} ·
      {L(`${skills.filter((s) => !s.builtin).length} yours`, `${num(skills.filter((s) => !s.builtin).length)} مال تو`)}
    </span>
    <button class="mini" onclick={() => (showSkills = !showSkills)}>{showSkills ? L('Hide the list', 'پنهان کردن فهرست') : L('Show the list', 'نمایش فهرست')}</button>
    <button class="mini" onclick={openSkillsFolder}>{L('Open the tips folder', 'باز کردن پوشه نکته‌ها')}</button>
  </div>
  {#if showSkills}
    <div class="tips" transition:slide={{ duration: 240 }}>
      {#each skills as skill (skill.name)}
        <div class="item">
          <button class="body open" onclick={() => (openSkill[skill.name] = !openSkill[skill.name])}>
            <span dir="auto"><b>{skill.name}</b> <em class="tag" class:mine={!skill.builtin}>{skill.builtin ? L('built in', 'آماده') : L('yours', 'مال تو')}</em></span>
            <small dir="auto">{skill.keys.join(', ')}</small>
            {#if openSkill[skill.name]}
              <pre dir="auto" transition:slide={{ duration: 200 }}>{skill.text}</pre>
            {/if}
          </button>
        </div>
      {/each}
    </div>
  {/if}

  <h2>{L('Lessons', 'درس‌ها')}</h2>
  {#each learn.lessons as lesson (lesson.id)}
    <div class="item" in:fly={{ y: 8, duration: 240, easing: cubicOut }}>
      <div class="body">
        <span dir="ltr">{lesson.text}</span>
        <small>
          {L(`From "${lesson.about}", ${day(lesson.at)}`, `از «${lesson.about}» — ${day(lesson.at)}`)}{lesson.uses ? L(`, used ${lesson.uses} times`, ` — ${num(lesson.uses)} بار به کار رفته`) : ''}
        </small>
      </div>
      <button class="x" aria-label="forget" onclick={() => forget({ lesson: lesson.id })}>×</button>
    </div>
  {:else}
    <p class="empty">{L('No lessons yet. When a task goes wrong or you correct it, the lesson shows here.', 'هنوز درسی نیست. وقتی کاری اشتباه شود یا اصلاحش کنی، اینجا می‌آید.')}</p>
  {/each}

  <h2>{L('Ways of doing tasks on the screen', 'روش‌های کار روی صفحه')}</h2>
  {#each learn.playbooks as p (p.goal)}
    <div class="item" in:fly={{ y: 8, duration: 240, easing: cubicOut }}>
      <button class="body open" onclick={() => (open[p.goal] = !open[p.goal])}>
        <span dir="auto">{p.goal}</span>
        <small>
          {L(`${p.steps.length} steps, ${Math.round(p.seconds)} seconds, ${day(p.at)}`, `${num(p.steps.length)} قدم، ${num(Math.round(p.seconds))} ثانیه — ${day(p.at)}`)}{p.uses ? L(`, reused ${p.uses} times`, ` — ${num(p.uses)} بار دوباره استفاده شده`) : ''}
        </small>
        {#if open[p.goal]}
          <ol dir="ltr" transition:slide={{ duration: 200 }}>
            {#each p.steps as s, i (i)}<li>{s}</li>{/each}
          </ol>
        {/if}
      </button>
      <button class="x" aria-label="forget" onclick={() => forget({ playbook: p.goal })}>×</button>
    </div>
  {:else}
    <p class="empty">{L('Nothing recorded yet. Every screen task that goes right is saved here.', 'هنوز روشی ثبت نشده. هر کار روی صفحه که درست انجام شود، اینجا ذخیره می‌شود.')}</p>
  {/each}


  <div class="actions">
    <button class="mini danger" class:sure={confirmClear} onclick={clearAll}>{confirmClear ? L('Sure? Everything is erased', 'مطمئنی؟ همه پاک می‌شود') : L('Forget everything', 'فراموش کردن همه')}</button>
  </div>
{/if}

<style>
  h1 {
    margin: 0 0 14px;
    font-size: 22px;
    font-weight: 800;
  }
  h2 {
    margin: 22px 0 10px;
    font-size: 16px;
    font-weight: 800;
  }
  .lead {
    margin: -6px 0 16px;
    color: var(--text-dim);
    font-size: 13px;
    line-height: 1.8;
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
  small {
    display: block;
    margin-top: 3px;
    color: var(--text-dim);
    font-size: 12px;
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
  }
  .stat {
    padding: 12px 14px;
    border-radius: 14px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
  }
  .stat b {
    display: block;
    font-size: 20px;
    font-weight: 800;
  }
  .stat em {
    font-style: normal;
    font-size: 11px;
    color: var(--text-muted);
  }
  .item {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 12px;
    margin-bottom: 8px;
    border-radius: 12px;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
  }
  .body {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    line-height: 1.7;
  }
  .open {
    border: none;
    background: none;
    color: inherit;
    text-align: start;
    cursor: pointer;
    padding: 0;
    font: inherit;
  }
  ol {
    margin: 6px 0 0;
    padding-inline-start: 18px;
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--text-muted);
  }
  .x {
    width: 22px;
    height: 22px;
    flex-shrink: 0;
    border: none;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-muted);
    cursor: pointer;
  }
  .x:hover {
    background: rgba(225, 29, 72, 0.85);
    color: #fff;
  }
  .tips-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 10px;
  }
  .tips-bar .count {
    margin-inline-end: auto;
    font-size: 12.5px;
    color: var(--text-muted);
  }
  .tag {
    font-style: normal;
    font-size: 10.5px;
    font-weight: 600;
    padding: 1px 7px;
    border-radius: 999px;
    color: var(--text-dim);
    background: rgba(255, 255, 255, 0.07);
  }
  .tag.mine {
    color: var(--accent-text);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }
  pre {
    margin: 8px 0 0;
    font-family: inherit;
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
    color: var(--text-muted);
  }
  .empty {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .actions {
    margin-top: 18px;
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
  .mini.danger:hover,
  .mini.sure {
    background: rgba(225, 29, 72, 0.85);
    color: #fff;
  }
</style>
