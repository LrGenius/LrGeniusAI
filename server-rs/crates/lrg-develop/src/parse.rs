//! Errors and warnings of the readers (the Lua/JSON table form and XMP).
//!
//! A [`ParseError`] means the input as a whole is unusable (not JSON or not
//! well-formed XML, not UTF-8, too large, not a settings table, no
//! `rdf:RDF`). Everything else is a [`ParseWarning`],
//! collected in a list (never a single slot): the value is kept (verbatim if
//! it had to be) and the caller decides how to report it. An unknown key is
//! never an error.

use std::fmt;

use crate::model::FileKind;
use crate::registry::ProcessVersion;

/// The input cannot be read at all.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// Not valid JSON.
    #[error("develop settings are not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Larger than the reader accepts.
    #[error("the input is {size} bytes, more than the {max}-byte limit")]
    TooLarge {
        /// Input size.
        size: usize,
        /// The limit.
        max: usize,
    },
    /// The top level is not a settings table (an object, or `[]` for the
    /// empty table).
    #[error("develop settings must be a JSON object (or [] for none), not {found}")]
    NotATable {
        /// What was found instead (`"a string"`, `"a non-empty array"`, ...).
        found: &'static str,
    },
    /// An XMP file that is not UTF-8 (after an optional byte-order mark).
    #[error("the XMP file is not UTF-8: {0}")]
    Encoding(#[from] std::str::Utf8Error),
    /// An XMP file that is not well-formed XML.
    #[error("the XMP file is not well-formed XML: {0}")]
    Xml(#[from] roxmltree::Error),
    /// An XMP file without an `rdf:RDF` element.
    #[error("the XMP file has no rdf:RDF element")]
    NoRdf,
    /// An XMP file nested deeper than any develop setting goes.
    #[error("the XMP file nests elements more than {max} levels deep")]
    TooDeep {
        /// The limit.
        max: usize,
    },
    /// An XMP file beyond a structural limit no real file comes near (too
    /// many attributes on one element, namespace declarations, or too long
    /// a namespace URI); refused before XML parsing.
    #[error("the XMP file has more than {max} {what}")]
    Limit {
        /// What is counted (`"attributes on one element"`, ...).
        what: &'static str,
        /// The limit.
        max: usize,
    },
}

/// What is wrong with one value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WarningKind {
    /// The registry does not know the key; the value is kept verbatim.
    #[error("unknown key; kept verbatim, never written")]
    UnknownKey,
    /// A registry key with a value of the wrong type (`"abc"` for
    /// `Exposure2012`, `null` inside a curve); kept verbatim, never written.
    #[error("expected {expected}; kept verbatim, never written")]
    WrongType {
        /// The registry's type for the key.
        expected: String,
    },
    /// A non-integer on an integer key; the model holds it rounded
    /// half-to-even.
    #[error("not an integer; rounded to {rounded}")]
    NonIntegerForIntKey {
        /// The rounded value now in the model.
        rounded: i64,
    },
    /// A value outside the key's closed set; kept as it is.
    #[error("not one of the key's known values; kept")]
    ValueNotInSet,
    /// An id or digest that is not 32 hex digits; kept as text.
    #[error("not 32 hex digits; kept as text")]
    NotHex32,
    /// `ProcessVersion` is not of the form `"15.4"`.
    #[error("not a process version")]
    InvalidProcessVersion,
    /// Settings older than PV2012 (`"6.7"`): readable, but not learned.
    #[error("process version {found} is older than PV2012 (6.7); these settings are not learned")]
    UnsupportedProcessVersion {
        /// The version found.
        found: ProcessVersion,
    },
    /// The white-balance keys name a different file kind than the caller;
    /// the keys win.
    #[error("the white-balance keys belong to a {family:?} file, but the caller said {hint:?}; the keys win")]
    FileKindMismatch {
        /// What the caller said.
        hint: FileKind,
        /// What the keys say.
        family: FileKind,
    },
    /// Both raw and non-raw white-balance keys are present.
    #[error("both raw (Temperature/Tint) and non-raw (IncrementalTemperature/IncrementalTint) keys are present")]
    ConflictingFileKind,
    /// `MaskBlendMode`/`MaskValue`/`MaskInverted` are missing or form none of
    /// add, subtract, intersect; kept as they are.
    #[error("MaskBlendMode/MaskValue/MaskInverted form no known combination; kept as they are")]
    UnrecognisedMaskCombine,
    /// The key appears more than once at one level (twice in one
    /// `rdf:Description`, or in two merged descriptions); the first
    /// occurrence is used (typed, or kept verbatim if it did not type), the
    /// later ones are kept verbatim.
    #[error(
        "the key appears more than once; the first value is used, the later one kept verbatim"
    )]
    DuplicateKey,
    /// XMP text that RDF does not allow where it stands (text next to
    /// elements or a container); the text is dropped, the rest is read.
    #[error("text next to elements is not valid RDF; the text is dropped")]
    MalformedRdf,
}

impl WarningKind {
    /// Stable name for counting and tests (`"UnknownKey"`, ...).
    pub fn name(&self) -> &'static str {
        match self {
            WarningKind::UnknownKey => "UnknownKey",
            WarningKind::WrongType { .. } => "WrongType",
            WarningKind::NonIntegerForIntKey { .. } => "NonIntegerForIntKey",
            WarningKind::ValueNotInSet => "ValueNotInSet",
            WarningKind::NotHex32 => "NotHex32",
            WarningKind::InvalidProcessVersion => "InvalidProcessVersion",
            WarningKind::UnsupportedProcessVersion { .. } => "UnsupportedProcessVersion",
            WarningKind::FileKindMismatch { .. } => "FileKindMismatch",
            WarningKind::ConflictingFileKind => "ConflictingFileKind",
            WarningKind::UnrecognisedMaskCombine => "UnrecognisedMaskCombine",
            WarningKind::DuplicateKey => "DuplicateKey",
            WarningKind::MalformedRdf => "MalformedRdf",
        }
    }
}

/// One problem with one value; the reading went on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseWarning {
    /// Where, e.g. `"MaskGroupBasedCorrections[2].CorrectionMasks[0].MaskSubType"`.
    pub path: String,
    /// The key name (last path element).
    pub key: String,
    /// The raw value, shortened, when it helps.
    pub raw: Option<String>,
    /// What is wrong.
    pub kind: WarningKind,
}

impl fmt::Display for ParseWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.kind)?;
        if let Some(raw) = &self.raw {
            write!(f, " (value: {raw})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseWarning {}
