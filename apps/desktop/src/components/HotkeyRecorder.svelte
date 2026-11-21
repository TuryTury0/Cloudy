<script lang="ts">
  // Click, press the key combination, done. Plain keys (without Ctrl/Alt/Win) are refused because a global
  // shortcut would take them away from every other program.
  import { shortcutFromKey } from '../lib/shortcut';
  import { L } from '@anytask/ui';
  import { invoke } from '../lib/ipc';

  interface Props {
    value: string;
    onchange: (value: string) => void;
    problem?: string;
  }

  let { value, onchange, problem = '' }: Props = $props();
  let recording = $state(false);
  let hint = $state('');

  /** [English, Persian] */
  const HINTS: Record<string, [string, string]> = {
    'needs-modifier': ['Hold Ctrl, Alt or Win too', 'Ctrl، Alt یا Win را هم نگه دار'],
    unsupported: ['This key is not supported', 'این کلید پشتیبانی نمی‌شود'],
    'modifier-only': ['', '']
  };

  async function onKey(event: KeyboardEvent) {
    if (!recording) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === 'Escape' && !event.ctrlKey && !event.altKey) {
      recording = false;
      hint = '';
      return;
    }
    const result = shortcutFromKey(event);
    if (!result.ok) {
      hint = L(...HINTS[result.reason]);
      return;
    }
    const error = await invoke<void>('validate_hotkey', { shortcut: result.shortcut }).then(
      () => '',
      (e: unknown) => String(e)
    );
    if (error) {
      hint = error;
      return;
    }
    recording = false;
    hint = '';
    onchange(result.shortcut);
  }
</script>

<button
  type="button"
  class="recorder"
  class:recording
  class:problem={Boolean(problem)}
  onclick={() => {
    recording = !recording;
    hint = recording ? L('Press the keys (Esc to cancel)', 'کلیدها را فشار بده (Esc برای لغو)') : '';
  }}
  onkeydown={onKey}
  onblur={() => {
    recording = false;
    hint = '';
  }}
>
  {#if recording}
    <span class="pulse"></span><span>{L('Recording…', 'در حال ضبط…')}</span>
  {:else}
    {#each value.split('+') as part, i (i)}
      <kbd>{part}</kbd>
    {/each}
  {/if}
</button>
{#if hint || problem}
  <div class="hint" class:error={Boolean(problem) && !hint}>{hint || problem}</div>
{/if}

<style>
  .recorder {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 190px;
    justify-content: center;
    padding: 8px 12px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    cursor: pointer;
    direction: ltr;
    transition: border-color var(--fast), background var(--fast), box-shadow var(--fast);
  }
  .recorder:hover {
    background: var(--surface-hover);
  }
  .recording {
    border-color: var(--accent-text);
    box-shadow: 0 0 0 3px var(--glow-soft);
  }
  .problem {
    border-color: var(--danger);
  }
  kbd {
    font-family: var(--mono);
    font-size: 11.5px;
    padding: 3px 8px;
    border-radius: 7px;
    background: rgba(255, 255, 255, 0.08);
    border: 1px solid rgba(255, 255, 255, 0.14);
    border-bottom-width: 2px;
  }
  .pulse {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--danger);
    animation: blink 0.9s ease-in-out infinite alternate;
  }
  .hint {
    margin-top: 6px;
    font-size: 11.5px;
    color: var(--text-dim);
  }
  .hint.error {
    color: var(--danger);
  }
  @keyframes blink {
    to {
      opacity: 0.25;
    }
  }
</style>
