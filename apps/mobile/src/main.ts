import { mount } from 'svelte';
import '@fontsource-variable/vazirmatn';
import '@anytask/ui/styles.css';
import { setLanguage } from '@anytask/ui';
import App from './App.svelte';

// The phone app is Persian only for now; shared texts (friendly errors) follow it.
setLanguage('fa');

const app = mount(App, { target: document.getElementById('app')! });

export default app;
