//! Safe image decoding: format sniffed from bytes (not the URL or headers),
//! dimensions and allocation capped before decoding to resist decompression bombs.

use std::io::Cursor;

use eframe::egui::ColorImage;
use image::{ImageReader, Limits, imageops::FilterType};

const MAX_ALLOC: u64 = 768 * 1024 * 1024;

fn load(bytes: &[u8], max_w: u32, max_h: u32) -> Result<image::DynamicImage, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_w);
    limits.max_image_height = Some(max_h);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    reader.decode().map_err(|e| e.to_string())
}

/// Decodes `bytes`, rejecting images with a side over `max_dim`, then downscales so neither
/// side exceeds `max_side` (the GPU's texture limit).
pub fn decode(bytes: &[u8], max_dim: u32, max_side: u32) -> Result<ColorImage, String> {
    let mut img = load(bytes, max_dim, max_dim)?;
    if img.width() > max_side || img.height() > max_side {
        img = img.resize(max_side, max_side, FilterType::Triangle);
    }
    let rgba = img.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// For reader pages: long webtoon strips (taller than `max_dim`, up to 10x) are allowed and cut
/// into stacked tiles of at most `max_side` tall, so they keep full width instead of being squashed.
pub fn decode_tiles(bytes: &[u8], max_dim: u32, max_side: u32) -> Result<Vec<ColorImage>, String> {
    let mut img = load(bytes, max_dim, max_dim.saturating_mul(10))?;
    if img.width() > max_side {
        img = img.resize(max_side, u32::MAX, FilterType::Triangle);
    }
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((0..h.max(1))
        .step_by(max_side as usize)
        .map(|y| {
            let tile = image::imageops::crop_imm(&rgba, 0, y, w, max_side.min(h - y)).to_image();
            ColorImage::from_rgba_unmultiplied([tile.width() as usize, tile.height() as usize], tile.as_raw())
        })
        .collect())
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
        // a strip taller than max_dim is accepted and tiled at full width
        let tiles = decode_tiles(&png(40, 250), 100, 100).unwrap();
        assert_eq!(tiles.iter().map(|t| t.size).collect::<Vec<_>>(), [[40, 100], [40, 100], [40, 50]]);
        assert!(decode_tiles(&png(200, 3), 100, 100).is_err());
    }
}
