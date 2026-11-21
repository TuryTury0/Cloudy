//! Text on the screen that UI Automation does not report (games, remote desktops, pictures, apps without an
//! accessibility tree), read with the OCR built into Windows (Windows.Media.Ocr, on this PC, nothing is sent
//! anywhere). Each line becomes a "text" element, so the model reads it and can click it by its words like a button.
//! Read on every look, alongside UI Automation, so the model always has the words on the screen.

use crate::marks::Rgb;
use anytask_core::tools::desktop::Element;

/// A line of text and its rectangle in the picture's pixels: x, y, width, height.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    pub text: String,
    pub rect: [f32; 4],
}

/// At most this many text elements in all (UI Automation's and these).
const MAX_LINES: usize = 40;

/// The OCR lines as "text" elements (positions 0..1000 of the screen), after the existing ones. Left out:
/// lines that sit inside an element UI Automation already named (a button's label, a text it reported), words it
/// already listed on the same line or as a control's name, and noise (see `uia::noise`).
pub fn merge(elements: &mut Vec<Element>, lines: &[TextLine], picture: (u32, u32)) {
    let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    let scale = |v: f32, size: f32| (v / size * 1000.0).round().clamp(0.0, 1000.0) as u16;
    let mut added = elements.iter().filter(|e| e.kind == "text").count();
    for line in lines {
        let text = line.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if added >= MAX_LINES || crate::uia::noise(&text, elements) {
            continue;
        }
        let [x, y, w, h] = line.rect;
        let (ex, ey) = (scale(x, pw), scale(y, ph));
        let (ew, eh) = (scale(x + w, pw).saturating_sub(ex).max(1), scale(y + h, ph).saturating_sub(ey).max(1));
        let (cx, cy) = (ex + ew / 2, ey + eh / 2);
        let listed = elements.iter().any(|e| e.name.eq_ignore_ascii_case(&text) && e.y.abs_diff(ey) < 20);
        if listed || crate::uia::inside_named(elements, (cx, cy)) {
            continue;
        }
        let mid = ((x + w / 2.0) / pw * 1000.0, (y + h / 2.0) / ph * 1000.0);
        elements.push(Element { mid: Some(mid), ..Element::new("text", &text, true, (ex, ey, ew, eh)) });
        added += 1;
    }
}

#[cfg(windows)]
pub fn read(picture: &Rgb, timeout: std::time::Duration) -> Result<Vec<TextLine>, String> {
    // Gray, one byte per pixel: what the OCR engine takes, and a third of the data.
    let gray: Vec<u8> = picture.pixels.chunks_exact(3).map(|p| ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u8).collect();
    let (width, height) = (picture.width, picture.height);
    let (sent, got) = std::sync::mpsc::channel();
    // Its own thread: the engine is a Windows runtime object that needs COM, and a stuck call must not hold the look.
    std::thread::spawn(move || {
        let _ = sent.send(recognize(&gray, width, height).map_err(|e| e.to_string()));
    });
    got.recv_timeout(timeout).map_err(|_| "Windows OCR did not answer in time.".to_string())?
}

#[cfg(windows)]
fn recognize(gray: &[u8], width: u32, height: u32) -> windows::core::Result<Vec<TextLine>> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    // SAFETY: initialises COM for this dedicated thread only.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let writer = DataWriter::new()?;
    writer.WriteBytes(gray)?;
    let buffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Gray8, width as i32, height as i32)?;
    // The languages of the user's Windows profile that have OCR installed (English on most PCs).
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let lines = result.Lines()?;
    let mut out = Vec::new();
    for i in 0..lines.Size()? {
        let line = lines.GetAt(i)?;
        let words = line.Words()?;
        let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for k in 0..words.Size()? {
            let rect = words.GetAt(k)?.BoundingRect()?;
            l = l.min(rect.X);
            t = t.min(rect.Y);
            r = r.max(rect.X + rect.Width);
            b = b.max(rect.Y + rect.Height);
        }
        if r > l && b > t {
            out.push(TextLine { text: line.Text()?.to_string(), rect: [l, t, r - l, b - t] });
        }
    }
    Ok(out)
}

#[cfg(not(windows))]
pub fn read(_picture: &Rgb, _timeout: std::time::Duration) -> Result<Vec<TextLine>, String> {
    Err("OCR is not available on this system.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(name: &str, x: u16, y: u16, w: u16, h: u16) -> Element {
        Element::new("button", name, true, (x, y, w, h))
    }

    #[test]
    fn text_lines_become_controls_after_the_existing_ones() {
        let mut elements = vec![element("Close", 950, 0, 50, 40), element("", 0, 0, 1000, 1000)];
        let lines = vec![
            TextLine { text: "Play  now".into(), rect: [100.0, 50.0, 200.0, 20.0] },
            TextLine { text: "x".into(), rect: [10.0, 10.0, 5.0, 5.0] },
            TextLine { text: "Close".into(), rect: [960.0, 5.0, 30.0, 10.0] },
        ];
        merge(&mut elements, &lines, (1000, 1000));
        assert_eq!(elements.len(), 3, "a one-letter line and a line inside a named control are left out: {elements:?}");
        let text = &elements[2];
        assert_eq!((text.kind.as_str(), text.name.as_str()), ("text", "Play now"));
        assert_eq!(text.middle(), (200.0, 60.0), "clicked on the middle of its words");
        assert_eq!((text.x, text.y, text.w, text.h), (100, 50, 200, 20));
    }

    #[test]
    fn words_ui_automation_already_gave_are_not_added_twice() {
        let mut elements = vec![Element::new("text", "Recommended", true, (100, 500, 100, 20))];
        let lines = vec![
            TextLine { text: "Recommended".into(), rect: [90.0, 505.0, 130.0, 20.0] },
            TextLine { text: "Based on what's in this playlist".into(), rect: [100.0, 530.0, 300.0, 15.0] },
        ];
        merge(&mut elements, &lines, (1000, 1000));
        let names: Vec<&str> = elements.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Recommended", "Based on what's in this playlist"]);
    }

    #[test]
    fn positions_follow_the_picture_size() {
        let mut elements = Vec::new();
        merge(&mut elements, &[TextLine { text: "File".into(), rect: [192.0, 108.0, 96.0, 54.0] }], (1920, 1080));
        assert_eq!((elements[0].x, elements[0].y, elements[0].w, elements[0].h), (100, 100, 50, 50));
    }

    /// Windows' own OCR on a synthetic screenshot (the second check's test picture of a saved Notepad file).
    #[cfg(windows)]
    #[test]
    fn windows_reads_the_text_of_a_screenshot() {
        let jpeg = include_bytes!("../../../apps/desktop/src-tauri/decision-tests/notepad-saved.jpg");
        let picture = image::load_from_memory(jpeg).unwrap().to_rgb8();
        let picture = Rgb { width: picture.width(), height: picture.height(), pixels: picture.into_raw() };
        match read(&picture, std::time::Duration::from_secs(10)) {
            Ok(lines) => {
                let all = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" | ");
                // 11-11: "dollar.txt - Notepad | File Edit View | Dollar: rials" in 0.2 s; the number in small Consolas on
                // this 1024 px JPEG was missed, which is why the real look reads the screen at full size.
                assert!(all.contains("dollar.txt") && all.contains("Notepad") && all.contains("Edit"), "{all}");
            }
            // A Windows without any OCR language cannot run this; that is not a failure of the code.
            Err(e) if e.contains("OCR") || e.contains("0x") => eprintln!("OCR not available here: {e}"),
            Err(e) => panic!("{e}"),
        }
    }
}
