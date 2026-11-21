// Tests import .svelte.ts modules (runes such as $state), so the Svelte compiler has to run for them.
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vitest/config';

export default defineConfig({ plugins: [svelte()] });
