//! Safe image decoding: format sniffed from bytes (not the URL or headers),
//! dimensions and allocation capped before decoding to resist decompression bombs.

use std::io::Cursor;

use eframe::egui::ColorImage;
use image::{ImageReader, Limits, imageops::FilterType};

const MAX_ALLOC: u64 = 512 * 1024 * 1024;

/// Decodes `bytes`, rejecting images with a side over `max_dim`, then downscales so neither
/// side exceeds `max_side` (the GPU's texture limit).
// shortcut: very tall strips get narrower when downscaled; split into tiles if that bothers readers.
pub fn decode(bytes: &[u8], max_dim: u32, max_side: u32) -> Result<ColorImage, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_dim);
    limits.max_image_height = Some(max_dim);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let mut img = reader.decode().map_err(|e| e.to_string())?;
    if img.width() > max_side || img.height() > max_side {
        img = img.resize(max_side, max_side, FilterType::Triangle);
    }
    let rgba = img.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::new(w, h)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn decodes_enforces_limits_and_fits_texture() {
        assert_eq!(decode(&png(4, 3), 100, 100).unwrap().size, [4, 3]);
        assert!(decode(&png(200, 3), 100, 100).is_err());
        assert!(decode(b"<html>not an image</html>", 100, 100).is_err());
        assert_eq!(decode(&png(40, 200), 1000, 100).unwrap().size, [20, 100]);
    }
}
