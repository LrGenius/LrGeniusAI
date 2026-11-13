//! Semantic (AI) mask components: typed people parts and landscape
//! classes, and neutral English names close to the ones Adobe's presets
//! give them (Adobe's own Subject and Sky presets say "Subject 1"/"Sky 1").
//!
//! | [`Semantic`] | `MaskSubType` | `MaskSubCategoryID` | Policy |
//! |---|---|---|---|
//! | `Subject` | 1 | – | LEARN |
//! | `Sky` | 2 | – | LEARN |
//! | `Background` | 0 | 22 | LEARN |
//! | `PeoplePart(part)` (preset form, "all people" unconfirmed) | 3 | [`PeoplePart`] | LEARN |
//! | `Landscape(class)` | 0 | [`LandscapeClass`] | LEARN |
//! | `PersonPartAt { part, point }` | 0 | [`PeoplePart`] + a real `ReferencePoint` | PHOTO, unverified until E2/E8 |

use super::BuildError;
use crate::model::correction::{Semantic, SensorPoint};

/// A people part (`MaskSubCategoryID` 2–9, 11, 12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PeoplePart {
    /// 2.
    FaceSkin,
    /// 3.
    IrisAndPupil,
    /// 4.
    BodySkin,
    /// 5.
    Hair,
    /// 6.
    Lips,
    /// 7.
    FacialHair,
    /// 8.
    EyeSclera,
    /// 9.
    Eyebrows,
    /// 11.
    Clothes,
    /// 12.
    Teeth,
}

impl PeoplePart {
    /// Every part, in id order.
    pub const ALL: [PeoplePart; 10] = [
        PeoplePart::FaceSkin,
        PeoplePart::IrisAndPupil,
        PeoplePart::BodySkin,
        PeoplePart::Hair,
        PeoplePart::Lips,
        PeoplePart::FacialHair,
        PeoplePart::EyeSclera,
        PeoplePart::Eyebrows,
        PeoplePart::Clothes,
        PeoplePart::Teeth,
    ];

    /// `MaskSubCategoryID`.
    pub fn id(self) -> i64 {
        match self {
            PeoplePart::FaceSkin => 2,
            PeoplePart::IrisAndPupil => 3,
            PeoplePart::BodySkin => 4,
            PeoplePart::Hair => 5,
            PeoplePart::Lips => 6,
            PeoplePart::FacialHair => 7,
            PeoplePart::EyeSclera => 8,
            PeoplePart::Eyebrows => 9,
            PeoplePart::Clothes => 11,
            PeoplePart::Teeth => 12,
        }
    }

    /// The part with this `MaskSubCategoryID`.
    pub fn from_id(id: i64) -> Option<PeoplePart> {
        PeoplePart::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The `MaskName` Adobe's portrait presets use.
    pub fn mask_name(self) -> &'static str {
        match self {
            PeoplePart::FaceSkin => "Face Skin",
            PeoplePart::IrisAndPupil => "Iris and Pupil",
            PeoplePart::BodySkin => "Body Skin",
            PeoplePart::Hair => "Hair",
            PeoplePart::Lips => "Lips",
            PeoplePart::FacialHair => "Facial Hair",
            PeoplePart::EyeSclera => "Eye Sclera",
            PeoplePart::Eyebrows => "Eyebrows",
            PeoplePart::Clothes => "Clothes",
            PeoplePart::Teeth => "Teeth",
        }
    }
}

/// A landscape class (`MaskSubCategoryID` 50001–50008).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LandscapeClass {
    /// 50001.
    Architecture,
    /// 50002.
    Mountains,
    /// 50003.
    ArtificialGround,
    /// 50004.
    NaturalGround,
    /// 50005.
    Vegetation,
    /// 50006 (a class of its own, not [`Semantic::Sky`]).
    Sky,
    /// 50007.
    Water,
    /// 50008.
    Snow,
}

impl LandscapeClass {
    /// Every class, in id order.
    pub const ALL: [LandscapeClass; 8] = [
        LandscapeClass::Architecture,
        LandscapeClass::Mountains,
        LandscapeClass::ArtificialGround,
        LandscapeClass::NaturalGround,
        LandscapeClass::Vegetation,
        LandscapeClass::Sky,
        LandscapeClass::Water,
        LandscapeClass::Snow,
    ];

    /// `MaskSubCategoryID`.
    pub fn id(self) -> i64 {
        50001
            + LandscapeClass::ALL
                .iter()
                .position(|c| *c == self)
                .expect("listed") as i64
    }

    /// The class with this `MaskSubCategoryID`.
    pub fn from_id(id: i64) -> Option<LandscapeClass> {
        LandscapeClass::ALL.into_iter().find(|c| c.id() == id)
    }

    /// The `MaskName` Adobe's landscape presets use.
    pub fn mask_name(self) -> &'static str {
        match self {
            LandscapeClass::Architecture => "Architecture",
            LandscapeClass::Mountains => "Mountains",
            LandscapeClass::ArtificialGround => "Artificial Ground",
            LandscapeClass::NaturalGround => "Natural Ground",
            LandscapeClass::Vegetation => "Vegetation",
            LandscapeClass::Sky => "Sky",
            LandscapeClass::Water => "Water",
            LandscapeClass::Snow => "Snow",
        }
    }
}

impl Semantic {
    /// A people part of everyone in the photo (the preset form,
    /// `MaskSubType` 3; "everyone" is unconfirmed until the hand test).
    pub fn people_part(part: PeoplePart) -> Semantic {
        Semantic::PeoplePart(part.id())
    }

    /// A landscape class.
    pub fn landscape(class: LandscapeClass) -> Semantic {
        Semantic::Landscape(class.id())
    }

    /// A people part of the one person at `point` (PHOTO; unverified until
    /// experiments E2/E8).
    pub fn person_part_at(part: PeoplePart, point: SensorPoint) -> Semantic {
        Semantic::PersonPartAt {
            part: part.id(),
            point,
        }
    }
}

/// The neutral English `MaskName` of a semantic component, checking its
/// category id.
pub(super) fn mask_name(s: Semantic) -> Result<&'static str, BuildError> {
    let part = |variant, id| {
        PeoplePart::from_id(id)
            .map(PeoplePart::mask_name)
            .ok_or(BuildError::InvalidCategory { variant, id })
    };
    match s {
        Semantic::Subject => Ok("Subject"),
        Semantic::Sky => Ok("Sky"),
        Semantic::Background => Ok("Background"),
        Semantic::PeoplePart(id) => part("PeoplePart", id),
        Semantic::PersonPartAt { part: id, .. } => part("PersonPartAt", id),
        Semantic::Landscape(id) => LandscapeClass::from_id(id)
            .map(LandscapeClass::mask_name)
            .ok_or(BuildError::InvalidCategory {
                variant: "Landscape",
                id,
            }),
    }
}
