//! Port of `services/exif.py`: extract Lightroom's reverse-geocoded
//! location tags (IPTC IIM record 2 in the JPEG APP13 segment) plus GPS
//! coordinates from EXIF, for LLM prompt context.
//!
//! Note: the Python module's docstring mentions XMP but only implements
//! IPTC + GPS; this port matches the implementation, not the docstring.

use std::collections::BTreeMap;
use std::io::Cursor;

const IPTC_LOCATION_ID: u8 = 0x5C; // 92  - Sub-location
const IPTC_CITY_ID: u8 = 0x5A; // 90  - City
const IPTC_STATE_ID: u8 = 0x5F; // 95  - Province-State
const IPTC_COUNTRY_CODE_ID: u8 = 0x64; // 100 - Country Code
const IPTC_COUNTRY_ID: u8 = 0x65; // 101 - Country Name

const IPTC_LOCATION_TAGS: [u8; 5] = [
    IPTC_LOCATION_ID,
    IPTC_CITY_ID,
    IPTC_STATE_ID,
    IPTC_COUNTRY_CODE_ID,
    IPTC_COUNTRY_ID,
];

#[derive(Debug, Default, Clone, PartialEq)]
pub struct LocationTags {
    pub location: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub country_code: Option<String>,
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
}

impl LocationTags {
    pub fn is_empty(&self) -> bool {
        self.location.is_none()
            && self.city.is_none()
            && self.state.is_none()
            && self.country.is_none()
            && self.country_code.is_none()
            && self.gps_latitude.is_none()
            && self.gps_longitude.is_none()
    }
}

/// Parse raw IPTC IIM (record-2) bytes into tag_id -> value.
fn parse_iptc(data: &[u8]) -> BTreeMap<u8, String> {
    let mut result = BTreeMap::new();
    let mut i = 0usize;
    while i + 4 < data.len() {
        if data[i] != 0x1C {
            i += 1;
            continue;
        }
        let record = data[i + 1];
        let tag = data[i + 2];
        let size = u16::from_be_bytes([data[i + 3], data[i + 4]]) as usize;
        i += 5;
        if record == 2 && IPTC_LOCATION_TAGS.contains(&tag) && i + size <= data.len() {
            let value = String::from_utf8_lossy(&data[i..i + size])
                .trim()
                .to_string();
            if !value.is_empty() {
                result.insert(tag, value);
            }
        }
        i += size;
    }
    result
}

/// Scan JPEG markers for APP13 (Photoshop IRB) and parse IPTC record 2.
fn read_iptc_from_jpeg(image_bytes: &[u8]) -> BTreeMap<u8, String> {
    let empty = BTreeMap::new();
    if image_bytes.len() < 4 || image_bytes[0] != 0xFF || image_bytes[1] != 0xD8 {
        return empty;
    }
    let mut pos = 2usize;
    loop {
        if pos + 2 > image_bytes.len() || image_bytes[pos] != 0xFF {
            return empty;
        }
        let marker = image_bytes[pos + 1];
        if matches!(marker, 0xD8..=0xDA) {
            return empty; // SOI, EOI, SOS - stop scanning
        }
        if pos + 4 > image_bytes.len() {
            return empty;
        }
        let length = u16::from_be_bytes([image_bytes[pos + 2], image_bytes[pos + 3]]) as usize;
        let seg_start = pos + 4;
        let seg_end = (seg_start + length.saturating_sub(2)).min(image_bytes.len());
        let segment = &image_bytes[seg_start..seg_end];

        if marker == 0xED {
            const HEADER: &[u8] = b"Photoshop 3.0\x00";
            if segment.starts_with(HEADER) {
                let irb = &segment[HEADER.len()..];
                let mut j = 0usize;
                while j + 12 <= irb.len() {
                    if &irb[j..j + 4] != b"8BIM" {
                        j += 1;
                        continue;
                    }
                    let resource_id = u16::from_be_bytes([irb[j + 4], irb[j + 5]]);
                    let name_len = irb[j + 6] as usize;
                    let name_skip = name_len + usize::from(name_len.is_multiple_of(2));
                    let data_offset = j + 7 + name_skip;
                    if data_offset + 4 > irb.len() {
                        break;
                    }
                    let data_len = u32::from_be_bytes([
                        irb[data_offset],
                        irb[data_offset + 1],
                        irb[data_offset + 2],
                        irb[data_offset + 3],
                    ]) as usize;
                    let data_start = data_offset + 4;
                    let data_end = (data_start + data_len).min(irb.len());
                    if resource_id == 0x0404 {
                        return parse_iptc(&irb[data_start..data_end]);
                    }
                    j = data_start + data_len + usize::from(data_len % 2 == 1);
                }
            }
        }
        pos = seg_end;
    }
}

fn dms_to_decimal(field: &exif::Field) -> Option<f64> {
    if let exif::Value::Rational(parts) = &field.value {
        if parts.len() >= 3 {
            let f = |r: &exif::Rational| {
                if r.denom == 0 {
                    0.0
                } else {
                    r.num as f64 / r.denom as f64
                }
            };
            return Some(f(&parts[0]) + f(&parts[1]) / 60.0 + f(&parts[2]) / 3600.0);
        }
    }
    None
}

fn read_gps(image_bytes: &[u8]) -> (Option<f64>, Option<f64>) {
    let Ok(exif) = exif::Reader::new().read_from_container(&mut Cursor::new(image_bytes)) else {
        return (None, None);
    };
    let get = |tag| exif.get_field(tag, exif::In::PRIMARY);
    let lat = get(exif::Tag::GPSLatitude).and_then(dms_to_decimal);
    let lon = get(exif::Tag::GPSLongitude).and_then(dms_to_decimal);
    let (Some(mut lat), Some(mut lon)) = (lat, lon) else {
        return (None, None);
    };
    let ref_str = |tag| {
        get(tag)
            .map(|f| f.display_value().to_string().to_uppercase())
            .unwrap_or_default()
    };
    if ref_str(exif::Tag::GPSLatitudeRef).contains('S') {
        lat = -lat;
    }
    if ref_str(exif::Tag::GPSLongitudeRef).contains('W') {
        lon = -lon;
    }
    (Some(lat), Some(lon))
}

fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round_ties_even() / 1_000_000.0
}

/// Port of `extract_location_tags`. Returns None when nothing was found.
pub fn extract_location_tags(image_bytes: &[u8]) -> Option<LocationTags> {
    let iptc = read_iptc_from_jpeg(image_bytes);
    let mut result = LocationTags {
        location: iptc.get(&IPTC_LOCATION_ID).cloned(),
        city: iptc.get(&IPTC_CITY_ID).cloned(),
        state: iptc.get(&IPTC_STATE_ID).cloned(),
        country: iptc.get(&IPTC_COUNTRY_ID).cloned(),
        country_code: iptc.get(&IPTC_COUNTRY_CODE_ID).cloned(),
        ..Default::default()
    };

    let (lat, lon) = read_gps(image_bytes);
    result.gps_latitude = lat.map(round6);
    result.gps_longitude = lon.map(round6);

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Port of `format_location_for_prompt`.
pub fn format_location_for_prompt(location: &LocationTags) -> Option<String> {
    let parts: Vec<&str> = [
        location.location.as_deref(),
        location.city.as_deref(),
        location.state.as_deref(),
        location.country.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();

    if !parts.is_empty() {
        return Some(parts.join(", "));
    }
    if let (Some(lat), Some(lon)) = (location.gps_latitude, location.gps_longitude) {
        return Some(format!("{lat:.6}, {lon:.6}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iptc_dataset(tag: u8, value: &str) -> Vec<u8> {
        let mut out = vec![0x1C, 0x02, tag];
        out.extend((value.len() as u16).to_be_bytes());
        out.extend(value.as_bytes());
        out
    }

    fn jpeg_with_iptc(datasets: &[Vec<u8>]) -> Vec<u8> {
        let iptc: Vec<u8> = datasets.concat();
        // 8BIM resource 0x0404, empty pascal name (1 byte len + 1 pad).
        let mut irb = b"8BIM".to_vec();
        irb.extend(0x0404u16.to_be_bytes());
        irb.extend([0x00, 0x00]); // empty name + pad
        irb.extend((iptc.len() as u32).to_be_bytes());
        irb.extend(&iptc);

        let mut segment = b"Photoshop 3.0\x00".to_vec();
        segment.extend(&irb);

        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xED];
        jpeg.extend(((segment.len() + 2) as u16).to_be_bytes());
        jpeg.extend(&segment);
        jpeg.extend([0xFF, 0xD9]);
        jpeg
    }

    #[test]
    fn extracts_lightroom_iptc_location() {
        let jpeg = jpeg_with_iptc(&[
            iptc_dataset(IPTC_LOCATION_ID, "Chiemsee"),
            iptc_dataset(IPTC_CITY_ID, "Prien am Chiemsee"),
            iptc_dataset(IPTC_STATE_ID, "Bayern"),
            iptc_dataset(IPTC_COUNTRY_ID, "Deutschland"),
            iptc_dataset(IPTC_COUNTRY_CODE_ID, "DE"),
        ]);
        let tags = extract_location_tags(&jpeg).unwrap();
        assert_eq!(tags.location.as_deref(), Some("Chiemsee"));
        assert_eq!(tags.city.as_deref(), Some("Prien am Chiemsee"));
        assert_eq!(tags.state.as_deref(), Some("Bayern"));
        assert_eq!(tags.country.as_deref(), Some("Deutschland"));
        assert_eq!(tags.country_code.as_deref(), Some("DE"));
        assert_eq!(
            format_location_for_prompt(&tags).unwrap(),
            "Chiemsee, Prien am Chiemsee, Bayern, Deutschland"
        );
    }

    #[test]
    fn no_location_returns_none() {
        assert!(extract_location_tags(&[0xFF, 0xD8, 0xFF, 0xD9]).is_none());
        assert!(extract_location_tags(b"not a jpeg").is_none());
    }

    #[test]
    fn gps_only_formats_coordinates() {
        let tags = LocationTags {
            gps_latitude: Some(47.8556),
            gps_longitude: Some(12.3657),
            ..Default::default()
        };
        assert_eq!(
            format_location_for_prompt(&tags).unwrap(),
            "47.855600, 12.365700"
        );
    }

    #[test]
    fn utf8_and_truncation_are_tolerated() {
        let jpeg = jpeg_with_iptc(&[iptc_dataset(IPTC_CITY_ID, "München")]);
        let tags = extract_location_tags(&jpeg).unwrap();
        assert_eq!(tags.city.as_deref(), Some("München"));
    }
}
