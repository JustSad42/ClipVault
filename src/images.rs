//! Pictures, using Windows' own codecs (WIC): decoding, scaling, saving, and the editor's
//! crop, rotate and blur. Pixels are 32-bit BGRA with straight alpha, top-down.

use crate::win::wide;
use std::path::Path;
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IStream, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};
use windows_core::{GUID, Interface, PCWSTR, Result};

#[derive(Clone, PartialEq)]
pub struct Bitmap {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

/// One factory per thread (the thread must have COM initialised).
fn factory() -> Result<IWICImagingFactory> {
    thread_local!(static F: std::cell::OnceCell<IWICImagingFactory> = const { std::cell::OnceCell::new() });
    F.with(|f| {
        if let Some(f) = f.get() {
            return Ok(f.clone());
        }
        let made: IWICImagingFactory = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
        Ok(f.get_or_init(|| made).clone())
    })
}

impl Bitmap {
    /// `source` converted to BGRA, optionally scaled to `size`.
    unsafe fn from_source(source: &IWICBitmapSource, size: Option<(u32, u32)>) -> Result<Bitmap> {
        unsafe {
            let wic = factory()?;
            let converter = wic.CreateFormatConverter()?;
            converter.Initialize(source, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
            let mut out: IWICBitmapSource = converter.cast()?;
            if let Some((w, h)) = size {
                let scaler = wic.CreateBitmapScaler()?;
                scaler.Initialize(&out, w, h, WICBitmapInterpolationModeFant)?;
                out = scaler.cast()?;
            }
            let (mut w, mut h) = (0, 0);
            out.GetSize(&mut w, &mut h)?;
            let len = (w as u64) * (h as u64) * 4;
            // 256 megapixels is far past any screenshot: refuse rather than exhaust memory
            if len == 0 || len > 1 << 30 {
                return Err(windows_core::Error::from_hresult(windows::Win32::Foundation::E_OUTOFMEMORY));
            }
            let mut px = vec![0u8; len as usize];
            out.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
            Ok(Bitmap { w, h, px })
        }
    }

    /// Decodes a file; with `max_side`, no bigger than that on its longest side.
    pub fn load(path: &Path, max_side: Option<u32>) -> Result<Bitmap> {
        let name = wide(&path.to_string_lossy());
        unsafe {
            let decoder = factory()?.CreateDecoderFromFilename(PCWSTR(name.as_ptr()), None, windows::Win32::Foundation::GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
            Self::from_frame(&decoder, 0, max_side)
        }
    }

    pub fn decode(bytes: &[u8], max_side: Option<u32>) -> Result<Bitmap> {
        unsafe {
            let stream = SHCreateMemStream(Some(bytes)).ok_or_else(windows_core::Error::empty)?;
            let decoder = factory()?.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
            Self::from_frame(&decoder, 0, max_side)
        }
    }

    /// The frame of an .ico (or any image file) closest to `want` pixels wide.
    pub fn decode_icon(bytes: &[u8], want: u32) -> Result<Bitmap> {
        unsafe {
            let stream = SHCreateMemStream(Some(bytes)).ok_or_else(windows_core::Error::empty)?;
            let decoder = factory()?.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
            let mut best = (u32::MAX, 0);
            for i in 0..decoder.GetFrameCount()?.min(64) {
                let (mut w, mut h) = (0, 0);
                decoder.GetFrame(i)?.GetSize(&mut w, &mut h)?;
                if w.abs_diff(want) < best.0 {
                    best = (w.abs_diff(want), i);
                }
            }
            Self::from_frame(&decoder, best.1, Some(want * 2))
        }
    }

    unsafe fn from_frame(decoder: &IWICBitmapDecoder, index: u32, max_side: Option<u32>) -> Result<Bitmap> {
        unsafe {
            let frame = decoder.GetFrame(index)?;
            let source: IWICBitmapSource = frame.cast()?;
            let (mut w, mut h) = (0, 0);
            source.GetSize(&mut w, &mut h)?;
            Self::from_source(&source, max_side.and_then(|m| fit(w, h, m)))
        }
    }

    pub fn from_icon(icon: HICON) -> Result<Bitmap> {
        unsafe {
            let made = factory()?.CreateBitmapFromHICON(icon);
            let _ = DestroyIcon(icon);
            Self::from_source(&made?.cast()?, None)
        }
    }

    fn wic(&self) -> Result<IWICBitmapSource> {
        unsafe { factory()?.CreateBitmapFromMemory(self.w, self.h, &GUID_WICPixelFormat32bppBGRA, self.w * 4, &self.px)?.cast() }
    }

    pub fn scaled_to_fit(&self, max_side: u32) -> Result<Bitmap> {
        match fit(self.w, self.h, max_side) {
            Some(size) => unsafe { Self::from_source(&self.wic()?, Some(size)) },
            None => Ok(self.clone()),
        }
    }

    unsafe fn encode(&self, stream: &IStream, container: &GUID, quality: Option<f32>) -> Result<()> {
        unsafe {
            let wic = factory()?;
            let encoder = wic.CreateEncoder(container, std::ptr::null())?;
            encoder.Initialize(stream, WICBitmapEncoderNoCache)?;
            let (mut frame, mut options) = (None, None);
            encoder.CreateNewFrame(&mut frame, &mut options)?;
            let frame = frame.ok_or_else(windows_core::Error::empty)?;
            if let (Some(q), Some(bag)) = (quality, options.as_ref()) {
                use windows::Win32::System::Com::StructuredStorage::PROPBAG2;
                use windows::Win32::System::Variant::VARIANT;
                let mut name = wide("ImageQuality");
                let option = PROPBAG2 { pstrName: windows_core::PWSTR(name.as_mut_ptr()), ..Default::default() };
                let value = VARIANT::from(q);
                let _ = bag.Write(1, &option, &value);
            }
            frame.Initialize(options.as_ref())?;
            frame.SetSize(self.w, self.h)?;
            let mut format = GUID_WICPixelFormat32bppBGRA;
            frame.SetPixelFormat(&mut format)?;
            // WriteSource converts to whatever the encoder settled on (JPEG has no alpha)
            frame.WriteSource(&self.wic()?, std::ptr::null())?;
            frame.Commit()?;
            encoder.Commit()
        }
    }

    pub fn png(&self) -> Result<Vec<u8>> {
        unsafe {
            let stream = SHCreateMemStream(None).ok_or_else(windows_core::Error::empty)?;
            self.encode(&stream, &GUID_ContainerFormatPng, None)?;
            read_all(&stream)
        }
    }

    /// Saves as JPEG when the name ends in .jpg/.jpeg, else PNG.
    pub fn save(&self, path: &Path) -> Result<()> {
        let jpeg = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
        let name = wide(&path.to_string_lossy());
        unsafe {
            let stream = factory()?.CreateStream()?;
            stream.InitializeFromFilename(PCWSTR(name.as_ptr()), GENERIC_WRITE.0)?;
            let (format, quality) = if jpeg { (GUID_ContainerFormatJpeg, Some(0.92)) } else { (GUID_ContainerFormatPng, None) };
            self.encode(&stream.cast()?, &format, quality)
        }
    }

    pub fn rotated(&self) -> Bitmap {
        // 90 degrees clockwise
        let (w, h) = (self.w as usize, self.h as usize);
        let mut px = vec![0u8; self.px.len()];
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = (h - 1 - y, x);
                px[(ny * h + nx) * 4..][..4].copy_from_slice(&self.px[(y * w + x) * 4..][..4]);
            }
        }
        Bitmap { w: self.h, h: self.w, px }
    }

    pub fn cropped(&self, r: Area) -> Bitmap {
        let mut px = Vec::with_capacity(r.w as usize * r.h as usize * 4);
        for y in r.y..r.y + r.h {
            let at = (y as usize * self.w as usize + r.x as usize) * 4;
            px.extend_from_slice(&self.px[at..at + r.w as usize * 4]);
        }
        Bitmap { w: r.w, h: r.h, px }
    }

    /// Pixelates an area so text in it can't be read back.
    pub fn pixelated(&self, r: Area) -> Bitmap {
        let mut out = self.clone();
        let block = (self.w.min(self.h) / 45).max(8);
        let stride = self.w as usize * 4;
        for by in (r.y..r.y + r.h).step_by(block as usize) {
            for bx in (r.x..r.x + r.w).step_by(block as usize) {
                let (x1, y1) = ((bx + block).min(r.x + r.w), (by + block).min(r.y + r.h));
                let mut sum = [0u64; 4];
                let n = u64::from(x1 - bx) * u64::from(y1 - by);
                for y in by..y1 {
                    for x in bx..x1 {
                        let i = y as usize * stride + x as usize * 4;
                        for (c, s) in sum.iter_mut().enumerate() {
                            *s += u64::from(out.px[i + c]);
                        }
                    }
                }
                for y in by..y1 {
                    for x in bx..x1 {
                        let i = y as usize * stride + x as usize * 4;
                        for (c, s) in sum.iter().enumerate() {
                            out.px[i + c] = (s / n.max(1)) as u8;
                        }
                    }
                }
            }
        }
        out
    }

    /// Premultiplied copy, which is what Direct2D draws.
    pub fn premultiplied(&self) -> Vec<u8> {
        let mut px = self.px.clone();
        for p in px.chunks_exact_mut(4) {
            let a = u32::from(p[3]);
            if a != 255 {
                for c in &mut p[..3] {
                    *c = ((u32::from(*c) * a + 127) / 255) as u8;
                }
            }
        }
        px
    }
}

/// The size that fits `w`×`h` within `max_side`, or `None` if it already does.
pub fn fit(w: u32, h: u32, max_side: u32) -> Option<(u32, u32)> {
    let longest = w.max(h);
    if longest <= max_side || longest == 0 {
        return None;
    }
    let k = f64::from(max_side) / f64::from(longest);
    Some((((f64::from(w) * k).round() as u32).max(1), ((f64::from(h) * k).round() as u32).max(1)))
}

/// A part of an image, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Area {
    /// The part between two corners, kept inside a `w`×`h` image; `None` when too small to matter.
    pub fn between(a: (f32, f32), b: (f32, f32), w: u32, h: u32) -> Option<Area> {
        let clamp = |v: f32, max: u32| v.round().clamp(0.0, max as f32) as u32;
        let (x0, x1) = (clamp(a.0.min(b.0), w), clamp(a.0.max(b.0), w));
        let (y0, y1) = (clamp(a.1.min(b.1), h), clamp(a.1.max(b.1), h));
        (x1 - x0 >= 4 && y1 - y0 >= 4).then_some(Area { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

fn read_all(stream: &IStream) -> Result<Vec<u8>> {
    unsafe {
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME)?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let mut out = vec![0u8; stat.cbSize as usize];
        let mut read = 0u32;
        stream.Read(out.as_mut_ptr() as _, out.len() as u32, Some(&mut read)).ok()?;
        out.truncate(read as usize);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> Bitmap {
        let (w, h) = (40u32, 20u32);
        let px = (0..w * h).flat_map(|i| [(i % 251) as u8, 20, 30, 255]).collect();
        Bitmap { w, h, px }
    }

    #[test]
    fn edits() {
        let b = checker();
        let r = b.rotated();
        assert_eq!((r.w, r.h), (20, 40));
        assert!(r.rotated().rotated().rotated() == b, "four turns give the picture back");
        let a = Area::between((30.0, 15.0), (10.0, 5.0), b.w, b.h).unwrap();
        assert_eq!(a, Area { x: 10, y: 5, w: 20, h: 10 });
        assert_eq!(b.cropped(a).px[..4], b.px[(5 * 40 + 10) * 4..][..4]);
        assert_eq!(Area::between((1.0, 1.0), (2.0, 30.0), 40, 20), None);
        let p = b.pixelated(a);
        assert_eq!(p.px[(5 * 40 + 10) * 4], p.px[(6 * 40 + 11) * 4]);
        assert_eq!(p.px[0..40], b.px[0..40], "outside the area is untouched");
        assert_eq!(fit(4000, 2000, 400), Some((400, 200)));
    }

    #[test]
    fn png_round_trip() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let b = checker();
        assert!(Bitmap::decode(&b.png().unwrap(), None).unwrap() == b);
        let small = Bitmap::decode(&b.png().unwrap(), Some(10)).unwrap();
        assert_eq!((small.w, small.h), (10, 5));
        let path = std::env::temp_dir().join(format!("clipvault-test-{}.jpg", std::process::id()));
        b.save(&path).unwrap();
        assert_eq!(Bitmap::load(&path, None).map(|j| (j.w, j.h)).unwrap(), (40, 20));
        let _ = std::fs::remove_file(path);
    }
}
