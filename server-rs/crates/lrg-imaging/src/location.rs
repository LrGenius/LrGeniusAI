//! Extract Lightroom's reverse-geocoded location tags for LLM prompt
//! context, from whichever source has them:
//!
//! - IPTC IIM record 2 in a JPEG's APP13 segment, plus GPS from EXIF
//!   (`extract_location_tags`) — works on rendered JPEGs (e.g. Lightroom's
//!   export-fallback path), since Lightroom only bakes its reverse-geocoded
//!   address into IPTC/EXIF at export time.
//! - The `.xmp` sidecar Lightroom writes next to a RAW file
//!   (`extract_location_from_xmp` / `find_sidecar_xmp`) — the only place
//!   this data lives when indexing originals directly, since re-encoding a
//!   RAW's preview to JPEG for the AI pipeline strips all metadata.
//!
//! `extract_location` combines both: sidecar XMP first (when a source path
//! is given), falling back to the JPEG-embedded tags.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::reader::Reader;

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

/// Returns the local part of a (possibly namespace-prefixed) XML name, e.g.
/// `b"photoshop:City"` -> `"City"`.
fn xml_local_name(qname: &[u8]) -> &str {
    let s = std::str::from_utf8(qname).unwrap_or("");
    s.rsplit(':').next().unwrap_or(s)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum XmpField {
    Location,
    City,
    State,
    Country,
    CountryCode,
    GpsLatitude,
    GpsLongitude,
}

impl XmpField {
    fn from_local_name(name: &str) -> Option<Self> {
        match name {
            "Location" => Some(Self::Location),
            "City" => Some(Self::City),
            "State" | "Province-State" => Some(Self::State),
            "Country" | "CountryName" => Some(Self::Country),
            "CountryCode" => Some(Self::CountryCode),
            "GPSLatitude" => Some(Self::GpsLatitude),
            "GPSLongitude" => Some(Self::GpsLongitude),
            _ => None,
        }
    }
}

/// Parses XMP's sexagesimal GPS coordinate strings, e.g. `"47,51.29N"`
/// (degrees, decimal minutes) or `"47,51,30N"` (degrees, minutes, decimal
/// seconds) — see the XMP spec's `exif:GPSLatitude`/`GPSLongitude` format.
/// Returns a signed decimal-degrees value (negative for S/W).
fn parse_xmp_gps_component(raw: &str) -> Option<f64> {
    let s = raw.trim();
    let (num_part, dir) = s.split_at(s.char_indices().last()?.0);
    let dir_char = dir.chars().next()?.to_ascii_uppercase();
    if !matches!(dir_char, 'N' | 'S' | 'E' | 'W') {
        return None;
    }
    let parts: Vec<&str> = num_part.split(',').collect();
    let magnitude = match parts.as_slice() {
        [deg, min] => deg.trim().parse::<f64>().ok()? + min.trim().parse::<f64>().ok()? / 60.0,
        [deg, min, sec] => {
            deg.trim().parse::<f64>().ok()?
                + min.trim().parse::<f64>().ok()? / 60.0
                + sec.trim().parse::<f64>().ok()? / 3600.0
        }
        _ => return None,
    };
    Some(if matches!(dir_char, 'S' | 'W') {
        -magnitude
    } else {
        magnitude
    })
}

fn apply_xmp_field(
    result: &mut LocationTags,
    lat_raw: &mut Option<String>,
    lon_raw: &mut Option<String>,
    field: XmpField,
    value: String,
) {
    if value.is_empty() {
        return;
    }
    match field {
        XmpField::Location => {
            result.location.get_or_insert(value);
        }
        XmpField::City => {
            result.city.get_or_insert(value);
        }
        XmpField::State => {
            result.state.get_or_insert(value);
        }
        XmpField::Country => {
            result.country.get_or_insert(value);
        }
        XmpField::CountryCode => {
            result.country_code.get_or_insert(value);
        }
        XmpField::GpsLatitude => {
            lat_raw.get_or_insert(value);
        }
        XmpField::GpsLongitude => {
            lon_raw.get_or_insert(value);
        }
    }
}

/// Applies any location-field attributes (`photoshop:City="..."` etc.) on
/// a `Start`/`Empty` tag — the form Lightroom actually writes.
fn apply_attributes(
    reader: &Reader<&[u8]>,
    e: &quick_xml::events::BytesStart<'_>,
    result: &mut LocationTags,
    lat_raw: &mut Option<String>,
    lon_raw: &mut Option<String>,
) {
    for attr in e.attributes().flatten() {
        let Some(field) = XmpField::from_local_name(xml_local_name(attr.key.as_ref())) else {
            continue;
        };
        let Ok(value) =
            attr.decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
        else {
            continue;
        };
        apply_xmp_field(result, lat_raw, lon_raw, field, value.trim().to_string());
    }
}

/// Parses Lightroom's `.xmp` sidecar XML for reverse-geocoded location
/// fields (`photoshop:City/State/Country`, `Iptc4xmpCore:CountryCode`,
/// `Iptc4xmpCore:Location`) and GPS (`exif:GPSLatitude/GPSLongitude`).
/// Lightroom writes these as RDF attributes on `rdf:Description`, but this
/// also accepts the child-element form (`<photoshop:City>value</...>`)
/// other XMP writers use.
pub fn extract_location_from_xmp(xmp_bytes: &[u8]) -> Option<LocationTags> {
    let mut reader = Reader::from_reader(xmp_bytes);
    reader.config_mut().trim_text(true);

    let mut result = LocationTags::default();
    let mut lat_raw: Option<String> = None;
    let mut lon_raw: Option<String> = None;
    // The element whose text content we're accumulating, plus the buffer
    // itself — quick-xml 0.41 splits `&entity;` refs out of `Text` events
    // into separate `GeneralRef` events, so a single child-element value
    // like `<photoshop:City>M&#252;nchen</photoshop:City>` arrives as
    // Text("M") + GeneralRef("#252") + Text("nchen") and has to be
    // reassembled before it means anything.
    let mut current_field: Option<XmpField> = None;
    let mut current_text = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Err(_) => break,
            Ok(Event::Start(e)) => {
                apply_attributes(&reader, &e, &mut result, &mut lat_raw, &mut lon_raw);
                current_field = XmpField::from_local_name(xml_local_name(e.name().as_ref()));
                current_text.clear();
            }
            Ok(Event::Empty(e)) => {
                // Self-closing tags carry attributes but no text child, so
                // don't open a text-accumulation context for them — there's
                // no matching `End` event to flush it.
                apply_attributes(&reader, &e, &mut result, &mut lat_raw, &mut lon_raw);
            }
            Ok(Event::Text(t)) => {
                if current_field.is_some() {
                    if let Ok(decoded) = t.xml_content(quick_xml::XmlVersion::Implicit1_0) {
                        current_text.push_str(&decoded);
                    }
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if current_field.is_some() {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        current_text.push(c);
                    } else if let Ok(name) = r.decode() {
                        if let Some(resolved) = quick_xml::escape::resolve_predefined_entity(&name)
                        {
                            current_text.push_str(resolved);
                        }
                    }
                }
            }
            Ok(Event::End(_)) => {
                if let Some(field) = current_field.take() {
                    apply_xmp_field(
                        &mut result,
                        &mut lat_raw,
                        &mut lon_raw,
                        field,
                        current_text.trim().to_string(),
                    );
                }
                current_text.clear();
            }
            _ => {}
        }
        buf.clear();
    }

    result.gps_latitude = lat_raw
        .as_deref()
        .and_then(parse_xmp_gps_component)
        .map(round6);
    result.gps_longitude = lon_raw
        .as_deref()
        .and_then(parse_xmp_gps_component)
        .map(round6);

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Looks for `<basename>.xmp` next to `original_path` (Lightroom's sidecar
/// naming convention), trying both common extension cases.
pub fn find_sidecar_xmp(original_path: &Path) -> Option<PathBuf> {
    let stem = original_path.file_stem()?.to_string_lossy().into_owned();
    let dir = original_path.parent()?;
    ["xmp", "XMP"]
        .into_iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|candidate| candidate.is_file())
}

/// Combines both location sources for a photo being indexed: the `.xmp`
/// sidecar next to `original_path` (if given and present — the only place
/// reverse-geocoded address data survives when indexing a RAW directly),
/// falling back to whatever IPTC/EXIF is embedded in `image_bytes` (the
/// JPEG actually sent to the AI pipeline).
pub fn extract_location(original_path: Option<&str>, image_bytes: &[u8]) -> Option<LocationTags> {
    if let Some(sidecar) = original_path.and_then(|p| find_sidecar_xmp(Path::new(p))) {
        match std::fs::read(&sidecar).map(|bytes| extract_location_from_xmp(&bytes)) {
            Ok(Some(tags)) => return Some(tags),
            Ok(None) => {}
            Err(e) => log::warn!("Failed to read XMP sidecar {}: {e}", sidecar.display()),
        }
    }
    extract_location_tags(image_bytes)
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

    #[test]
    fn parses_lightroom_xmp_attributes() {
        let xmp = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
    xmlns:Iptc4xmpCore="http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    photoshop:City="Prien am Chiemsee"
    photoshop:State="Bayern"
    photoshop:Country="Deutschland"
    Iptc4xmpCore:CountryCode="DE"
    Iptc4xmpCore:Location="Chiemsee"
    exif:GPSLatitude="47,51.29N"
    exif:GPSLongitude="12,21.94E">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;
        let tags = extract_location_from_xmp(xmp).unwrap();
        assert_eq!(tags.city.as_deref(), Some("Prien am Chiemsee"));
        assert_eq!(tags.state.as_deref(), Some("Bayern"));
        assert_eq!(tags.country.as_deref(), Some("Deutschland"));
        assert_eq!(tags.country_code.as_deref(), Some("DE"));
        assert_eq!(tags.location.as_deref(), Some("Chiemsee"));
        assert_eq!(tags.gps_latitude, Some(47.854833));
        assert_eq!(tags.gps_longitude, Some(12.365667));
    }

    #[test]
    fn parses_xmp_child_element_form() {
        let xmp = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/">
    <photoshop:City>M&#252;nchen</photoshop:City>
    <exif:GPSLatitude>48,8,37S</exif:GPSLatitude>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let tags = extract_location_from_xmp(xmp).unwrap();
        assert_eq!(tags.city.as_deref(), Some("München"));
        assert!((tags.gps_latitude.unwrap() - (-48.14361)).abs() < 1e-4);
    }

    #[test]
    fn xmp_with_no_location_fields_returns_none() {
        let xmp = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about=""/></rdf:RDF></x:xmpmeta>"#;
        assert!(extract_location_from_xmp(xmp).is_none());
    }

    #[test]
    fn xmp_gps_component_parsing() {
        assert!((parse_xmp_gps_component("47,51.29N").unwrap() - 47.854833).abs() < 1e-6);
        assert!((parse_xmp_gps_component("12,21.94W").unwrap() - (-12.365667)).abs() < 1e-6);
        assert!((parse_xmp_gps_component("48,8,37S").unwrap() - (-48.14361)).abs() < 1e-4);
        assert_eq!(parse_xmp_gps_component("not-gps"), None);
    }

    #[test]
    fn find_sidecar_xmp_matches_basename_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let raw_path = dir.path().join("IMG_1234.CR2");
        std::fs::write(&raw_path, b"raw").unwrap();
        assert!(find_sidecar_xmp(&raw_path).is_none());

        let sidecar_path = dir.path().join("IMG_1234.xmp");
        std::fs::write(&sidecar_path, b"<xmp/>").unwrap();
        assert_eq!(find_sidecar_xmp(&raw_path), Some(sidecar_path));
    }

    #[test]
    fn extract_location_prefers_sidecar_over_embedded_iptc() {
        let dir = tempfile::tempdir().unwrap();
        let raw_path = dir.path().join("IMG_5678.CR2");
        std::fs::write(&raw_path, b"raw").unwrap();
        std::fs::write(
            dir.path().join("IMG_5678.xmp"),
            br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" photoshop:City="Sidecar City"/></rdf:RDF></x:xmpmeta>"#,
        )
        .unwrap();

        let jpeg = jpeg_with_iptc(&[iptc_dataset(IPTC_CITY_ID, "Embedded City")]);
        let tags = extract_location(Some(raw_path.to_str().unwrap()), &jpeg).unwrap();
        assert_eq!(tags.city.as_deref(), Some("Sidecar City"));

        // No sidecar present -> falls back to the embedded IPTC tags.
        let no_sidecar = dir.path().join("IMG_9999.CR2");
        std::fs::write(&no_sidecar, b"raw").unwrap();
        let tags = extract_location(Some(no_sidecar.to_str().unwrap()), &jpeg).unwrap();
        assert_eq!(tags.city.as_deref(), Some("Embedded City"));
    }
}
