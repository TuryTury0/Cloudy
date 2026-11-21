//! The rulers on a screenshot (for the few things clicked by X Y; controls are clicked by name and have no boxes or
//! numbers), the change fingerprint, and JPEG encoding. Pure image work, no OS calls.

/// An RGB picture, 3 bytes per pixel, rows top to bottom.
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// 3x5 digits, one row per byte (3 low bits, left to right).
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];
const SCALE: u32 = 2;
const LABEL_H: u32 = 5 * SCALE + 4;

impl Rgb {
    fn put(&mut self, x: u32, y: u32, c: [u8; 3]) {
        if x < self.width && y < self.height {
            let i = ((y * self.width + x) * 3) as usize;
            self.pixels[i..i + 3].copy_from_slice(&c);
        }
    }

    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, c: [u8; 3]) {
        for yy in y..(y + h).min(self.height) {
            for xx in x..(x + w).min(self.width) {
                self.put(xx, yy, c);
            }
        }
    }

    fn number(&mut self, x: u32, y: u32, n: u32, c: [u8; 3]) {
        for (i, ch) in n.to_string().bytes().enumerate() {
            let glyph = DIGITS[(ch - b'0') as usize];
            let gx = x + i as u32 * 4 * SCALE;
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..3 {
                    if bits & (0b100 >> col) != 0 {
                        self.fill(gx + col * SCALE, y + row as u32 * SCALE, SCALE, SCALE, c);
                    }
                }
            }
        }
    }
}

/// Width in pixels of a label for `n`.
fn label_width(n: u32) -> u32 {
    n.to_string().len() as u32 * 4 * SCALE + 3
}

/// Rulers for clicking what has no name: X along the top and Y along the left, both 0..1000 like
/// the model's coordinates, with faint dotted lines every 100. Models read positions off them instead of
/// guessing (some answered in screenshot pixels, which landed far off on a wide screen).
pub fn draw_rulers(image: &mut Rgb) {
    let (w, h) = (image.width, image.height);
    let dot = |image: &mut Rgb, x: u32, y: u32| {
        if x < w && y < h {
            let i = ((y * w + x) * 3) as usize;
            for c in &mut image.pixels[i..i + 3] {
                // Toward mid grey, so the dots show on light and dark screens.
                *c = ((u16::from(*c) + 128) / 2) as u8;
            }
        }
    };
    for step in 1..10u32 {
        let x = step * w / 10;
        let y = step * h / 10;
        for t in (0..h).step_by(4) {
            dot(image, x, t);
        }
        for t in (0..w).step_by(4) {
            dot(image, t, y);
        }
        let label = step * 100;
        image.fill(x + 1, 0, label_width(label), LABEL_H, [20, 20, 20]);
        image.number(x + 3, 2, label, [255, 230, 80]);
        image.fill(0, y + 1, label_width(label), LABEL_H, [20, 20, 20]);
        image.number(2, y + 3, label, [255, 230, 80]);
    }
}

/// Changes whenever the picture changes (FNV-1a over a sample of the pixels).
pub fn fingerprint(image: &Rgb) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in image.pixels.iter().step_by(7) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash ^ (u64::from(image.width) << 32 | u64::from(image.height))
}

pub fn encode_jpeg(image: &Rgb, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode(&image.pixels, image.width, image.height, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("JPEG: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(w: u32, h: u32) -> Rgb {
        Rgb { width: w, height: h, pixels: vec![0; (w * h * 3) as usize] }
    }

    fn at(img: &Rgb, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * img.width + x) * 3) as usize;
        [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2]]
    }

    #[test]
    fn rulers_label_every_100_along_the_top_and_left() {
        let mut img = blank(1000, 500);
        draw_rulers(&mut img);
        let yellow = [255, 230, 80];
        // "500" along the top starts just right of x = 500; along the left just under y = 250.
        assert!((500..540).any(|x| (0..LABEL_H).any(|y| at(&img, x, y) == yellow)));
        assert!((0..40).any(|x| (250..250 + LABEL_H).any(|y| at(&img, x, y) == yellow)));
        // Faint dots on the grid line, the rest of the picture untouched.
        assert_eq!(at(&img, 300, 204), [64, 64, 64]);
        assert_eq!(at(&img, 333, 222), [0, 0, 0]);
    }

    #[test]
    fn rulers_on_a_tiny_picture_do_not_panic() {
        let mut img = blank(50, 30);
        draw_rulers(&mut img);
    }

    #[test]
    fn fingerprint_follows_the_picture_and_jpeg_encodes() {
        let a = blank(64, 48);
        let mut b = blank(64, 48);
        assert_eq!(fingerprint(&a), fingerprint(&blank(64, 48)));
        b.pixels[0] = 200;
        assert_ne!(fingerprint(&a), fingerprint(&b));
        let jpeg = encode_jpeg(&a, 70).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }
}
