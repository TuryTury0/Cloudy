// Turns the technical error lines from the core ("Gemini (x): HTTP 401: ...") into short sentences in the app's language.
// The original stays available as `detail` for the chat and the logs.

import { L } from './i18n.svelte';

export interface FriendlyError {
  text: string;
  detail: string;
  /** The fix is in Settings > Models (add a model, a key, or correct the model id). */
  needsModel: boolean;
}

export function friendlyError(message: string): FriendlyError {
  const detail = message.trim();
  const make = (text: string, needsModel = false): FriendlyError => ({ text, detail, needsModel });
  const http = /HTTP (\d{3})/.exec(detail);
  if (detail.includes('Settings > Models')) return make(L('No model is chosen yet; add one in Settings › Models.', 'هنوز مدلی انتخاب نشده؛ از تنظیمات › مدل‌ها یکی اضافه کن.'), true);
  if (detail.includes('No API key is saved')) return make(L('The API key of this model is not saved.', 'کلید API این مدل ذخیره نشده.'), true);
  if (http) {
    const code = Number(http[1]);
    // A refusal of the user's location is the network, not the key (Gemini from Iran without a VPN).
    if (/location is not supported|refused this address|unsupported_country|region is not supported/i.test(detail))
      return make(L('This service does not answer from here: turn on the VPN (or the proxy in Settings › Models).', 'این سرویس از اینجا جواب نمی‌دهد: VPN را روشن کن (یا پروکسی را در تنظیمات › مدل‌ها).'));
    if (code === 401 || code === 403) return make(L(`The API key was not accepted (${code}).`, `کلید API قبول نشد (${code}).`), true);
    if (code === 404) return make(L('The model or address was not found (404); check the model id.', 'مدل یا آدرس پیدا نشد (404)؛ شناسه مدل را چک کن.'), true);
    if (code === 429 && /per ?day/i.test(detail)) return make(L("This model's free daily quota is used up; it works again tomorrow, or choose another model or key in Settings.", 'سهمیه رایگان روزانه این مدل تمام شده؛ فردا دوباره کار می‌کند، یا از تنظیمات مدل یا کلید دیگری انتخاب کن.'), true);
    // No (more) quota for this model on the key: a free key often has none for Flash or Pro, while Flash-Lite works.
    if (code === 429 && /exceeded your current quota|plan and billing/i.test(detail) && !/retry in \d/i.test(detail))
      return make(L("Your key has no quota left for this model (free keys often have little or none for Flash and Pro). Choose a Flash-Lite model, another key, or turn on billing in Google AI Studio.", 'کلید تو برای این مدل سهمیه ندارد (کلیدهای رایگان برای Flash و Pro سهمیه کم یا هیچ دارند). یک مدل Flash-Lite یا کلید دیگری انتخاب کن، یا در Google AI Studio پرداخت را فعال کن.'), true);
    if (code === 429) return make(L('The model hit its rate limit or quota (429); try again in a moment.', 'محدودیت سرعت یا سهمیه مدل پر شده (429)؛ کمی بعد دوباره امتحان کن.'));
    if (code === 503 || /high demand|overloaded/i.test(detail)) return make(L("This model's server is busy right now (503); try again in a moment, or choose a backup model in Settings › Models.", 'سرور این مدل الان شلوغ است (503)؛ کمی بعد دوباره امتحان کن، یا در تنظیمات › مدل‌ها یک «مدل پشتیبان» انتخاب کن.'));
    if (code >= 500) return make(L(`The model's server failed (${code}); try again.`, `سرور مدل خطا داد (${code})؛ دوباره امتحان کن.`));
    if (code === 400) return make(L('The model refused this request (400).', 'مدل این درخواست را نپذیرفت (400).'));
  }
  if (detail.includes('Network error')) return make(L('Could not reach the model; check the internet or the address.', 'اتصال به مدل برقرار نشد؛ اینترنت یا آدرس را چک کن.'));
  if (detail.includes('empty answer')) return make(L('The model gave an empty answer; try again.', 'مدل جواب خالی داد؛ دوباره امتحان کن.'));
  return make(detail);
}
