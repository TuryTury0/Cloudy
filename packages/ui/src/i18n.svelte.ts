// The app's language: English (default) or Persian. Every window sets it from Settings; texts are written next
// to where they are used as L('English', 'فارسی'), so a new text can never miss one of the two languages.

export type Language = 'en' | 'fa';

export const LANGUAGES: readonly { id: Language; label: string }[] = [
  { id: 'en', label: 'English' },
  { id: 'fa', label: 'فارسی' }
];

/** Reactive: templates that call L() or dir() update when the language changes. */
export const i18n = $state<{ lang: Language }>({ lang: 'en' });

export function normalizeLanguage(value: unknown): Language {
  return value === 'fa' ? 'fa' : 'en';
}

/** Sets the language for this window; the root element gets the lang attribute (fonts follow it, see tokens.css). */
export function setLanguage(value: unknown, root: HTMLElement | null = typeof document === 'undefined' ? null : document.documentElement): void {
  i18n.lang = normalizeLanguage(value);
  root?.setAttribute('lang', i18n.lang);
}

/** The text in the current language. */
export function L(en: string, fa: string): string {
  return i18n.lang === 'fa' ? fa : en;
}

/** Reading direction of the current language, for a window's root element. */
export function dir(): 'ltr' | 'rtl' {
  return i18n.lang === 'fa' ? 'rtl' : 'ltr';
}

/** BCP 47 tag for numbers and dates in the current language. */
export function locale(): string {
  return i18n.lang === 'fa' ? 'fa-IR' : 'en-US';
}

/** A number in the digits of the current language. */
export function num(value: number): string {
  return value.toLocaleString(locale());
}

/** A date (from milliseconds) the way the current language writes it. */
export function day(ms: number): string {
  return new Date(ms).toLocaleDateString(locale());
}
