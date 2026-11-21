// Small text helpers for the phone app: the Rust side and the PC answer in English, the app speaks Persian.
import { friendlyError } from '@anytask/ui';

const ERRORS: Array<[RegExp, string]> = [
  [/Not connected to the PC/i, 'به کامپیوتر وصل نیست.'],
  [/did not answer/i, 'کامپیوتر جواب نداد؛ دوباره امتحان کن.'],
  [/not reachable/i, 'کامپیوتر پیدا نشد. روشن است و به همین وای‌فای (یا Tailscale) وصل است؟'],
  [/No computer on this network is waiting/i, 'هیچ کامپیوتری روی این وای‌فای منتظر کد نیست. در کامپیوتر: تنظیمات ← گوشی ← «افزودن گوشی».'],
  [/code is not right|expired/i, 'کد درست نیست یا منقضی شده؛ کد روی کامپیوتر را دوباره نگاه کن.'],
  [/code has 6 digits/i, 'کد ۶ رقم دارد.'],
  [/not an? (AnyTask|Cloudy) pairing code|code is damaged/i, 'این متن، کد اتصال Cloudy نیست.'],
  [/key does not match/i, 'کلید کامپیوتر عوض شده؛ دوباره جفت کن.'],
  [/refused the connection/i, 'کامپیوتر اتصال را نپذیرفت.'],
  [/closed the connection/i, 'کامپیوتر اتصال را بست (کامپیوتر درست است؟).'],
  [/phone is not paired/i, 'این گوشی جفت نیست؛ کد را دوباره بزن.'],
  [/Connect to the computer once first/i, 'اول یک بار به کامپیوتر وصل شو تا گوشی یاد بگیرد چطور روشنش کند.'],
  [/No computer is paired/i, 'هنوز کامپیوتری جفت نشده.'],
  [/No file was chosen/i, 'فایلی انتخاب نشد.'],
  [/could not be read from the picker/i, 'فایل از انتخابگر خوانده نشد؛ دوباره امتحان کن.'],
  [/No Persian voice is installed/i, 'صدای فارسی روی گوشی نصب نیست (تنظیمات ← تبدیل متن به گفتار).'],
  [/no speech voice/i, 'گوشی صدای گفتار ندارد.'],
  [/no speech recognition/i, 'این گوشی تشخیص گفتار ندارد؛ برنامه Google را نصب یا به‌روز کن.'],
  [/Speech recognition needs the internet/i, 'تشخیص گفتار به اینترنت نیاز دارد (شاید VPN لازم باشد).'],
  [/microphone could not be used/i, 'میکروفون در دسترس نیست.'],
  [/Could not read that file|file is missing/i, 'این فایل خوانده نشد.'],
  [/Could not save it in Downloads|could not store it/i, 'فایل در گوشی ذخیره نشد.'],
  [/No app on the phone opens/i, 'برنامه‌ای در گوشی این فایل را باز نمی‌کند.'],
  [/That is a folder/i, 'این یک پوشه است.'],
  [/stopped short|damaged piece|from the start|Expected the piece/i, 'انتقال نیمه‌کاره ماند؛ دوباره امتحان کن.'],
  [/Windows refused/i, 'ویندوز قبول نکرد.'],
  [/The PC refused|could not do it/i, 'کامپیوتر نتوانست انجامش دهد.'],
  [/access is denied|permission denied/i, 'اجازه دسترسی نیست.'],
  [/cannot find the path|cannot find the file|not found/i, 'پیدا نشد.']
];

/** A Persian sentence for an error from the Rust side, the PC or Android. */
export function faError(error: unknown): string {
  const text = String(error ?? '')
    .replace(/^Error:\s*/, '')
    .trim();
  if (!text) return 'خطا';
  for (const [pattern, fa] of ERRORS) if (pattern.test(text)) return fa;
  return friendlyError(text).text;
}

const number = new Intl.NumberFormat('fa-IR', { maximumFractionDigits: 1 });
const whole = new Intl.NumberFormat('fa-IR', { maximumFractionDigits: 0 });

/** A number in Persian digits. */
export function fa(n: number, digits = 0): string {
  return digits ? number.format(n) : whole.format(n);
}

/** 1536 -> "۱٫۵ KB". */
export function formatSize(bytes: number | undefined): string {
  if (bytes === undefined || !Number.isFinite(bytes)) return '';
  if (bytes < 1024) return `${whole.format(bytes)} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value >= 100 ? whole.format(value) : number.format(value)} ${units[unit]}`;
}

const day = new Intl.DateTimeFormat('fa-IR', { dateStyle: 'medium' });
const time = new Intl.DateTimeFormat('fa-IR', { hour: '2-digit', minute: '2-digit' });

/** Today: the time; otherwise the date (Persian calendar). */
export function formatDate(ms: number | undefined): string {
  if (!ms) return '';
  const date = new Date(ms);
  return date.toDateString() === new Date().toDateString() ? time.format(date) : day.format(date);
}

/** Persian or Arabic digits typed on the phone's keyboard, as ASCII digits. */
export function asciiDigits(text: string): string {
  return text.replace(/[۰-۹٠-٩]/g, (d) => String((d.charCodeAt(0) & 0xf) % 10));
}

export type FileKind = 'folder' | 'drive' | 'image' | 'video' | 'audio' | 'doc' | 'archive' | 'app' | 'file';

const KINDS: Array<[FileKind, RegExp]> = [
  ['image', /\.(png|jpe?g|gif|webp|bmp|heic|svg|ico|tiff?)$/i],
  ['video', /\.(mp4|mkv|mov|avi|webm|wmv|flv|m4v|3gp)$/i],
  ['audio', /\.(mp3|wav|flac|ogg|m4a|aac|wma|opus)$/i],
  ['doc', /\.(pdf|docx?|xlsx?|pptx?|txt|md|rtf|odt|csv|json|html?|epub)$/i],
  ['archive', /\.(zip|rar|7z|tar|gz|bz2|xz|iso)$/i],
  ['app', /\.(exe|msi|apk|bat|cmd|ps1|lnk|appx|msix)$/i]
];

export function fileKind(name: string, dir: boolean, path = ''): FileKind {
  if (dir) return /^[A-Za-z]:\\?$/.test(path) ? 'drive' : 'folder';
  for (const [kind, pattern] of KINDS) if (pattern.test(name)) return kind;
  return 'file';
}