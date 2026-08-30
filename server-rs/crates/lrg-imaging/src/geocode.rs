//! Offline reverse geocoding: coordinates in, a place name out.
//!
//! Lightroom's own address lookup writes city/state/country into the catalog,
//! but those fields only reach us when they survive to the file we are handed
//! — and an unconfirmed lookup suggestion never does. What is left is a pair
//! of coordinates, and `"43.536700, -7.040950"` in the prompt is worth nothing
//! to a vision model: it will not name Ribadeo, it will write "a rocky beach"
//! (issue #321). Turning the coordinates into a name is what puts the place in
//! the title and the caption.
//!
//! The lookup is offline by design. This is a local-first plugin, and sending
//! every indexed photo's position to a geocoding service is exactly the kind
//! of thing a user who runs a local model is avoiding. The `reverse_geocoder`
//! crate bundles GeoNames' `cities1000` (~145k places worldwide, everything
//! down to villages of a thousand people), which is why a town of 9,000 like
//! Ribadeo resolves at all.
//!
//! Data: GeoNames (<https://www.geonames.org>), CC BY 4.0.

use std::sync::OnceLock;

use reverse_geocoder::ReverseGeocoder;

use crate::location::LocationTags;

/// Beyond this the nearest known settlement says nothing useful about where
/// the photo was taken — mid-ocean, deep desert, high mountains. Naming a town
/// 80 km away is worse than naming nothing, because the model will happily
/// build a caption around it.
const MAX_PLACE_DISTANCE_KM: f64 = 50.0;

/// Within this radius the photo is, for captioning purposes, *in* the place.
/// Further out it is *near* it, and the prompt says so — a photo taken 20 km
/// outside a town is not a photo of that town.
pub const IN_PLACE_DISTANCE_KM: f64 = 5.0;

/// The mean Earth radius, for turning the geocoder's unit-sphere distance
/// back into kilometres.
const EARTH_RADIUS_KM: f64 = 6371.0;

/// The nearest populated place to a coordinate pair.
#[derive(Debug, Clone, PartialEq)]
pub struct NearestPlace {
    pub name: String,
    /// First-level administrative division (state, province, region), empty
    /// when GeoNames has none for the place.
    pub admin1: String,
    /// Second-level division (county, district), empty when unknown.
    pub admin2: String,
    pub country_code: String,
    /// English country name for `country_code`, absent for a code that is not
    /// ISO 3166-1 alpha-2 (GeoNames uses a handful, e.g. `XK` for Kosovo).
    pub country: Option<String>,
    pub distance_km: f64,
}

/// Loaded once, on the first photo of the first run that actually needs it.
///
/// Building it parses ~145k CSV rows into a k-d tree, so it is deliberately
/// not part of server start-up: a catalog without GPS, or a user who switched
/// location context off, never pays for it.
fn geocoder() -> &'static ReverseGeocoder {
    static GEOCODER: OnceLock<ReverseGeocoder> = OnceLock::new();
    GEOCODER.get_or_init(|| {
        let t0 = std::time::Instant::now();
        let geocoder = ReverseGeocoder::new();
        log::debug!(
            "Loaded offline reverse-geocoding index in {:?}",
            t0.elapsed()
        );
        geocoder
    })
}

/// Squared chord length on the unit sphere (what the k-d tree returns) to
/// great-circle kilometres.
fn chord_squared_to_km(squared: f64) -> f64 {
    let chord = squared.max(0.0).sqrt().min(2.0);
    EARTH_RADIUS_KM * 2.0 * (chord / 2.0).asin()
}

/// The nearest populated place, or `None` when the coordinates are not real
/// coordinates or nothing known is close enough to be worth naming.
pub fn nearest_place(latitude: f64, longitude: f64) -> Option<NearestPlace> {
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return None;
    }
    // 0,0 is Null Island: the coordinate a camera or a converter writes when
    // it has no fix, not a photo taken in the Gulf of Guinea.
    if latitude == 0.0 && longitude == 0.0 {
        return None;
    }

    let result = geocoder().search((latitude, longitude));
    let distance_km = chord_squared_to_km(result.distance);
    if distance_km > MAX_PLACE_DISTANCE_KM {
        return None;
    }
    let record = result.record;
    Some(NearestPlace {
        name: record.name.clone(),
        admin1: record.admin1.clone(),
        admin2: record.admin2.clone(),
        country_code: record.cc.clone(),
        country: isocountry::CountryCode::for_alpha2(&record.cc)
            .ok()
            .map(|c| c.name().to_string()),
        distance_km,
    })
}

/// Fills a photo's missing place names from its coordinates, and reports
/// whether it did.
///
/// Only when there is no place name at all: a city the photographer (or
/// Lightroom) put on the photo is a statement about where they were, and the
/// nearest settlement to a GPS fix is a guess. The guess never overwrites the
/// statement, and never mixes with it either — a photo tagged "Ribadeo" does
/// not get a country filled in from a different record.
pub fn fill_place_from_gps(tags: &mut LocationTags) -> bool {
    if tags.has_place_name() {
        return false;
    }
    let (Some(lat), Some(lon)) = (tags.gps_latitude, tags.gps_longitude) else {
        return false;
    };
    let Some(place) = nearest_place(lat, lon) else {
        return false;
    };

    tags.city = Some(place.name);
    if !place.admin1.is_empty() {
        tags.state = Some(place.admin1);
    }
    if let Some(country) = place.country {
        tags.country = Some(country);
    }
    if tags.country_code.is_none() && !place.country_code.is_empty() {
        tags.country_code = Some(place.country_code);
    }
    tags.gps_place_distance_km = Some(place.distance_km);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_town_behind_the_coordinates() {
        // The beach in issue #321.
        let place = nearest_place(43.5370, -7.0409).expect("Ribadeo is in cities1000");
        assert_eq!(place.name, "Ribadeo");
        assert_eq!(place.admin1, "Galicia");
        assert_eq!(place.country.as_deref(), Some("Spain"));
        assert!(place.distance_km < 1.0, "got {} km", place.distance_km);
    }

    #[test]
    fn distance_is_in_kilometres() {
        // ~50 km north of Munich's centre is still within the cap, but is not
        // Munich any more.
        let place = nearest_place(48.6, 11.5).unwrap();
        assert!(place.distance_km < MAX_PLACE_DISTANCE_KM);
        assert_eq!(place.country.as_deref(), Some("Germany"));
    }

    #[test]
    fn mid_ocean_has_no_place() {
        assert!(nearest_place(0.0, -140.0).is_none());
    }

    #[test]
    fn null_island_and_impossible_coordinates_are_rejected() {
        assert!(nearest_place(0.0, 0.0).is_none());
        assert!(nearest_place(91.0, 10.0).is_none());
        assert!(nearest_place(10.0, 200.0).is_none());
    }

    #[test]
    fn fills_only_when_nothing_is_known() {
        let mut tags = LocationTags {
            gps_latitude: Some(43.5370),
            gps_longitude: Some(-7.0409),
            ..Default::default()
        };
        assert!(fill_place_from_gps(&mut tags));
        assert_eq!(tags.city.as_deref(), Some("Ribadeo"));
        assert_eq!(tags.country.as_deref(), Some("Spain"));
        assert!(tags.gps_place_distance_km.is_some());

        let mut tagged = LocationTags {
            city: Some("Ribadeo".into()),
            gps_latitude: Some(48.1372),
            gps_longitude: Some(11.5755),
            ..Default::default()
        };
        assert!(!fill_place_from_gps(&mut tagged));
        assert_eq!(tagged.city.as_deref(), Some("Ribadeo"));
        assert!(tagged.country.is_none());
        assert!(tagged.gps_place_distance_km.is_none());
    }

    #[test]
    fn without_coordinates_nothing_happens() {
        let mut tags = LocationTags::default();
        assert!(!fill_place_from_gps(&mut tags));
        assert!(tags.is_empty());
    }
}
