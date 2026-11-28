//! Reading pictures: text with the engine built into Windows, and QR codes. Both work offline.

use crate::images::Bitmap;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

/// Text found in a picture ("" when there is none or no engine is installed).
pub fn read_text(b: &Bitmap) -> String {
    ocr(b).unwrap_or_default()
}

fn ocr(b: &Bitmap) -> windows_core::Result<String> {
    thread_local!(static ENGINE: std::cell::OnceCell<Option<OcrEngine>> = const { std::cell::OnceCell::new() });
    let Some(engine) = ENGINE.with(|e| e.get_or_init(|| OcrEngine::TryCreateFromUserProfileLanguages().ok()).clone()) else { return Ok(String::new()) };
    let max = OcrEngine::MaxImageDimension().unwrap_or(10_000);
    if b.w > max || b.h > max || b.w < 20 || b.h < 20 {
        return Ok(String::new());
    }
    let writer = DataWriter::new()?;
    writer.WriteBytes(&b.premultiplied())?;
    let buffer = writer.DetachBuffer()?;
    let picture = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buffer, BitmapPixelFormat::Bgra8, b.w as i32, b.h as i32, BitmapAlphaMode::Premultiplied)?;
    let result = engine.RecognizeAsync(&picture)?.join()?;
    Ok(result.Text()?.to_string_lossy())
}

/// The text of a QR code in the picture ("" when there is none).
pub fn read_qr(b: &Bitmap) -> String {
    // big screenshots are scaled down for speed; codes stay readable
    let small;
    let b = if b.w.max(b.h) > 1600 {
        match b.scaled_to_fit(1600) {
            Ok(s) => {
                small = s;
                &small
            }
            Err(_) => b,
        }
    } else {
        b
    };
    let (w, h) = (b.w as usize, b.h as usize);
    let mut image = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        let p = &b.px[(y * w + x) * 4..][..4];
        // see-through pixels count as white paper
        let grey = (u32::from(p[2]) * 77 + u32::from(p[1]) * 150 + u32::from(p[0]) * 29) >> 8;
        let a = u32::from(p[3]);
        ((grey * a + 255 * (255 - a)) / 255) as u8
    });
    image.detect_grids().into_iter().find_map(|g| g.decode().ok()).map(|(_, text)| text.trim().to_string()).unwrap_or_default()
}
