// Makes the synthetic "screenshots" of the second check's built-in test (no real screen content): a music player in
// several states, a Save-as dialog, a saved Notepad file, a delete confirmation, in English and Persian. The app
// embeds five of them from apps/desktop/src-tauri/decision-tests (see decision.rs).
//   cd <a folder with playwright-core installed> && node scripts/decision-test-images.mjs
import { chromium } from 'playwright-core';
import { mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const out = join(dirname(fileURLToPath(import.meta.url)), '..', 'apps', 'desktop', 'src-tauri', 'decision-tests');
mkdirSync(out, { recursive: true });

const base = `<style>
  body{margin:0;font-family:'Segoe UI',Tahoma,sans-serif;background:#121212;color:#fff}
  .bar{position:fixed;left:0;right:0;bottom:0;height:90px;background:#181818;border-top:1px solid #333;display:flex;align-items:center;padding:0 24px;gap:24px}
  .cover{width:56px;height:56px;background:linear-gradient(135deg,#e33,#622);border-radius:4px}
  .t b{display:block;font-size:15px}.t span{color:#aaa;font-size:12px}
  .btn{width:38px;height:38px;border-radius:50%;background:#fff;color:#000;display:grid;place-items:center;font-size:18px;margin-left:200px}
  .prog{flex:1;height:4px;background:#444;border-radius:2px}.prog i{display:block;height:100%;background:#1db954;border-radius:2px}
  .side{position:fixed;left:0;top:0;bottom:90px;width:220px;background:#000;padding:20px;color:#bbb;font-size:14px;line-height:2.2}
  .main{margin-left:260px;padding:30px}.row{padding:10px;border-bottom:1px solid #222;display:flex;gap:20px}.row.on{color:#1db954}
  .win{background:#f3f3f3;color:#111;height:100vh}.title{background:#fff;padding:8px 14px;font-size:13px;border-bottom:1px solid #ddd}
  .menu{padding:6px 14px;font-size:13px;color:#333;border-bottom:1px solid #e5e5e5}.text{padding:16px;font-family:Consolas,monospace;font-size:15px;white-space:pre}
  .dlg{position:fixed;left:50%;top:50%;transform:translate(-50%,-50%);width:520px;background:#fff;color:#111;border:1px solid #999;box-shadow:0 20px 60px rgba(0,0,0,.5);border-radius:8px}
  .dlg h3{margin:0;padding:12px 16px;font-size:14px;border-bottom:1px solid #ddd}.dlg .b{padding:18px 16px;font-size:14px}
  .dlg .f{padding:12px 16px;display:flex;gap:10px;justify-content:flex-end;background:#f3f3f3;border-radius:0 0 8px 8px}
  .dlg button{padding:6px 22px;font-size:13px;border:1px solid #999;border-radius:4px;background:#fff}.dlg button.p{background:#0067c0;color:#fff;border-color:#0067c0}
  input{width:340px;padding:5px;font-size:13px}
</style>`;

const player = (song, artist, playing, pos, lang = 'en') => `${base}
  <div class="side">${lang === 'fa' ? 'خانه<br>جستجو<br>کتابخانه شما' : 'Home<br>Search<br>Your Library'}</div>
  <div class="main"><h1>${lang === 'fa' ? 'آهنگ‌های پسندیده' : 'Liked Songs'}</h1>
    <div class="row ${song === 'Believer' ? 'on' : ''}"><span>1</span><span>Believer</span><span>Imagine Dragons</span></div>
    <div class="row ${song === 'Enemy' ? 'on' : ''}"><span>2</span><span>Enemy</span><span>Imagine Dragons</span></div>
    <div class="row ${song === 'Waka Waka' ? 'on' : ''}"><span>3</span><span>Waka Waka</span><span>Shakira</span></div></div>
  <div class="bar"><div class="cover"></div><div class="t"><b>${song}</b><span>${artist}</span></div>
    <div class="btn" title="${playing ? 'Pause' : 'Play'}">${playing ? '&#10074;&#10074;' : '&#9654;'}</div>
    <span>${pos}</span><div class="prog"><i style="width:${playing ? 35 : 0}%"></i></div><span>3:24</span></div>`;

const notepad = (title, text, dialog = '') => `${base}<div class="win"><div class="title">${title}</div>
  <div class="menu">File &nbsp; Edit &nbsp; View</div><div class="text">${text}</div></div>${dialog}`;

const saveAs = `<div class="dlg"><h3>Save as</h3><div class="b">File name: <input value="dollar.txt"><br><br>Save as type: Text documents (*.txt)</div>
  <div class="f"><button class="p">Save</button><button>Cancel</button></div></div>`;
const del = (lang) => lang === 'fa'
  ? `<div class="dlg" dir="rtl"><h3>حذف فایل</h3><div class="b">آیا مطمئن هستید که می‌خواهید «گزارش مالی.docx» را برای همیشه حذف کنید؟</div><div class="f"><button class="p">بله</button><button>خیر</button></div></div>`
  : `<div class="dlg"><h3>Delete File</h3><div class="b">Are you sure you want to permanently delete "report.docx"?</div><div class="f"><button class="p">Yes</button><button>No</button></div></div>`;

const pages = {
  'music-believer-playing': player('Believer', 'Imagine Dragons', true, '1:12'),
  'music-believer-paused': player('Believer', 'Imagine Dragons', false, '0:00'),
  'music-enemy-playing': player('Enemy', 'Imagine Dragons', true, '0:48'),
  'music-believer-playing-fa': player('Believer', 'Imagine Dragons', true, '1:12', 'fa'),
  'notepad-saveas-open': notepad('*Untitled - Notepad', 'Dollar: 1,125,400 rials', saveAs),
  'notepad-saved': notepad('dollar.txt - Notepad', 'Dollar: 1,125,400 rials'),
  'notepad-unsaved-fa': notepad('*Untitled - Notepad', '<div dir="rtl">لیست خرید: نان، شیر، پنیر</div>'),
  'delete-confirm-en': notepad('Documents', '', del('en')),
  'delete-confirm-fa': notepad('Documents', '', del('fa'))
};

const browser = await chromium.launch({ channel: 'chromium-headless-shell' });
const page = await browser.newPage({ viewport: { width: 1024, height: 640 } });
for (const [name, html] of Object.entries(pages)) {
  await page.setContent(html);
  await page.screenshot({ path: join(out, `${name}.jpg`), type: 'jpeg', quality: 80 });
}
await browser.close();
console.log(Object.keys(pages).length, 'images in', out);
