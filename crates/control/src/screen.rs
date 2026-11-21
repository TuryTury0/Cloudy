//! Screenshots, scaled down by the OS while copying (fast, good quality).

use crate::marks::Rgb;

/// Captures the primary screen at most `max_width` pixels wide. Also returns the screen size the coordinates of
/// UI Automation use, so control positions can be turned into 0..1000.
#[cfg(windows)]
pub fn capture_primary(max_width: u32) -> Result<(Rgb, (i32, i32)), String> {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: reads system metrics only.
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    if sw <= 0 || sh <= 0 {
        return Err("no screen".into());
    }
    Ok((capture_rect(0, 0, sw, sh, max_width)?, (sw, sh)))
}

/// Captures just the window in front (clipped to the primary screen); used to tell when it has stopped changing,
/// without being fooled by animations elsewhere on the screen. Falls back to the whole screen.
#[cfg(windows)]
pub fn capture_foreground(max_width: u32) -> Result<Rgb, String> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: reads the rectangle of the foreground window into a local value.
    let (rect, sw, sh) = unsafe {
        let mut rect = RECT::default();
        let ok = GetWindowRect(GetForegroundWindow(), &mut rect).is_ok();
        (ok.then_some(rect), GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
    };
    match rect {
        Some(r) => {
            let (l, t, rr, b) = (r.left.max(0), r.top.max(0), r.right.min(sw), r.bottom.min(sh));
            if rr - l >= 16 && b - t >= 16 { capture_rect(l, t, rr - l, b - t, max_width) } else { capture_primary(max_width).map(|(img, _)| img) }
        }
        None => capture_primary(max_width).map(|(img, _)| img),
    }
}

/// Copies a screen rectangle into an RGB picture at most `max_width` wide.
#[cfg(windows)]
fn capture_rect(x: i32, y: i32, w: i32, h: i32, max_width: u32) -> Result<Rgb, String> {
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        GetDIBits, HALFTONE, ReleaseDC, SRCCOPY, SelectObject, SetStretchBltMode, StretchBlt,
    };

    // SAFETY: plain GDI calls on handles created and released in this function; buffers are sized for the bitmap.
    unsafe {
        let tw = (w as u32).min(max_width.max(16)) as i32;
        let th = (((h as i64) * tw as i64) / w.max(1) as i64).max(1) as i32;
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, tw, th);
        let old = SelectObject(memory, bitmap.into());
        SetStretchBltMode(memory, HALFTONE);
        let copied = StretchBlt(memory, 0, 0, tw, th, Some(screen), x, y, w, h, SRCCOPY | CAPTUREBLT);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: tw,
                biHeight: -th,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (tw * th * 4) as usize];
        let lines = GetDIBits(memory, bitmap, 0, th as u32, Some(bgra.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        SelectObject(memory, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        if !copied.as_bool() || lines == 0 {
            return Err("the screen could not be copied".into());
        }
        let mut pixels = Vec::with_capacity((tw * th * 3) as usize);
        for px in bgra.chunks_exact(4) {
            pixels.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        Ok(Rgb { width: tw as u32, height: th as u32, pixels })
    }
}

#[cfg(not(windows))]
pub fn capture_primary(_max_width: u32) -> Result<(Rgb, (i32, i32)), String> {
    Err("Screen capture is not supported on this system yet.".into())
}

#[cfg(not(windows))]
pub fn capture_foreground(_max_width: u32) -> Result<Rgb, String> {
    Err("Screen capture is not supported on this system yet.".into())
}
