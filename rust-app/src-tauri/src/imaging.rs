use anyhow::{ensure, Result};
use image::{
    imageops::{crop_imm, resize, FilterType},
    RgbaImage,
};
use std::{io::Cursor, path::Path};

pub fn load(path: &Path) -> Result<RgbaImage> {
    ensure!(path.is_file(), "图片不存在");
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    let mut reader = reader;
    reader.limits(limits);
    Ok(reader.decode()?.to_rgba8())
}
pub fn region(source: &RgbaImage, x: f64, y: f64, w: f64, h: f64, scale: f64) -> RgbaImage {
    let px = (x * source.width() as f64).round() as u32;
    let py = (y * source.height() as f64).round() as u32;
    let width = ((w * source.width() as f64).round() as u32).min(source.width() - px);
    let height = ((h * source.height() as f64).round() as u32).min(source.height() - py);
    let crop = crop_imm(source, px, py, width, height).to_image();
    resize(
        &crop,
        (width as f64 * scale).round() as u32,
        (height as f64 * scale).round() as u32,
        FilterType::Triangle,
    )
}
pub fn png(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

pub fn number_region(source: &RgbaImage, x: f64, y: f64, w: f64, h: f64) -> RgbaImage {
    // Preserve the existing WPF preprocessing order: crop, invert maximum
    // channel to grayscale, scale 3x, then surround by 16 white pixels.
    let mut gray = region(source, x, y, w, h, 1.0);
    for p in gray.pixels_mut() {
        let value = 255 - p[0].max(p[1]).max(p[2]);
        *p = image::Rgba([value, value, value, 255]);
    }
    let enlarged = resize(
        &gray,
        gray.width() * 3,
        gray.height() * 3,
        FilterType::Triangle,
    );
    let mut padded = RgbaImage::from_pixel(
        enlarged.width() + 32,
        enlarged.height() + 32,
        image::Rgba([255, 255, 255, 255]),
    );
    image::imageops::replace(&mut padded, &enlarged, 16, 16);
    padded
}
