<script lang="ts">
  // Each window loads the same page with ?view=<name> and renders its own root.
  import ChatView from './views/ChatView.svelte';
  import IslandView from './views/IslandView.svelte';
  import SettingsView from './views/SettingsView.svelte';

  const view = new URLSearchParams(location.search).get('view') ?? 'island';
</script>

{#if view === 'settings'}
  <SettingsView />
{:else if view === 'chat'}
  <ChatView />
{:else}
  <IslandView />
{/if}
