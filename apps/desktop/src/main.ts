import { mount } from 'svelte';
import '@fontsource-variable/vazirmatn';
import '@fontsource-variable/plus-jakarta-sans';
import '@anytask/ui/styles.css';
import App from './App.svelte';

const app = mount(App, { target: document.getElementById('app')! });

export default app;
