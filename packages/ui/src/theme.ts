// Colour themes: up to three hex colours with weights, turned into CSS variables for the glass UI.
// Ported from v1 (src/renderer/ui.js): dark accents are lifted so text and meters stay readable on dark glass.

export interface Theme {
  name: string;
  colors: string[];
  weights: number[];
}

export interface Rgb {
  r: number;
  g: number;
  b: number;
}

export const DEFAULT_THEME: Theme = Object.freeze({
  name: 'Cloudy',
  colors: ['#38bdf8', '#ffffff', '#bae6fd'],
  weights: [45, 35, 20]
}) as Theme;

export const PRESET_THEMES: Theme[] = [
  DEFAULT_THEME,
  { name: 'Monochrome Noir', colors: ['#ffffff', '#94a3b8', '#1e293b'], weights: [60, 25, 15] },
  { name: 'Aurora', colors: ['#22d3ee', '#a855f7', '#22c55e'], weights: [45, 35, 20] },
  { name: 'Sunset', colors: ['#fb7185', '#f59e0b', '#a855f7'], weights: [50, 30, 20] },
  { name: 'Ocean', colors: ['#38bdf8', '#2563eb', '#14b8a6'], weights: [50, 30, 20] },
  { name: 'Forest', colors: ['#4ade80', '#16a34a', '#facc15'], weights: [55, 30, 15] },
  { name: 'Royal', colors: ['#c084fc', '#6366f1', '#f472b6'], weights: [50, 30, 20] }
];

const HEX = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i;
const MIN_ACCENT_LUMINANCE = 0.4;

export function normalizeHex(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const match = HEX.exec(value.trim());
  if (!match) return null;
  const digits = match[1].length === 3 ? match[1].replace(/./g, '$&$&') : match[1];
  return `#${digits.toLowerCase()}`;
}

export function parseHex(hex: string): Rgb {
  const n = parseInt(hex.slice(1), 16);
  return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 };
}

export function rgba(hex: string, alpha: number): string {
  const { r, g, b } = parseHex(hex);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

export function luminance({ r, g, b }: Rgb): number {
  const linear = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
}

function toHsl({ r, g, b }: Rgb) {
  const rn = r / 255;
  const gn = g / 255;
  const bn = b / 255;
  const max = Math.max(rn, gn, bn);
  const min = Math.min(rn, gn, bn);
  const l = (max + min) / 2;
  const d = max - min;
  if (d === 0) return { h: 0, s: 0, l };
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h: number;
  if (max === rn) h = (gn - bn) / d + (gn < bn ? 6 : 0);
  else if (max === gn) h = (bn - rn) / d + 2;
  else h = (rn - gn) / d + 4;
  return { h: h * 60, s, l };
}

function fromHsl(h: number, s: number, l: number): string {
  const a = s * Math.min(l, 1 - l);
  const channel = (n: number) => {
    const k = (n + h / 30) % 12;
    const value = l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1));
    return Math.round(value * 255).toString(16).padStart(2, '0');
  };
  return `#${channel(0)}${channel(8)}${channel(4)}`;
}

/** Dark accents are lifted until they stay readable on the dark glass. */
export function readableAccent(hex: string): string {
  const rgb = parseHex(hex);
  if (luminance(rgb) >= MIN_ACCENT_LUMINANCE) return hex;
  const { h, s } = toHsl(rgb);
  return fromHsl(h, Math.min(s, 0.85), 0.78);
}

/** Cleans a theme from settings or the assistant: valid hex only, weights normalised to percentages. */
export function normalizeTheme(input: Partial<Theme> | null | undefined): Theme {
  const colors = (Array.isArray(input?.colors) ? input!.colors : [])
    .map(normalizeHex)
    .filter((c): c is string => Boolean(c))
    .slice(0, 3);
  if (!colors.length) return { ...DEFAULT_THEME, colors: [...DEFAULT_THEME.colors], weights: [...DEFAULT_THEME.weights] };
  const given = Array.isArray(input?.weights) ? input!.weights.slice(0, colors.length) : [];
  const usable = given.length === colors.length && given.every((w) => Number.isFinite(w) && w > 0);
  const raw = usable ? given : colors.map(() => 1);
  const total = raw.reduce((sum, w) => sum + w, 0);
  const name = typeof input?.name === 'string' ? input.name.replace(/[\u0000-\u001f<>]/g, '').trim().slice(0, 40) : '';
  return { name: name || 'Custom', colors, weights: raw.map((w) => Math.round((w / total) * 1000) / 10) };
}

/** CSS custom properties for a theme. */
export function themeVariables(theme: Theme): Record<string, string> {
  const t = normalizeTheme(theme);
  const accent = readableAccent(t.colors[0]);
  return {
    '--accent': t.colors[0],
    '--accent-2': t.colors[1] ?? t.colors[0],
    '--accent-3': t.colors[2] ?? t.colors[1] ?? t.colors[0],
    '--accent-text': accent,
    '--glow': rgba(accent, 0.45),
    '--glow-soft': rgba(accent, 0.18)
  };
}

export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement): void {
  for (const [key, value] of Object.entries(themeVariables(theme))) root.style.setProperty(key, value);
}

/** Turns the app's "Reduce motion" setting on or off for this window (see tokens.css). */
export function applyMotion(reduce: boolean, root: HTMLElement = document.documentElement): void {
  root.classList.toggle('reduce-motion', reduce);
}

/** Lets the glass surfaces of a window catch a soft light under the pointer (--shine in tokens.css). */
export function trackShine(root: HTMLElement = document.documentElement): () => void {
  let frame = 0;
  let x = -600;
  let y = -600;
  const paint = () => {
    frame = 0;
    root.style.setProperty('--mx', `${x}px`);
    root.style.setProperty('--my', `${y}px`);
  };
  const move = (event: PointerEvent) => {
    x = event.clientX;
    y = event.clientY;
    if (!frame) frame = requestAnimationFrame(paint);
  };
  const leave = () => {
    x = -600;
    y = -600;
    if (!frame) frame = requestAnimationFrame(paint);
  };
  window.addEventListener('pointermove', move, { passive: true });
  document.addEventListener('pointerleave', leave);
  return () => {
    cancelAnimationFrame(frame);
    window.removeEventListener('pointermove', move);
    document.removeEventListener('pointerleave', leave);
  };
}