//! The people who matter in a culling selection.
//!
//! Aftershoot calls them key faces and Narrative main faces: the couple at a
//! wedding, the child at a birthday, the player a parent came to see. Culling
//! otherwise counts faces by size, so a large guest in the foreground speaks for
//! a frame and the bride's blink behind them does not.
//!
//! Who counts, and why it is decided over the whole selection rather than per
//! photo: a key person is someone the photographer kept photographing, and
//! that only shows across the shoot. Two kinds:
//!
//! - **named** in People — someone the photographer took the trouble to name
//!   and who is in these photos;
//! - **frequent** — in at least [`KeyPeopleRule::min_share`] of the selection's
//!   photos that have faces, and in at least [`KeyPeopleRule::min_photos`].
//!
//! The thresholds are a first guess, not a measurement: no public data set has
//! a photographer's key people, so no benchmark can say what they should be.

use std::collections::{HashMap, HashSet};

/// When someone appears often enough to count.
#[derive(Debug, Clone, Copy)]
pub struct KeyPeopleRule {
    /// Share of the selection's photos with faces a person must be in.
    pub min_share: f64,
    /// And at least this many photos, so a selection of four does not make
    /// everyone in two of them key.
    pub min_photos: usize,
}

impl KeyPeopleRule {
    pub const fn defaults() -> Self {
        KeyPeopleRule {
            min_share: 0.2,
            min_photos: 3,
        }
    }
}

impl Default for KeyPeopleRule {
    fn default() -> Self {
        Self::defaults()
    }
}

/// The key people of a selection. `appearances` maps each person to the
/// distinct photos they are in; `named` are the persons with a name in People;
/// `photos_with_faces` is how many photos of the selection have any face.
pub fn key_persons(
    appearances: &HashMap<String, HashSet<String>>,
    named: &HashSet<String>,
    photos_with_faces: usize,
    rule: &KeyPeopleRule,
) -> HashSet<String> {
    let needed = ((photos_with_faces as f64) * rule.min_share)
        .ceil()
        .max(rule.min_photos as f64) as usize;
    appearances
        .iter()
        .filter(|(person, photos)| {
            !photos.is_empty() && (named.contains(*person) || photos.len() >= needed)
        })
        .map(|(person, _)| person.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(pairs: &[(&str, &[&str])]) -> HashMap<String, HashSet<String>> {
        pairs
            .iter()
            .map(|(p, photos)| {
                (
                    p.to_string(),
                    photos.iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn the_couple_and_the_named_guest_count_and_a_passer_by_does_not() {
        // Twenty photos with faces: the couple in most, a named aunt in one, a
        // guest in two.
        let couple: Vec<String> = (0..12).map(|i| format!("p{i}")).collect();
        let couple: Vec<&str> = couple.iter().map(String::as_str).collect();
        let appearances = seen(&[
            ("person_1", &couple),
            ("person_2", &couple[..8]),
            ("person_3", &["p15"]),
            ("person_4", &["p16", "p17"]),
        ]);
        let named: HashSet<String> = ["person_3".to_string()].into();
        let key = key_persons(&appearances, &named, 20, &KeyPeopleRule::defaults());
        let mut key: Vec<&str> = key.iter().map(String::as_str).collect();
        key.sort_unstable();
        assert_eq!(key, ["person_1", "person_2", "person_3"]);
    }

    #[test]
    fn a_small_selection_needs_three_photos() {
        let appearances = seen(&[("person_1", &["a", "b"]), ("person_2", &["a", "b", "c"])]);
        let key = key_persons(&appearances, &HashSet::new(), 4, &KeyPeopleRule::defaults());
        assert_eq!(key, ["person_2".to_string()].into());
    }
}
