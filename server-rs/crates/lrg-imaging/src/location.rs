//! Port of `services/exif.py`: extract Lightroom's reverse-geocoded
//! location tags (IPTC IIM record 2 in the JPEG APP13 segment) plus GPS
//! coordinates from EXIF, for LLM prompt context.
//!
//! The IPTC path only sees what is embedded in a JPEG. Lightroom keeps the
//! place a photo was taken in its catalog and writes it into an XMP sidecar
//! next to a raw original, so [`read_sidecar_location`] reads that too — for a
//! raw workflow it is the only copy of the location that exists on disk.

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
    /// Set when the place names above were *derived* from the coordinates by
    /// [`crate::geocode::fill_place_from_gps`] rather than read off the photo:
    /// how far the named place is, in kilometres.
    ///
    /// The prompt needs the difference. A city the photographer put on the
    /// photo is a fact about it; the nearest town to a GPS fix is an estimate,
    /// and a model told the estimate as fact will write a caption about a
    /// harbour the photo does not show.
    pub gps_place_distance_km: Option<f64>,
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

    /// Whether anything here names a place, as opposed to locating one.
    /// Coordinates alone do not: they are what reverse geocoding consumes.
    pub fn has_place_name(&self) -> bool {
        self.location.is_some()
            || self.city.is_some()
            || self.state.is_some()
            || self.country.is_some()
    }

    /// Fills empty fields from `other`, keeping everything already set.
    ///
    /// Used to layer the sources by how much each knows about the
    /// photographer's intent: what the plugin read out of the catalog first,
    /// then what the file itself carries.
    ///
    /// Coordinates fill in freely — a position is a position. Place *names* do
    /// not: two sources that disagree describe two different places, and
    /// filling the gaps between them invents a third. A catalog that says
    /// "Sankt Peter-Ording, Germany" merged field-wise with a file that says
    /// "Ribadeo, Galicia, Spain" yields "Sankt Peter-Ording, Galicia, Germany",
    /// which is nowhere. So the names are only filled in when the two agree on
    /// every name they both carry; otherwise ours stand alone.
    pub fn merge_missing(&mut self, other: &LocationTags) {
        if self.names_agree_with(other) {
            fn fill(target: &mut Option<String>, source: &Option<String>) {
                if target.is_none() {
                    target.clone_from(source);
                }
            }
            fill(&mut self.location, &other.location);
            fill(&mut self.city, &other.city);
            fill(&mut self.state, &other.state);
            fill(&mut self.country, &other.country);
            fill(&mut self.country_code, &other.country_code);
        }
        if self.gps_latitude.is_none() && self.gps_longitude.is_none() {
            self.gps_latitude = other.gps_latitude;
            self.gps_longitude = other.gps_longitude;
        }
    }

    /// Whether two sources contradict each other on any place name they both
    /// carry. Compared case-insensitively and ignoring surrounding space:
    /// "spain" and "Spain " are the same claim.
    fn names_agree_with(&self, other: &LocationTags) -> bool {
        fn same(a: &Option<String>, b: &Option<String>) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
                _ => true,
            }
        }
        same(&self.location, &other.location)
            && same(&self.city, &other.city)
            && same(&self.state, &other.state)
            && same(&self.country, &other.country)
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

/// The largest sidecar we will read. An XMP sidecar is a few kilobytes;
/// anything past this is not one, and we are about to hold it in memory for
/// every photo of a batch.
const MAX_SIDECAR_BYTES: u64 = 4 * 1024 * 1024;

/// Unescapes the five XML predefined entities. XMP values are attribute or
/// element text, so nothing else can appear in them.
fn unescape_xml(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Reads one qualified XMP property, in either shape Lightroom may write it:
/// an attribute on `rdf:Description`, or its own element. An `rdf:Alt`
/// language table (which is how a localised value is stored) yields its first
/// `rdf:li`.
fn xmp_value(xmp: &str, qualified_name: &str) -> Option<String> {
    if let Some(value) = xmp
        .split_once(&format!("{qualified_name}=\""))
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value)
    {
        let value = unescape_xml(value).trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }

    let open = format!("<{qualified_name}>");
    let close = format!("</{qualified_name}>");
    let inner = xmp
        .split_once(&open)
        .and_then(|(_, rest)| rest.split_once(&close))
        .map(|(inner, _)| inner)?;
    // <dc:title><rdf:Alt><rdf:li xml:lang="x-default">Text</rdf:li></rdf:Alt>
    let inner = match inner.split_once("<rdf:li") {
        Some((_, rest)) => rest
            .split_once('>')
            .and_then(|(_, text)| text.split_once("</rdf:li>"))
            .map(|(text, _)| text)?,
        None => inner,
    };
    let value = unescape_xml(inner).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Parses an XMP GPS coordinate: `"43,32.220000N"` (degrees, decimal minutes)
/// or `"43,32,13N"` (degrees, minutes, seconds), the two forms the XMP
/// specification allows. A plain decimal is accepted too, because some writers
/// emit one.
fn parse_xmp_coordinate(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    let (numbers, hemisphere) = match raw.chars().last() {
        Some(c) if matches!(c.to_ascii_uppercase(), 'N' | 'S' | 'E' | 'W') => {
            (&raw[..raw.len() - c.len_utf8()], c.to_ascii_uppercase())
        }
        _ => (raw, 'N'),
    };

    let mut parts = numbers.split(',');
    let degrees: f64 = parts.next()?.trim().parse().ok()?;
    let minutes: f64 = match parts.next() {
        Some(m) => m.trim().parse().ok()?,
        None => 0.0,
    };
    let seconds: f64 = match parts.next() {
        Some(s) => s.trim().parse().ok()?,
        None => 0.0,
    };
    if parts.next().is_some() {
        return None;
    }

    let magnitude = degrees.abs() + minutes / 60.0 + seconds / 3600.0;
    let negative = matches!(hemisphere, 'S' | 'W') || degrees.is_sign_negative();
    Some(if negative { -magnitude } else { magnitude })
}

/// Pulls the location out of XMP packet text (a sidecar's contents, or an XMP
/// block lifted from a file). Returns `None` when it holds no location.
pub fn extract_location_from_xmp(xmp: &str) -> Option<LocationTags> {
    let mut tags = LocationTags {
        location: xmp_value(xmp, "Iptc4xmpCore:Location"),
        city: xmp_value(xmp, "photoshop:City"),
        state: xmp_value(xmp, "photoshop:State"),
        country: xmp_value(xmp, "photoshop:Country"),
        country_code: xmp_value(xmp, "Iptc4xmpCore:CountryCode"),
        ..Default::default()
    };

    let latitude = xmp_value(xmp, "exif:GPSLatitude")
        .as_deref()
        .and_then(parse_xmp_coordinate);
    let longitude = xmp_value(xmp, "exif:GPSLongitude")
        .as_deref()
        .and_then(parse_xmp_coordinate);
    if let (Some(lat), Some(lon)) = (latitude, longitude) {
        tags.gps_latitude = Some(round6(lat));
        tags.gps_longitude = Some(round6(lon));
    }

    (!tags.is_empty()).then_some(tags)
}

/// Reads the location from the XMP sidecar belonging to `image_path`.
///
/// This is the raw shooter's copy of the location: normalising a raw original
/// re-encodes it to a metadata-free JPEG, and a raw container never carried
/// Lightroom's IPTC place names in the first place — they live in the catalog
/// and, when the user has sidecars turned on, in the `.xmp` beside the file.
///
/// Both conventions are tried: `IMG_1234.xmp` (Lightroom's, replacing the
/// extension) and `IMG_1234.CR2.xmp` (appended, used by several other tools).
pub fn read_sidecar_location(image_path: &std::path::Path) -> Option<LocationTags> {
    let mut candidates = vec![
        image_path.with_extension("xmp"),
        image_path.with_extension("XMP"),
    ];
    let appended = {
        let mut name = image_path.file_name()?.to_os_string();
        name.push(".xmp");
        image_path.with_file_name(name)
    };
    candidates.push(appended);

    for candidate in candidates {
        if candidate == image_path {
            continue;
        }
        let readable = std::fs::metadata(&candidate)
            .map(|m| m.is_file() && m.len() <= MAX_SIDECAR_BYTES)
            .unwrap_or(false);
        if !readable {
            continue;
        }
        match std::fs::read_to_string(&candidate) {
            Ok(text) => {
                if let Some(tags) = extract_location_from_xmp(&text) {
                    log::debug!("Read location from sidecar {}", candidate.display());
                    return Some(tags);
                }
            }
            Err(e) => log::debug!("Could not read sidecar {}: {e}", candidate.display()),
        }
    }
    None
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

    /// The shape Lightroom writes: GPS as degrees + decimal minutes with a
    /// hemisphere letter, and — because the user never confirmed the address
    /// suggestions — no city, state or country at all. Coordinates are the
    /// only thing there is, which is what reverse geocoding is for.
    #[test]
    fn reads_gps_from_a_lightroom_sidecar_without_place_names() {
        let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    exif:GPSVersionID="2.3.0.0"
    exif:GPSLatitude="48,27.5784N"
    exif:GPSLongitude="12,5.3578E"
    exif:GPSAltitude="50493/100"/>
 </rdf:RDF>
</x:xmpmeta>"#;
        let tags = extract_location_from_xmp(xmp).unwrap();
        assert!(!tags.has_place_name());
        assert_eq!(tags.gps_latitude, Some(48.45964));
        assert_eq!(tags.gps_longitude, Some(12.089297));
    }

    #[test]
    fn reads_place_names_from_a_sidecar() {
        let xmp = r#"<rdf:Description rdf:about=""
    photoshop:City="Ribadeo" photoshop:State="Galicia"
    photoshop:Country="Spain" Iptc4xmpCore:CountryCode="ES">
    <Iptc4xmpCore:Location>Praia das Catedrais &amp; cliffs</Iptc4xmpCore:Location>
</rdf:Description>"#;
        let tags = extract_location_from_xmp(xmp).unwrap();
        assert_eq!(tags.city.as_deref(), Some("Ribadeo"));
        assert_eq!(tags.state.as_deref(), Some("Galicia"));
        assert_eq!(tags.country.as_deref(), Some("Spain"));
        assert_eq!(tags.country_code.as_deref(), Some("ES"));
        assert_eq!(
            tags.location.as_deref(),
            Some("Praia das Catedrais & cliffs")
        );
        assert!(tags.has_place_name());
    }

    #[test]
    fn xmp_without_location_is_none() {
        assert!(extract_location_from_xmp("<rdf:Description dc:title=\"x\"/>").is_none());
        assert!(extract_location_from_xmp("").is_none());
    }

    #[test]
    fn coordinate_forms_and_hemispheres() {
        assert_eq!(parse_xmp_coordinate("48,27.5784N"), Some(48.45964));
        assert_eq!(parse_xmp_coordinate("7,2,30W"), Some(-7.041666666666667));
        assert_eq!(parse_xmp_coordinate("43.5370"), Some(43.5370));
        assert_eq!(parse_xmp_coordinate("-7.0409"), Some(-7.0409));
        assert!(parse_xmp_coordinate("north-ish").is_none());
        assert!(parse_xmp_coordinate("1,2,3,4N").is_none());
    }

    #[test]
    fn sidecar_is_found_next_to_the_original() {
        let dir = std::env::temp_dir().join(format!("lrg-sidecar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_1234.CR2");
        std::fs::write(&raw, b"not really a raw file").unwrap();
        assert!(read_sidecar_location(&raw).is_none());

        std::fs::write(
            dir.join("IMG_1234.xmp"),
            r#"<rdf:Description photoshop:City="Ribadeo"/>"#,
        )
        .unwrap();
        let tags = read_sidecar_location(&raw).unwrap();
        assert_eq!(tags.city.as_deref(), Some("Ribadeo"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn merge_fills_the_gaps_when_both_sources_agree() {
        let mut catalog = LocationTags {
            city: Some("Ribadeo".into()),
            ..Default::default()
        };
        catalog.merge_missing(&LocationTags {
            city: Some("ribadeo ".into()),
            state: Some("Galicia".into()),
            country: Some("Spain".into()),
            gps_latitude: Some(43.5),
            gps_longitude: Some(-7.0),
            ..Default::default()
        });
        assert_eq!(catalog.city.as_deref(), Some("Ribadeo"));
        assert_eq!(catalog.state.as_deref(), Some("Galicia"));
        assert_eq!(catalog.country.as_deref(), Some("Spain"));
        assert_eq!(catalog.gps_latitude, Some(43.5));
    }

    /// Two sources describing different places must not be stitched into a
    /// third that exists nowhere — caught by a live run, which produced
    /// "Sankt Peter-Ording, Galicia, Germany".
    #[test]
    fn merge_never_stitches_two_different_places_together() {
        let mut catalog = LocationTags {
            city: Some("Sankt Peter-Ording".into()),
            country: Some("Germany".into()),
            ..Default::default()
        };
        catalog.merge_missing(&LocationTags {
            city: Some("Ribadeo".into()),
            state: Some("Galicia".into()),
            country: Some("Spain".into()),
            gps_latitude: Some(54.3),
            gps_longitude: Some(8.6),
            ..Default::default()
        });
        assert_eq!(catalog.city.as_deref(), Some("Sankt Peter-Ording"));
        assert_eq!(catalog.state, None);
        assert_eq!(catalog.country.as_deref(), Some("Germany"));
        // The coordinates are not a name and still fill in.
        assert_eq!(catalog.gps_latitude, Some(54.3));
    }

    #[test]
    fn utf8_and_truncation_are_tolerated() {
        let jpeg = jpeg_with_iptc(&[iptc_dataset(IPTC_CITY_ID, "München")]);
        let tags = extract_location_tags(&jpeg).unwrap();
        assert_eq!(tags.city.as_deref(), Some("München"));
    }
}
