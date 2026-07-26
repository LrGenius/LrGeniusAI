//! Port of `providers/base.py::_image_to_base64`: skip the PIL
//! decode/re-encode round-trip when the bytes are already JPEG (magic
//! number `FF D8 FF`), converting everything else to JPEG q95 first.

use base64::Engine;

#[derive(Debug, thiserror::Error)]
#[error("Failed to process image: {0}")]
pub struct ImageEncodeError(String);

pub fn image_to_base64(image_data: &[u8]) -> Result<String, ImageEncodeError> {
    if image_data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok(base64::engine::general_purpose::STANDARD.encode(image_data));
    }
    let img = image::load_from_memory(image_data)
        .map_err(|e| ImageEncodeError(e.to_string()))?
        .to_rgb8();
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 95);
    img.write_with_encoder(encoder)
        .map_err(|e| ImageEncodeError(e.to_string()))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_bytes_pass_through_unmodified() {
        let jpeg = [0xFFu8, 0xD8, 0xFF, 0x11, 0x22];
        let encoded = image_to_base64(&jpeg).unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(decoded, jpeg);
    }

    #[test]
    fn non_jpeg_is_reencoded() {
        let png = image::RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(png)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let encoded = image_to_base64(buf.get_ref()).unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert!(
            decoded.starts_with(&[0xFF, 0xD8, 0xFF]),
            "should be re-encoded as JPEG"
        );
    }
}
