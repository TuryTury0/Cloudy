<script lang="ts">
  // A glass sheet that rises from the bottom (questions like "download to the phone?"). Tapping outside closes it.
  import type { Snippet } from 'svelte';
  import { fade, fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';

  interface Props {
    open: boolean;
    title?: string;
    onclose: () => void;
    children: Snippet;
  }

  let { open, title = '', onclose, children }: Props = $props();
</script>

<svelte:window onkeydown={(e) => open && e.key === 'Escape' && onclose()} />

{#if open}
  <div class="wrap" transition:fade={{ duration: 180 }}>
    <button class="scrim" aria-label="بستن" onclick={onclose}></button>
    <div class="sheet" role="dialog" aria-modal="true" aria-label={title} transition:fly={{ y: 320, duration: 340, easing: cubicOut }}>
      <i class="grab"></i>
      {#if title}<h3>{title}</h3>{/if}
      {@render children()}
    </div>
  </div>
{/if}

<style>
  .wrap {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
  }
  .scrim {
    position: absolute;
    inset: 0;
    border: none;
    background: rgba(0, 0, 0, 0.5);
  }
  .sheet {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin: 0 10px calc(10px + env(safe-area-inset-bottom));
    padding: 10px 18px 18px;
    border-radius: 28px;
    background: linear-gradient(180deg, rgba(40, 44, 64, 0.82), rgba(14, 16, 26, 0.9));
    backdrop-filter: blur(28px) saturate(170%);
    -webkit-backdrop-filter: blur(28px) saturate(170%);
    border: 1px solid rgba(255, 255, 255, 0.12);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.2),
      0 -20px 60px -20px rgba(0, 0, 0, 0.8);
  }
  .grab {
    align-self: center;
    width: 40px;
    height: 4px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.22);
  }
  h3 {
    margin: 4px 0 0;
    font-size: 18px;
    font-weight: 800;
  }
</style>
