//! Pictures that only Windows can open: HEIC/HEIF (phone photos), AVIF,
//! JPEG XL and camera RAW files.
//!
//! These are decoded by the Windows Imaging Component, the same codecs that
//! the Photos app and File Explorer use. Windows ships some of them and
//! offers the rest as free extensions in the Microsoft Store ("HEIF Image
//! Extensions", "AV1 Video Extension", "Raw Image Extension"). When the codec
//! for a file is missing the file is reported as unreadable, nothing else.

use crate::util::extension;
use std::path::Path;

/// File types that are handed to Windows instead of the built-in decoders.
pub const EXTENSIONS: [&str; 16] = [
    "heic", "heif", "hif", "avif", "jxl", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf",
    "pef", "srw", "nrw",
];

pub fn handles(path: &Path) -> bool {
    EXTENSIONS.contains(&extension(path).as_str())
}

/// A picture decoded by Windows, already scaled down.
pub struct Decoded {
    pub image: image::RgbaImage,
    /// Size of the original picture.
    pub full_width: u32,
    pub full_height: u32,
}

/// Sizes that keep the picture's shape and fit `max_side` (never enlarging).
pub fn fit(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let max_side = max_side.max(1);
    if width <= max_side && height <= max_side {
        return (width.max(1), height.max(1));
    }
    let scale = max_side as f64 / width.max(height) as f64;
    (
        ((width as f64 * scale).round() as u32).clamp(1, max_side),
        ((height as f64 * scale).round() as u32).clamp(1, max_side),
    )
}

/// Decodes any picture Windows has a codec for and scales it to fit
/// `max_side` x `max_side`.
#[cfg(windows)]
pub fn decode(path: &Path, max_side: u32) -> Result<Decoded, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory,
        WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
        WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };

    thread_local! {
        // COM is set up once per worker thread and stays for the thread's life.
        static COM: () = {
            // SAFETY: plain COM initialisation; a thread that already chose
            // another mode keeps it, which is fine for in-process WIC.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
        };
    }
    COM.with(|_| ());

    let missing = |e: windows::core::Error| {
        format!(
            "Windows could not open this picture type (the codec may be missing from the Microsoft Store): {}",
            e.message()
        )
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: COM calls on objects created here; `wide` outlives the call that
    // reads it and the pixel buffer has exactly the size WIC is told.
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| format!("Windows imaging is not available: {}", e.message()))?;
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(missing)?;
        let frame = decoder.GetFrame(0).map_err(missing)?;
        let (mut full_width, mut full_height) = (0u32, 0u32);
        frame
            .GetSize(&mut full_width, &mut full_height)
            .map_err(missing)?;
        if full_width == 0 || full_height == 0 || full_width > 65_000 || full_height > 65_000 {
            return Err("the picture has an impossible size".into());
        }
        let (w, h) = fit(full_width, full_height, max_side.clamp(8, 4096));
        let scaler = factory.CreateBitmapScaler().map_err(missing)?;
        scaler
            .Initialize(&frame, w, h, WICBitmapInterpolationModeFant)
            .map_err(missing)?;
        let converter = factory.CreateFormatConverter().map_err(missing)?;
        converter
            .Initialize(
                &scaler,
                &GUID_WICPixelFormat32bppRGBA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(missing)?;
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        converter
            .CopyPixels(std::ptr::null(), w * 4, &mut pixels)
            .map_err(missing)?;
        let image = image::RgbaImage::from_raw(w, h, pixels)
            .ok_or_else(|| "the decoded picture has the wrong size".to_string())?;
        Ok(Decoded {
            image,
            full_width,
            full_height,
        })
    }
}

#[cfg(not(windows))]
pub fn decode(_: &Path, _: u32) -> Result<Decoded, String> {
    Err("this picture type needs Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_its_file_types() {
        assert!(handles(Path::new("X:\\Phone\\IMG_0042.HEIC")));
        assert!(handles(Path::new("X:\\Camera\\DSC0001.nef")));
        assert!(!handles(Path::new("X:\\Photos\\beach.jpg")));
        assert!(!handles(Path::new("X:\\Photos\\noext")));
    }

    #[test]
    fn fit_keeps_the_shape() {
        assert_eq!(fit(4000, 3000, 1000), (1000, 750));
        assert_eq!(fit(3000, 4000, 1000), (750, 1000));
        assert_eq!(fit(640, 480, 1000), (640, 480));
        assert_eq!(fit(10_000, 1, 100), (100, 1));
        assert_eq!(fit(0, 0, 100), (1, 1));
    }

    /// The Windows decoding path itself, proven on a format every Windows
    /// has a codec for (the store codecs cannot be assumed on a test PC).
    #[cfg(windows)]
    #[test]
    fn windows_decodes_and_scales_a_picture() {
        let dir = std::env::temp_dir().join(format!("twinforge-wic-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.png");
        let mut img = image::RgbImage::new(320, 200);
        for (x, _, px) in img.enumerate_pixels_mut() {
            *px = if x < 160 {
                image::Rgb([250, 10, 10])
            } else {
                image::Rgb([10, 10, 250])
            };
        }
        img.save(&path).unwrap();

        let small = decode(&path, 64).expect("Windows decodes PNG");
        assert_eq!((small.full_width, small.full_height), (320, 200));
        assert_eq!((small.image.width(), small.image.height()), (64, 40));
        let left = small.image.get_pixel(8, 20);
        let right = small.image.get_pixel(56, 20);
        assert!(left[0] > 200 && left[2] < 60 && left[3] == 255, "{left:?}");
        assert!(right[2] > 200 && right[0] < 60, "{right:?}");

        let full = decode(&path, 4096).unwrap();
        assert_eq!((full.image.width(), full.image.height()), (320, 200));

        std::fs::write(dir.join("broken.heic"), b"this is not a picture").unwrap();
        assert!(decode(&dir.join("broken.heic"), 64).is_err());
        assert!(decode(&dir.join("missing.heic"), 64).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
