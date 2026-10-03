//! XMP → [`XmpDocument`]: a sidecar, a develop preset or a profile.
//!
//! Two passes. The first reads RDF into the XMP data model
//! ([`XmpNode`]), whatever syntax the file used:
//!
//! - attribute form (`crs:Exposure2012="+0.50"`) and element form
//!   (`<crs:Exposure2012>+0.50</crs:Exposure2012>`), mixed freely;
//! - structures as a nested `rdf:Description`, as attributes on the property
//!   element, or with `rdf:parseType="Resource"`;
//! - `rdf:Seq`, `rdf:Bag` and `rdf:Alt` (empty items included), `xml:lang`,
//!   and qualifiers (`rdf:value`, as an element or an attribute, with
//!   siblings);
//! - every `rdf:Description` under every outermost `rdf:RDF`, merged in
//!   document order.
//!
//! A property element that mixes forms RDF does not allow (a container next
//! to other elements or field attributes, something other than `rdf:li` in
//! a container) is read as a plain structure of everything it holds, so the
//! second pass reports it (`WrongType` or `UnknownKey`) and keeps it whole.
//! Text next to elements cannot be kept in the data model; it is dropped
//! with a [`WarningKind::MalformedRdf`] (one per top-level property).
//! Top-level properties outside `crs:` are counted and never built.
//!
//! The second pass types the `crs:` properties (matched by namespace URI,
//! never by prefix) through the registry, into the same model the Lua reader
//! produces: the same key ids, value variants, correction and mask
//! classification, `Look` and white-balance family (the shared rules are in
//! `crate::reader` and [`crate::model::correction`]). Top-level names resolve
//! at the global level first, then as preset header keys.
//!
//! - `True`/`true`/`False`/`false` are both accepted; `"+15"` is 15,
//!   `"1.000000"` is 1.0; a non-integer on an integer key is rounded
//!   half-to-even with a warning; ids are checked for 32 hex digits;
//! - global curves (`"x, y"`) and local curves (`"x,y"`) are both read into
//!   [`Value::Curve`];
//! - an `rdf:Alt` with only an `x-default` item is [`Value::Alt`]; one with
//!   further languages, an `rdf:Bag` or `rdf:Alt` where the registry expects
//!   an `rdf:Seq`, and any value with qualifiers are kept whole as
//!   [`Opaque::Xmp`] under their key, without a warning (valid XMP the model
//!   does not interpret) but counted in [`SkippedCounts::kept_whole`], so a
//!   caller or a sweep sees the lost typing;
//! - `crss:` (saved snapshots) is skipped entirely; `Look.Parameters` (a
//!   profile definition) and `Preset.Parameters` (a copy of the last applied
//!   preset) are kept as [`Opaque::Xmp`] and never interpreted; properties in
//!   other namespaces (`dc:`, `xmp:`, `exif:`, ...) are not part of the
//!   model and are dropped at the top level. [`SkippedCounts`] counts all of
//!   these;
//! - an unknown `crs:` key is kept verbatim with a
//!   [`WarningKind::UnknownKey`], a known key of the wrong shape with a
//!   [`WarningKind::WrongType`]; neither is an error.
//!
//! [`ParseError`] only for input that is unusable as a whole: more than
//! [`MAX_XMP_BYTES`], not UTF-8 (after an optional byte-order mark), not
//! well-formed XML, no `rdf:RDF`, nesting deeper than [`MAX_XMP_DEPTH`], or
//! beyond one of the structural limits ([`MAX_XMP_ATTRIBUTES`],
//! [`MAX_XMP_NAMESPACES`], [`MAX_XMP_NAMESPACE_URI`]).
//!
//! The depth and structural limits are enforced by a linear, non-recursive
//! pre-scan of the text *before* roxmltree sees it. roxmltree's tokenizer
//! recurses once per element and has no depth limit, so a deeply nested
//! file would overflow the stack (an abort, not a panic) inside
//! `Document::parse`; and it has quadratic paths in its duplicate-attribute
//! check and namespace resolution. The pre-scan refuses such a file first.
//! Namespace URIs are shared ([`Arc<str>`]) rather than copied per field, so
//! a long URI cannot multiply into gigabytes.
//! Everything else is a [`ParseWarning`] with a path such as
//! `MaskGroupBasedCorrections[2].CorrectionMasks[0].MaskSubType`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use roxmltree::{Document, Node, ParsingOptions};

use crate::model::correction::Correction;
use crate::model::value::{Fields, Finite, Hex32, Opaque, OpaqueEntry, Struct, Value};
use crate::model::xmp_node::{XmpArrayKind, XmpField, XmpNode, XmpValue};
use crate::model::{DevelopSettings, FileKindHint};
use crate::parse::{ParseError, ParseWarning, WarningKind};
use crate::reader::{self, child, parse_point, ScalarIn};
use crate::registry::{self, CurveKind, KeySpec, Level, Resolved, StructKind, ValueKind};

/// Largest XMP file [`parse`] accepts (4 MiB, the same limit as the
/// sidecar reader in `lrg-imaging`).
pub const MAX_XMP_BYTES: usize = 4 * 1024 * 1024;

/// Deepest element nesting below an `rdf:Description` that [`parse`]
/// accepts. Real files nest about 20 levels (range masks inside brush
/// aggregates inside corrections). A pre-scan checks the depth before the
/// XML parser runs (its tokenizer recurses once per element), allowing a
/// few more levels for the envelope (`x:xmpmeta`, `rdf:RDF`,
/// `rdf:Description`); the reader's own recursion checks it again. Low
/// enough that a debug build on a 2 MiB thread keeps a real margin.
pub const MAX_XMP_DEPTH: usize = 64;

/// Most attributes one start tag may carry (real files: about 200). Bounds
/// the XML parser's duplicate-attribute check, which is quadratic per
/// element.
pub const MAX_XMP_ATTRIBUTES: usize = 1024;

/// Most namespace declarations (`xmlns`, `xmlns:*`) in one file (real files:
/// about 20). Bounds the XML parser's namespace resolution, which is
/// quadratic in the namespaces in scope.
pub const MAX_XMP_NAMESPACES: usize = 256;

/// Longest namespace URI, in bytes as written (real files: under 60).
pub const MAX_XMP_NAMESPACE_URI: usize = 1024;

/// Elements the pre-scan allows above the first property.
const DEPTH_ENVELOPE: usize = 8;

/// Most XML nodes the parser builds (real files: a few thousand elements).
const MAX_XMP_NODES: u32 = 1 << 18;

/// The Camera Raw settings namespace (`crs:`).
pub const CRS_NS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
/// The Camera Raw saved-settings namespace (`crss:`, snapshots).
pub const CRSS_NS: &str = "http://ns.adobe.com/camera-raw-saved-settings/1.0/";
const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// What an XMP file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum XmpKind {
    /// A photo's sidecar: `crs:ProcessVersion` without `crs:PresetType`.
    Sidecar,
    /// A develop preset: `crs:PresetType="Normal"`.
    Preset,
    /// A profile: `crs:PresetType="Look"`. Its develop settings are the
    /// profile definition.
    Profile,
    /// Anything else (no develop settings, or an unknown preset type). Not
    /// an error: the settings are read all the same.
    NotDevelop,
}

/// The header of a preset or profile file (the `Level::Header` keys).
///
/// The fields Lightroom shows or the writer sets are typed; every other
/// header key, and a typed one whose value did not fit (a name with several
/// languages, an empty `CameraModelRestriction`), stays in [`Self::rest`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresetHeader {
    /// `PresetType` (`"Normal"`, `"Look"`).
    pub preset_type: Option<String>,
    /// `UUID`.
    pub uuid: Option<Hex32>,
    /// `Name` (`x-default`).
    pub name: Option<String>,
    /// `ShortName` (`x-default`).
    pub short_name: Option<String>,
    /// `SortName` (`x-default`).
    pub sort_name: Option<String>,
    /// `Group` (`x-default`).
    pub group: Option<String>,
    /// `Description` (`x-default`).
    pub description: Option<String>,
    /// `Cluster`.
    pub cluster: Option<String>,
    /// `CameraModelRestriction` (`None` also for the empty string, which
    /// stays in [`Self::rest`]).
    pub camera_model_restriction: Option<String>,
    /// `SupportsAmount`.
    pub supports_amount: Option<bool>,
    /// `SupportsAmount2`.
    pub supports_amount2: Option<bool>,
    /// `SupportsColor`.
    pub supports_color: Option<bool>,
    /// `SupportsMonochrome`.
    pub supports_monochrome: Option<bool>,
    /// Every other header key, and what did not fit a typed field.
    pub rest: Fields,
}

impl PresetHeader {
    fn from_fields(mut f: Fields) -> PresetHeader {
        const H: Level = Level::Header;
        let string = |v: &Value| v.as_str().map(str::to_owned);
        let alt = |v: &Value| match v {
            Value::Alt(s) => Some(s.clone()),
            _ => None,
        };
        PresetHeader {
            preset_type: f.take_if(H, "PresetType", string),
            uuid: f.take_if(H, "UUID", |v| v.as_str().and_then(Hex32::parse)),
            name: f.take_if(H, "Name", alt),
            short_name: f.take_if(H, "ShortName", alt),
            sort_name: f.take_if(H, "SortName", alt),
            group: f.take_if(H, "Group", alt),
            description: f.take_if(H, "Description", alt),
            cluster: f.take_if(H, "Cluster", string),
            camera_model_restriction: f.take_if(H, "CameraModelRestriction", |v| {
                v.as_str().filter(|r| !r.is_empty()).map(str::to_owned)
            }),
            supports_amount: f.take_if(H, "SupportsAmount", Value::as_bool),
            supports_amount2: f.take_if(H, "SupportsAmount2", Value::as_bool),
            supports_color: f.take_if(H, "SupportsColor", Value::as_bool),
            supports_monochrome: f.take_if(H, "SupportsMonochrome", Value::as_bool),
            rest: f,
        }
    }
}

/// Subtrees the parser skipped on purpose, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkippedCounts {
    /// `crss:` properties (snapshots).
    pub crss: usize,
    /// `Preset.Parameters` kept opaque (a copy of the last applied preset).
    pub preset_parameters: usize,
    /// `Look.Parameters` kept opaque (a profile definition).
    pub look_parameters: usize,
    /// Top-level properties in other namespaces (`dc:`, `xmp:`, `tiff:`,
    /// ...), not part of the develop model.
    pub other_namespaces: usize,
    /// Registry keys whose value came in a form the model does not type (an
    /// `rdf:Bag`, an `rdf:Alt` with more than `x-default`, qualifiers) and
    /// was kept whole as [`Opaque::Xmp`]: valid XMP, so not a warning, but
    /// typing was lost. Lightroom's own files never do this; the local
    /// sweeps require 0. The by-design opaque kinds (`Look.Parameters`,
    /// `Preset.Parameters`, keys the registry keeps opaque) are not counted.
    pub kept_whole: usize,
}

/// One parsed XMP file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmpDocument {
    /// Sidecar, preset, profile or none of them.
    pub kind: XmpKind,
    /// The preset/profile header; `None` when the file has no header key.
    pub header: Option<PresetHeader>,
    /// The develop settings (for a profile: its definition).
    pub develop: DevelopSettings,
    /// Everything that was odd, in document order.
    pub warnings: Vec<ParseWarning>,
    /// What was skipped on purpose.
    pub skipped_subtrees: SkippedCounts,
}

/// Parses an XMP file (see the module docs for the rules).
pub fn parse(bytes: &[u8]) -> Result<XmpDocument, ParseError> {
    if bytes.len() > MAX_XMP_BYTES {
        return Err(ParseError::TooLarge {
            size: bytes.len(),
            max: MAX_XMP_BYTES,
        });
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes)?;
    // Before roxmltree: it recurses per element and has quadratic paths.
    prescan(text)?;
    // DTDs stay refused (XMP never has one); the pre-scan relies on it.
    let options = ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_XMP_NODES,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(text, options)?;
    let mut pass1 = Pass1::default();
    let properties = pass1.rdf_properties(&doc)?;

    let mut interp = Interp {
        warnings: pass1.warnings,
        skipped: pass1.skipped,
    };
    let (global, header) = interp.top_level(properties);
    let kind = document_kind(&global, &header);
    let header = (!header.is_empty()).then(|| PresetHeader::from_fields(header));
    let develop = reader::finish(global, FileKindHint::Unknown, &mut interp.warnings);
    Ok(XmpDocument {
        kind,
        header,
        develop,
        warnings: interp.warnings,
        skipped_subtrees: interp.skipped,
    })
}

fn document_kind(global: &Fields, header: &Fields) -> XmpKind {
    const PRESET_TYPE: &str = "PresetType";
    match header.get(Level::Header, PRESET_TYPE) {
        Some(Value::Str(t)) if t == "Normal" => return XmpKind::Preset,
        Some(Value::Str(t)) if t == "Look" => return XmpKind::Profile,
        Some(_) => return XmpKind::NotDevelop,
        None if header.opaque.iter().any(|o| o.name == PRESET_TYPE) => return XmpKind::NotDevelop,
        None => {}
    }
    let has_pv = global.get(Level::Global, "ProcessVersion").is_some()
        || global.opaque.iter().any(|o| o.name == "ProcessVersion");
    if has_pv {
        XmpKind::Sidecar
    } else {
        XmpKind::NotDevelop
    }
}

// ---- pre-scan: depth and structural limits, before roxmltree -------------

/// What the pre-scan saw (for tests).
#[derive(Debug, Default, PartialEq, Eq)]
struct Scan {
    /// Deepest element nesting, counted from the document root.
    max_depth: usize,
    /// Namespace declarations.
    namespaces: usize,
}

fn is_xml_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

/// The index of the first `pat` at or after `from`.
fn find(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    b.get(from..)?
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|p| from + p)
}

/// One linear pass over the text, without recursion: element depth,
/// attributes per start tag, namespace declarations and their length.
///
/// It only has to agree with roxmltree on well-formed input: roxmltree
/// stops at the first malformed token, so it never gets deeper than what
/// was counted up to there. Comments, CDATA sections and processing
/// instructions are skipped; `>` inside a quoted attribute value does not
/// end a tag. Any other `<!` (a DOCTYPE) stops the scan: roxmltree refuses
/// it at that point (`allow_dtd: false` in the prolog, an unknown token in
/// content). An unterminated construct also stops it, for roxmltree to
/// report.
fn prescan(text: &str) -> Result<Scan, ParseError> {
    let b = text.as_bytes();
    let max_depth = MAX_XMP_DEPTH + DEPTH_ENVELOPE;
    let mut scan = Scan::default();
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(off) = b[i..].iter().position(|&c| c == b'<') {
        let lt = i + off;
        let rest = &b[lt..];
        let skip_to =
            |open: usize, close: &[u8]| find(b, lt + open, close).map(|e| e + close.len());
        let next = if rest.starts_with(b"<!--") {
            skip_to(4, b"-->")
        } else if rest.starts_with(b"<![CDATA[") {
            skip_to(9, b"]]>")
        } else if rest.starts_with(b"<?") {
            skip_to(2, b"?>")
        } else if rest.starts_with(b"<!") {
            None
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            find(b, lt, b">").map(|e| e + 1)
        } else {
            match start_tag(b, lt, &mut scan.namespaces)? {
                Some((after, self_closing)) => {
                    if !self_closing {
                        depth += 1;
                        scan.max_depth = scan.max_depth.max(depth);
                        if depth > max_depth {
                            return Err(ParseError::TooDeep { max: MAX_XMP_DEPTH });
                        }
                    }
                    Some(after)
                }
                None => None,
            }
        };
        match next {
            Some(n) => i = n,
            None => break,
        }
    }
    Ok(scan)
}

/// Scans the start tag whose `<` is at `lt`: the index after its `>` and
/// whether it closes itself, or `None` when the text ends first. Counts its
/// attributes and namespace declarations against the limits.
fn start_tag(
    b: &[u8],
    lt: usize,
    namespaces: &mut usize,
) -> Result<Option<(usize, bool)>, ParseError> {
    let len = b.len();
    let mut i = lt + 1;
    while i < len && !is_xml_space(b[i]) && !matches!(b[i], b'>' | b'/') {
        i += 1;
    }
    let mut attributes = 0usize;
    loop {
        while i < len && is_xml_space(b[i]) {
            i += 1;
        }
        match b.get(i) {
            None => return Ok(None),
            Some(b'>') => return Ok(Some((i + 1, false))),
            Some(b'/') if b.get(i + 1) == Some(&b'>') => return Ok(Some((i + 2, true))),
            _ => {}
        }
        let name_start = i;
        while i < len && !is_xml_space(b[i]) && !matches!(b[i], b'=' | b'>' | b'/' | b'"' | b'\'') {
            i += 1;
        }
        let name = &b[name_start..i];
        while i < len && is_xml_space(b[i]) {
            i += 1;
        }
        if b.get(i) != Some(&b'=') {
            // Malformed (roxmltree rejects this tag); just make progress.
            if i == name_start {
                i += 1;
            }
            continue;
        }
        i += 1;
        attributes += 1;
        if attributes > MAX_XMP_ATTRIBUTES {
            return Err(ParseError::Limit {
                what: "attributes on one element",
                max: MAX_XMP_ATTRIBUTES,
            });
        }
        while i < len && is_xml_space(b[i]) {
            i += 1;
        }
        let Some(&quote @ (b'"' | b'\'')) = b.get(i) else {
            continue;
        };
        let Some(close) = b[i + 1..].iter().position(|&c| c == quote) else {
            return Ok(None);
        };
        let value_len = close;
        i += close + 2;
        if name == b"xmlns" || name.starts_with(b"xmlns:") {
            *namespaces += 1;
            if *namespaces > MAX_XMP_NAMESPACES {
                return Err(ParseError::Limit {
                    what: "namespace declarations",
                    max: MAX_XMP_NAMESPACES,
                });
            }
            if value_len > MAX_XMP_NAMESPACE_URI {
                return Err(ParseError::Limit {
                    what: "bytes in one namespace URI",
                    max: MAX_XMP_NAMESPACE_URI,
                });
            }
        }
    }
}

// ---- pass 1: RDF → XMP data model -----------------------------------------

fn is_rdf(n: &Node, local: &str) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(RDF_NS) && n.tag_name().name() == local
}

/// Attributes other than `rdf:*`/`xml:*` count as fields, and `rdf:value`
/// (a qualified value in attribute form).
fn is_field_attribute(ns: Option<&str>, name: &str) -> bool {
    match ns {
        Some(RDF_NS) => name == "value",
        Some(XML_NS) => false,
        _ => true,
    }
}

fn has_field_attributes(el: Node) -> bool {
    el.attributes()
        .any(|a| is_field_attribute(a.namespace(), a.name()))
}

fn container_kind(n: Node) -> Option<XmpArrayKind> {
    if n.tag_name().namespace() != Some(RDF_NS) {
        return None;
    }
    match n.tag_name().name() {
        "Seq" => Some(XmpArrayKind::Seq),
        "Bag" => Some(XmpArrayKind::Bag),
        "Alt" => Some(XmpArrayKind::Alt),
        _ => None,
    }
}

/// The only element child of `el`, if it has exactly one.
fn only_element<'a, 'input>(el: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    let mut elements = el.children().filter(Node::is_element);
    match (elements.next(), elements.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// One step of a pass-1 path (built only when a warning needs it, so a
/// deep file with long names costs nothing per element).
enum Seg<'a> {
    Name(&'a str),
    Index(usize),
}

/// Text dropped below the current top-level property.
struct Dropped {
    path: String,
    key: String,
    first: String,
    more: usize,
}

#[derive(Default)]
struct Pass1<'a> {
    /// Every namespace URI once: fields share it instead of copying it.
    uris: HashMap<&'a str, Arc<str>>,
    path: Vec<Seg<'a>>,
    dropped: Option<Dropped>,
    warnings: Vec<ParseWarning>,
    skipped: SkippedCounts,
}

impl<'a> Pass1<'a> {
    fn ns(&mut self, uri: Option<&'a str>) -> Arc<str> {
        let uri = uri.unwrap_or("");
        self.uris
            .entry(uri)
            .or_insert_with(|| Arc::from(uri))
            .clone()
    }

    /// The `crs:` properties of every `rdf:Description` under every
    /// outermost `rdf:RDF`, in document order. Other top-level properties
    /// are counted, never built.
    fn rdf_properties<'input>(
        &mut self,
        doc: &'a Document<'input>,
    ) -> Result<Vec<XmpField>, ParseError> {
        let roots: Vec<Node<'a, 'input>> = doc
            .descendants()
            .filter(|n| is_rdf(n, "RDF") && !n.ancestors().skip(1).any(|a| is_rdf(&a, "RDF")))
            .collect();
        if roots.is_empty() {
            return Err(ParseError::NoRdf);
        }
        let mut out = Vec::new();
        for d in roots
            .iter()
            .flat_map(|r| r.children().filter(|n| is_rdf(n, "Description")))
        {
            self.note_text(d);
            self.flush_dropped();
            for a in d.attributes() {
                if !is_field_attribute(a.namespace(), a.name()) {
                    continue;
                }
                if self.top_level_is_crs(a.namespace()) {
                    out.push(XmpField {
                        ns: self.ns(a.namespace()),
                        name: a.name().to_owned(),
                        node: XmpNode::text(a.value()),
                    });
                }
            }
            for c in d.children().filter(Node::is_element) {
                if !self.top_level_is_crs(c.tag_name().namespace()) {
                    continue;
                }
                let name = c.tag_name().name();
                self.path.push(Seg::Name(name));
                let node = self.property_node(c, 1);
                self.path.pop();
                self.flush_dropped();
                out.push(XmpField {
                    ns: self.ns(c.tag_name().namespace()),
                    name: name.to_owned(),
                    node: node?,
                });
            }
        }
        Ok(out)
    }

    /// True for a `crs:` property; counts `crss:` and other namespaces.
    fn top_level_is_crs(&mut self, ns: Option<&str>) -> bool {
        match ns {
            Some(CRS_NS) => true,
            Some(CRSS_NS) => {
                self.skipped.crss += 1;
                false
            }
            _ => {
                self.skipped.other_namespaces += 1;
                false
            }
        }
    }

    fn path_string(&self) -> String {
        let mut out = String::new();
        for seg in &self.path {
            match seg {
                Seg::Name(n) => out = child(&out, n),
                Seg::Index(i) => out.push_str(&format!("[{i}]")),
            }
        }
        out
    }

    /// Records non-whitespace text directly in `el` that the value built
    /// from `el` cannot hold (text next to elements or attributes).
    fn note_text(&mut self, el: Node) {
        let Some(text) = el
            .children()
            .filter(Node::is_text)
            .filter_map(|n| n.text())
            .find(|t| !t.trim().is_empty())
        else {
            return;
        };
        match &mut self.dropped {
            Some(d) => d.more += 1,
            None => {
                let path = self.path_string();
                let key = match self.path.iter().rev().find_map(|s| match s {
                    Seg::Name(n) => Some(*n),
                    Seg::Index(_) => None,
                }) {
                    Some(n) => n.to_owned(),
                    None => "rdf:Description".to_owned(),
                };
                self.dropped = Some(Dropped {
                    path: if path.is_empty() { key.clone() } else { path },
                    key,
                    first: XmpNode::text(text.trim()).summary(),
                    more: 0,
                });
            }
        }
    }

    /// One [`WarningKind::MalformedRdf`] for the text dropped under the
    /// property just read (the first place, and how many more).
    fn flush_dropped(&mut self) {
        if let Some(d) = self.dropped.take() {
            let raw = if d.more == 0 {
                d.first
            } else {
                format!("{} (and {} more text run(s))", d.first, d.more)
            };
            reader::warn(
                &mut self.warnings,
                &d.path,
                &d.key,
                Some(raw),
                WarningKind::MalformedRdf,
            );
        }
    }

    /// The fields of a node that is a structure: its field attributes, then
    /// its child elements.
    fn fields_of<'input>(
        &mut self,
        node: Node<'a, 'input>,
        depth: usize,
    ) -> Result<Vec<XmpField>, ParseError> {
        self.note_text(node);
        let mut out = Vec::new();
        for a in node.attributes() {
            if is_field_attribute(a.namespace(), a.name()) {
                out.push(XmpField {
                    ns: self.ns(a.namespace()),
                    name: a.name().to_owned(),
                    node: XmpNode::text(a.value()),
                });
            }
        }
        for c in node.children().filter(Node::is_element) {
            let name = c.tag_name().name();
            self.path.push(Seg::Name(name));
            let value = self.property_node(c, depth + 1);
            self.path.pop();
            out.push(XmpField {
                ns: self.ns(c.tag_name().namespace()),
                name: name.to_owned(),
                node: value?,
            });
        }
        Ok(out)
    }

    /// The value of a property element or an `rdf:li`.
    ///
    /// A container or a nested `rdf:Description` is taken as such only when
    /// it is the element's only child element and the element has no field
    /// attributes (and, for a container, holds nothing but `rdf:li`);
    /// anything else is read as a plain structure of everything the element
    /// holds, so nothing is dropped silently.
    fn property_node<'input>(
        &mut self,
        el: Node<'a, 'input>,
        depth: usize,
    ) -> Result<XmpNode, ParseError> {
        if depth > MAX_XMP_DEPTH {
            return Err(ParseError::TooDeep { max: MAX_XMP_DEPTH });
        }
        let lang = el.attribute((XML_NS, "lang")).map(str::to_owned);
        let single = only_element(el).filter(|_| !has_field_attributes(el));
        let container = single.and_then(|c| {
            let kind = container_kind(c)?;
            c.children()
                .filter(Node::is_element)
                .all(|n| is_rdf(&n, "li"))
                .then_some((kind, c))
        });
        let has_elements = el.children().any(|n| n.is_element());
        let value = if let Some((kind, c)) = container {
            self.note_text(el);
            self.note_text(c);
            let mut items = Vec::new();
            for (i, li) in c.children().filter(Node::is_element).enumerate() {
                self.path.push(Seg::Index(i));
                let item = self.property_node(li, depth + 2);
                self.path.pop();
                items.push(item?);
            }
            XmpValue::Array { kind, items }
        } else if let Some(desc) = single.filter(|c| is_rdf(c, "Description")) {
            self.note_text(el);
            XmpValue::Struct(self.fields_of(desc, depth + 1)?)
        } else if has_elements
            || has_field_attributes(el)
            || el.attribute((RDF_NS, "parseType")) == Some("Resource")
        {
            XmpValue::Struct(self.fields_of(el, depth)?)
        } else if let Some(uri) = el.attribute((RDF_NS, "resource")) {
            self.note_text(el);
            XmpValue::Text(uri.to_owned())
        } else {
            XmpValue::Text(
                el.children()
                    .filter(Node::is_text)
                    .filter_map(|n| n.text())
                    .collect(),
            )
        };
        Ok(with_qualifiers(value, lang))
    }
}

/// A structure with an `rdf:value` field is a simple value whose other
/// fields are qualifiers.
fn with_qualifiers(value: XmpValue, lang: Option<String>) -> XmpNode {
    let XmpValue::Struct(mut fields) = value else {
        return XmpNode {
            value,
            lang,
            qualifiers: Vec::new(),
        };
    };
    match fields
        .iter()
        .position(|f| &*f.ns == RDF_NS && f.name == "value")
    {
        Some(i) => {
            let inner = fields.remove(i).node;
            XmpNode {
                value: inner.value,
                lang: lang.or(inner.lang),
                qualifiers: fields,
            }
        }
        None => XmpNode {
            value: XmpValue::Struct(fields),
            lang,
            qualifiers: Vec::new(),
        },
    }
}

// ---- pass 2: XMP data model → develop model -------------------------------

struct Interp {
    warnings: Vec<ParseWarning>,
    skipped: SkippedCounts,
}

/// How an array fits the registry's `rdf:Seq` shape.
enum Shape {
    /// An `rdf:Seq` whose items all fit.
    Fits,
    /// Valid XMP the model does not interpret (another container, items with
    /// a language or qualifiers): kept whole.
    KeepWhole,
    /// Not the expected shape.
    Wrong,
}

fn seq_shape(node: &XmpNode, item_fits: impl Fn(&XmpNode) -> bool) -> Shape {
    match &node.value {
        XmpValue::Array {
            kind: XmpArrayKind::Seq,
            items,
        } => {
            if items.iter().any(|i| !i.is_plain()) {
                Shape::KeepWhole
            } else if items.iter().all(item_fits) {
                Shape::Fits
            } else {
                Shape::Wrong
            }
        }
        XmpValue::Array { .. } => Shape::KeepWhole,
        _ => Shape::Wrong,
    }
}

fn is_text(n: &XmpNode) -> bool {
    matches!(n.value, XmpValue::Text(_))
}

fn is_struct(n: &XmpNode) -> bool {
    matches!(n.value, XmpValue::Struct(_))
}

/// The text items of a fitting `rdf:Seq`.
fn seq_texts(node: &XmpNode) -> Vec<&str> {
    match &node.value {
        XmpValue::Array { items, .. } => items.iter().filter_map(XmpNode::as_text).collect(),
        _ => Vec::new(),
    }
}

/// The structure items of a fitting `rdf:Seq`, consumed.
fn seq_structs(node: XmpNode) -> Vec<Vec<XmpField>> {
    match node.value {
        XmpValue::Array { items, .. } => items
            .into_iter()
            .filter_map(|i| match i.value {
                XmpValue::Struct(fields) => Some(fields),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn scalar_in(s: &str) -> Option<ScalarIn> {
    match s.trim() {
        "True" | "true" => Some(ScalarIn::Bool(true)),
        "False" | "false" => Some(ScalarIn::Bool(false)),
        t => t
            .parse::<f64>()
            .ok()
            .and_then(|x| Finite::new(x).ok())
            .map(ScalarIn::Num),
    }
}

fn describe(kind: ValueKind) -> String {
    match kind {
        ValueKind::Int => "an integer".into(),
        ValueKind::Real => "a number".into(),
        ValueKind::Bool(_) => "True/False (or 0/1)".into(),
        ValueKind::IntFlag => "0/1 (or True/False)".into(),
        ValueKind::Enum(set) => format!("one of {set:?}"),
        ValueKind::EnumInt(set) => format!("one of {set:?}"),
        ValueKind::Str | ValueKind::VersionStr | ValueKind::Hex32 => "text".into(),
        ValueKind::VersionU32 => "a packed version number".into(),
        ValueKind::Curve(CurveKind::Global) => "an rdf:Seq of \"x, y\" points".into(),
        ValueKind::Curve(CurveKind::Local) => "an rdf:Seq of \"x,y\" points".into(),
        ValueKind::StrSeq => "an rdf:Seq of text items".into(),
        ValueKind::LangAlt => "an rdf:Alt of language items".into(),
        ValueKind::Struct(k) => format!("a {} structure", k.name()),
        ValueKind::StructSeq(k) => format!("an rdf:Seq of {} structures", k.name()),
        ValueKind::CorrectionSeq => "an rdf:Seq of corrections".into(),
        ValueKind::ComponentSeq => "an rdf:Seq of mask components".into(),
        ValueKind::Settings | ValueKind::Any | ValueKind::PerFormat { .. } => "any value".into(),
    }
}

fn opaque(name: String, node: XmpNode) -> OpaqueEntry {
    OpaqueEntry {
        ns: None,
        name,
        value: Opaque::Xmp(node),
    }
}

impl Interp {
    fn warn(&mut self, path: &str, key: &str, node: &XmpNode, kind: WarningKind) {
        reader::warn(&mut self.warnings, path, key, Some(node.summary()), kind);
    }

    /// Splits the top-level `crs:` properties (pass 1 keeps no others)
    /// into global and header fields: a name resolves at the global level
    /// first, then as a header key.
    fn top_level(&mut self, properties: Vec<XmpField>) -> (Fields, Fields) {
        let mut global = Fields::default();
        let mut header = Fields::default();
        // One set for both: a name always lands on the same side.
        let mut seen = HashSet::new();
        for f in properties {
            debug_assert_eq!(&*f.ns, CRS_NS, "pass 1 keeps only crs: at the top");
            let is_header = matches!(registry::resolve(Level::Global, &f.name), Resolved::Unknown)
                && registry::lookup(Level::Header, &f.name).is_some();
            if is_header {
                self.field(&mut header, &mut seen, Level::Header, f, "");
            } else {
                self.field(&mut global, &mut seen, Level::Global, f, "");
            }
        }
        (global, header)
    }

    fn fields(&mut self, list: Vec<XmpField>, level: Level, path: &str) -> Fields {
        let mut out = Fields::default();
        let mut seen = HashSet::new();
        for f in list {
            self.field(&mut out, &mut seen, level, f, path);
        }
        out
    }

    /// Types one field into `out`. `seen` holds the `crs:` names already
    /// read at this level: a repeat is kept verbatim with a
    /// [`WarningKind::DuplicateKey`], also when the first occurrence went
    /// to `opaque` (wrong type, unknown), so the first value always wins.
    fn field(
        &mut self,
        out: &mut Fields,
        seen: &mut HashSet<String>,
        level: Level,
        f: XmpField,
        path: &str,
    ) {
        let XmpField { ns, name, node } = f;
        if &*ns == CRSS_NS {
            self.skipped.crss += 1;
            return;
        }
        let p = child(path, &name);
        if &*ns != CRS_NS {
            // A foreign property inside a develop structure: kept, with its
            // namespace, and reported.
            self.warn(&p, &name, &node, WarningKind::UnknownKey);
            out.opaque.push(OpaqueEntry {
                ns: Some(ns),
                name,
                value: Opaque::Xmp(node),
            });
            return;
        }
        if !seen.insert(name.clone()) {
            self.warn(&p, &name, &node, WarningKind::DuplicateKey);
            out.opaque.push(opaque(name, node));
            return;
        }
        match registry::resolve(level, &name) {
            Resolved::Key(id) => match self.value(id.spec(), node, &p, &name) {
                Ok(v) => {
                    out.values.insert(id, v);
                }
                Err(node) => out.opaque.push(opaque(name, node)),
            },
            Resolved::Pattern(_) => out.opaque.push(opaque(name, node)),
            Resolved::Unknown => {
                self.warn(&p, &name, &node, WarningKind::UnknownKey);
                out.opaque.push(opaque(name, node));
            }
        }
    }

    /// Types `node` as `spec`'s XMP kind; `Err(node)` (after a
    /// [`WarningKind::WrongType`]) when it does not have that shape.
    fn value(
        &mut self,
        spec: &KeySpec,
        node: XmpNode,
        path: &str,
        key: &str,
    ) -> Result<Value, XmpNode> {
        let kind = spec.kind.xmp_form();
        let keep = |node| Ok(Value::Opaque(Opaque::Xmp(node)));
        match kind {
            ValueKind::Settings => {
                match spec.level {
                    Level::Struct(StructKind::Look) => self.skipped.look_parameters += 1,
                    Level::Struct(StructKind::Preset) => self.skipped.preset_parameters += 1,
                    _ => {}
                }
                return keep(node);
            }
            ValueKind::Any | ValueKind::PerFormat { .. } => return keep(node),
            _ => {}
        }
        if !node.is_plain() {
            return self.keep_whole(node);
        }
        let typed = match kind {
            ValueKind::Int
            | ValueKind::Real
            | ValueKind::IntFlag
            | ValueKind::EnumInt(_)
            | ValueKind::VersionU32
            | ValueKind::Bool(_) => node.as_text().and_then(scalar_in).and_then(|input| {
                reader::scalar(
                    spec,
                    input,
                    path,
                    key,
                    || node.summary(),
                    &mut self.warnings,
                )
            }),
            ValueKind::Enum(_) | ValueKind::Str | ValueKind::VersionStr | ValueKind::Hex32 => {
                node.as_text().and_then(|s| {
                    reader::text(kind, s, path, key, || node.summary(), &mut self.warnings)
                })
            }
            ValueKind::Curve(_) => match seq_shape(&node, is_text) {
                Shape::Fits => seq_texts(&node)
                    .into_iter()
                    .map(parse_point)
                    .collect::<Option<Vec<_>>>()
                    .map(Value::Curve),
                Shape::KeepWhole => return self.keep_whole(node),
                Shape::Wrong => None,
            },
            ValueKind::StrSeq => match seq_shape(&node, is_text) {
                Shape::Fits => Some(Value::StrList(
                    seq_texts(&node).into_iter().map(str::to_owned).collect(),
                )),
                Shape::KeepWhole => return self.keep_whole(node),
                Shape::Wrong => None,
            },
            ValueKind::LangAlt => match &node.value {
                XmpValue::Array {
                    kind: XmpArrayKind::Alt,
                    items,
                } if items.iter().all(|i| is_text(i) && i.qualifiers.is_empty()) => {
                    match items.as_slice() {
                        [only] if only.lang.as_deref() == Some("x-default") => match &only.value {
                            XmpValue::Text(s) => Some(Value::Alt(s.clone())),
                            _ => None,
                        },
                        // Further languages: valid, kept whole.
                        _ => return self.keep_whole(node),
                    }
                }
                XmpValue::Array { .. } => return self.keep_whole(node),
                _ => None,
            },
            ValueKind::Struct(k) => {
                if let XmpValue::Struct(fields) = node.value {
                    let fields = self.fields(fields, Level::Struct(k), path);
                    return Ok(Value::Struct(Struct { kind: k, fields }));
                }
                None
            }
            ValueKind::StructSeq(_) | ValueKind::CorrectionSeq | ValueKind::ComponentSeq => {
                match seq_shape(&node, is_struct) {
                    Shape::Fits => return Ok(self.struct_seq(kind, node, path)),
                    Shape::KeepWhole => return self.keep_whole(node),
                    Shape::Wrong => None,
                }
            }
            ValueKind::Settings | ValueKind::Any | ValueKind::PerFormat { .. } => {
                unreachable!("kept whole above")
            }
        };
        match typed {
            Some(v) => Ok(v),
            None => {
                self.warn(
                    path,
                    key,
                    &node,
                    WarningKind::WrongType {
                        expected: describe(kind),
                    },
                );
                Err(node)
            }
        }
    }

    /// A registry key kept whole because of its form (see
    /// [`SkippedCounts::kept_whole`]): counted, since its typing is lost.
    fn keep_whole(&mut self, node: XmpNode) -> Result<Value, XmpNode> {
        self.skipped.kept_whole += 1;
        Ok(Value::Opaque(Opaque::Xmp(node)))
    }

    /// A fitting `rdf:Seq` of structures as `kind` (a structure list,
    /// corrections or mask components).
    fn struct_seq(&mut self, kind: ValueKind, node: XmpNode, path: &str) -> Value {
        let items = seq_structs(node).into_iter().enumerate();
        match kind {
            ValueKind::StructSeq(k) => Value::StructList(
                items
                    .map(|(i, fields)| Struct {
                        kind: k,
                        fields: self.fields(fields, Level::Struct(k), &format!("{path}[{i}]")),
                    })
                    .collect(),
            ),
            ValueKind::CorrectionSeq => Value::Corrections(
                items
                    .map(|(i, fields)| {
                        let p = format!("{path}[{i}]");
                        let fields = self.fields(fields, Level::Correction, &p);
                        Correction::from_fields(fields, &p, &mut self.warnings)
                    })
                    .collect(),
            ),
            ValueKind::ComponentSeq => Value::Tools(
                items
                    .map(|(i, fields)| {
                        self.fields(fields, Level::MaskTool, &format!("{path}[{i}]"))
                    })
                    .collect(),
            ),
            _ => unreachable!("only called for sequences of structures"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Combine, FileKind, MaskTool, Semantic};

    const HEAD: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#;
    const TAIL: &str = "</rdf:RDF></x:xmpmeta>";

    /// One description with the `crs:` namespace bound to `crs`.
    fn doc(attrs: &str, body: &str) -> String {
        format!(
            r#"{HEAD}<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" {attrs}>{body}</rdf:Description>{TAIL}"#
        )
    }

    fn read(text: &str) -> XmpDocument {
        parse(text.as_bytes()).unwrap_or_else(|e| panic!("{e}"))
    }

    fn kinds(d: &XmpDocument) -> Vec<&'static str> {
        d.warnings.iter().map(|w| w.kind.name()).collect()
    }

    #[test]
    fn hard_errors_are_only_for_unusable_input() {
        assert!(matches!(parse(b"<a>"), Err(ParseError::Xml(_))));
        assert!(matches!(
            parse(b"\xff\xfe<a/>"),
            Err(ParseError::Encoding(_))
        ));
        assert!(matches!(parse(b"<a/>"), Err(ParseError::NoRdf)));
        let big = vec![b' '; MAX_XMP_BYTES + 1];
        assert!(matches!(parse(&big), Err(ParseError::TooLarge { .. })));
        let dtd = format!("<!DOCTYPE x [<!ENTITY e \"y\">]>{}", doc("", ""));
        assert!(matches!(parse(dtd.as_bytes()), Err(ParseError::Xml(_))));
    }

    #[test]
    fn nesting_just_past_the_limit_is_refused() {
        let depth = MAX_XMP_DEPTH + 5;
        let body = format!(
            "{}{}",
            "<crs:A rdf:parseType=\"Resource\">".repeat(depth),
            "</crs:A>".repeat(depth)
        );
        assert!(matches!(
            parse(doc("", &body).as_bytes()),
            Err(ParseError::TooDeep { .. })
        ));
    }

    /// Parses on a thread with a 2 MiB stack (the default for tokio
    /// workers, `spawn_blocking` and `std::thread`). roxmltree recurses once
    /// per element, so without the pre-scan these inputs abort the process
    /// with a stack overflow, in debug and release builds alike.
    fn parse_on_small_stack(text: String) -> Result<XmpDocument, ParseError> {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || parse(text.as_bytes()))
            .expect("spawn")
            .join()
            .expect("no panic")
    }

    #[test]
    fn very_deep_nesting_is_refused_before_the_xml_parser_recurses() {
        let n = 100_000;
        let plain = format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
        assert!(plain.len() < MAX_XMP_BYTES);
        assert!(matches!(
            parse_on_small_stack(plain),
            Err(ParseError::TooDeep { .. })
        ));
        let n = 50_000;
        let in_rdf = doc(
            "",
            &format!("{}{}", "<crs:A>".repeat(n), "</crs:A>".repeat(n)),
        );
        assert!(in_rdf.len() < MAX_XMP_BYTES);
        assert!(matches!(
            parse_on_small_stack(in_rdf),
            Err(ParseError::TooDeep { .. })
        ));
    }

    #[test]
    fn the_prescan_ignores_markup_in_comments_cdata_pis_and_attributes() {
        let deep = "<b>".repeat(500);
        let text = format!(
            r#"<?xml version="1.0"?><a x='/>' y=">"><!-- {deep} --><![CDATA[{deep}]]><?pi {deep} ?><e/><f  z = "1" /></a>"#
        );
        let scan = prescan(&text).unwrap();
        assert_eq!(scan.max_depth, 1);
        assert_eq!(
            prescan("<a><b></b><c/><d><e></e></d></a>")
                .unwrap()
                .max_depth,
            3
        );
        let ns = prescan(r#"<a xmlns="urn:a" xmlns:b="urn:b" b:c="1"/>"#).unwrap();
        assert_eq!(ns.namespaces, 2);
        // A real file stays well inside the limits.
        let d = doc(r#"crs:ProcessVersion="15.4""#, "");
        assert!(prescan(&d).unwrap().max_depth <= DEPTH_ENVELOPE);
    }

    #[test]
    fn structural_limits_refuse_quadratic_inputs() {
        let attrs: String = (0..100_000).map(|i| format!(" a{i}=\"\"")).collect();
        let many_attrs = format!("<a{attrs}/>");
        assert!(matches!(
            parse(many_attrs.as_bytes()),
            Err(ParseError::Limit {
                max: MAX_XMP_ATTRIBUTES,
                ..
            })
        ));
        let decls: String = (0..20_000)
            .map(|i| format!(" xmlns:p{i}=\"urn:{i}\""))
            .collect();
        // Split over elements, so the per-element attribute cap is not what
        // fires.
        let per_element: Vec<String> = decls
            .split(" xmlns")
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .chunks(100)
            .map(|c| format!("<e xmlns{}/>", c.join(" xmlns")))
            .collect();
        let many_ns = format!("<a>{}</a>", per_element.concat());
        assert!(matches!(
            parse(many_ns.as_bytes()),
            Err(ParseError::Limit {
                max: MAX_XMP_NAMESPACES,
                ..
            })
        ));
        let long_uri = format!(
            r#"<a xmlns:p="urn:{}"/>"#,
            "x".repeat(MAX_XMP_NAMESPACE_URI)
        );
        assert!(matches!(
            parse(long_uri.as_bytes()),
            Err(ParseError::Limit {
                max: MAX_XMP_NAMESPACE_URI,
                ..
            })
        ));
        let e = ParseError::Limit {
            what: "attributes on one element",
            max: MAX_XMP_ATTRIBUTES,
        };
        assert_eq!(
            e.to_string(),
            "the XMP file has more than 1024 attributes on one element"
        );
    }

    #[test]
    fn namespace_uris_are_shared_not_copied_per_field() {
        let uri = format!("urn:{}", "u".repeat(1000));
        let body = format!(
            r#"<crs:LensBlur rdf:parseType="Resource" xmlns:p="{uri}">{}</crs:LensBlur>"#,
            "<p:a/>".repeat(10_000)
        );
        let d = read(&doc("", &body));
        let Some(Value::Struct(blur)) = d.develop.get_by_name("LensBlur") else {
            panic!("LensBlur");
        };
        let foreign = &blur.fields.opaque;
        assert_eq!(foreign.len(), 10_000);
        let first = foreign[0].ns.as_ref().expect("namespace kept");
        assert_eq!(&**first, uri);
        assert!(foreign
            .iter()
            .all(|o| o.ns.as_ref().is_some_and(|ns| Arc::ptr_eq(ns, first))));
    }

    #[test]
    fn top_level_foreign_properties_are_counted_not_built() {
        let body = format!(
            r#"<p:a xmlns:p="urn:p">{}</p:a>{}"#,
            "<p:b/>".repeat(100),
            r#"<dc:x xmlns:dc="http://purl.org/dc/elements/1.1/">t</dc:x>"#
        );
        let d = read(&doc(
            r#"crs:ProcessVersion="15.4" xmlns:q="urn:q" q:c="1""#,
            &body,
        ));
        assert_eq!(d.skipped_subtrees.other_namespaces, 3);
        assert!(d.develop.opaque.is_empty());
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    }

    #[test]
    fn a_bom_is_stripped() {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(doc(r#"crs:ProcessVersion="15.4""#, "").as_bytes());
        let d = parse(&bytes).unwrap();
        assert_eq!(d.kind, XmpKind::Sidecar);
    }

    #[test]
    fn the_namespace_is_matched_by_uri_not_prefix() {
        let text = format!(
            r#"{HEAD}<rdf:Description xmlns:cr="http://ns.adobe.com/camera-raw-settings/1.0/" xmlns:crs="urn:not-camera-raw" cr:Exposure2012="+0.50" crs:Contrast2012="10"/>{TAIL}"#
        );
        let d = read(&text);
        assert_eq!(
            d.develop.get_by_name("Exposure2012"),
            Some(&Value::Real(Finite::new_const(0.5)))
        );
        assert_eq!(d.develop.get_by_name("Contrast2012"), None);
        assert_eq!(d.skipped_subtrees.other_namespaces, 1);
        assert!(d.warnings.is_empty());
    }

    #[test]
    fn numbers_bools_and_signs_are_typed_by_the_registry() {
        let d = read(&doc(
            r#"crs:Exposure2012="+0.50" crs:Contrast2012="+15" crs:Shadows2012="-7" crs:Texture="0" crs:ConvertToGrayscale="True" crs:AutoLateralCA="1" crs:LensProfileEnable="1""#,
            "",
        ));
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        let s = &d.develop;
        assert_eq!(
            s.get_by_name("Exposure2012"),
            Some(&Value::Real(Finite::new_const(0.5)))
        );
        assert_eq!(s.get_by_name("Contrast2012"), Some(&Value::Int(15)));
        assert_eq!(s.get_by_name("Shadows2012"), Some(&Value::Int(-7)));
        assert_eq!(s.get_by_name("Texture"), Some(&Value::Int(0)));
        assert_eq!(
            s.get_by_name("ConvertToGrayscale"),
            Some(&Value::Bool(true))
        );
        assert_eq!(s.get_by_name("AutoLateralCA"), Some(&Value::Int(1)));
    }

    #[test]
    fn wrong_values_are_warned_about_and_kept() {
        let d = read(&doc(
            r#"crs:Exposure2012="abc" crs:Contrast2012="12.5" crs:WhiteBalance="Sunset" crs:Mystery="1""#,
            "",
        ));
        assert_eq!(
            kinds(&d),
            [
                "WrongType",
                "NonIntegerForIntKey",
                "ValueNotInSet",
                "UnknownKey"
            ]
        );
        assert_eq!(d.develop.get_by_name("Contrast2012"), Some(&Value::Int(12)));
        assert_eq!(
            d.develop.get_by_name("WhiteBalance"),
            Some(&Value::Str("Sunset".into()))
        );
        let names: Vec<&str> = d.develop.opaque.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["Exposure2012", "Mystery"]);
        assert_eq!(d.warnings[0].raw.as_deref(), Some("abc"));
    }

    #[test]
    fn duplicate_keys_keep_the_first_value() {
        let text = format!(
            r#"{HEAD}<rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Contrast2012="10"/><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Contrast2012="20"/>{TAIL}"#
        );
        let d = read(&text);
        assert_eq!(kinds(&d), ["DuplicateKey"]);
        assert_eq!(d.develop.get_by_name("Contrast2012"), Some(&Value::Int(10)));
        assert_eq!(d.develop.opaque.len(), 1);
    }

    #[test]
    fn a_wrongly_typed_first_value_still_wins_over_a_duplicate() {
        let text = format!(
            r#"{HEAD}<rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="abc" crs:Mystery="1"/><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+1" crs:Mystery="2"/>{TAIL}"#
        );
        let d = read(&text);
        assert_eq!(
            kinds(&d),
            ["WrongType", "UnknownKey", "DuplicateKey", "DuplicateKey"]
        );
        assert_eq!(d.develop.get_by_name("Exposure2012"), None);
        let names: Vec<&str> = d.develop.opaque.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            names,
            ["Exposure2012", "Mystery", "Exposure2012", "Mystery"]
        );
    }

    #[test]
    fn rdf_value_as_an_attribute_is_the_value() {
        let d = read(&doc("", r#"<crs:Texture rdf:value="5"/>"#));
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        assert_eq!(d.develop.get_by_name("Texture"), Some(&Value::Int(5)));
        assert_eq!(d.skipped_subtrees.kept_whole, 0);

        let d = read(&doc(
            "",
            r#"<crs:Texture rdf:value="5" crs:Note="q" xml:lang="en"/>"#,
        ));
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        let Some(Value::Opaque(Opaque::Xmp(tex))) = d.develop.get_by_name("Texture") else {
            panic!("Texture kept whole");
        };
        assert_eq!(tex.value, XmpValue::Text("5".into()));
        assert_eq!(tex.lang.as_deref(), Some("en"));
        assert_eq!(tex.qualifiers.len(), 1);
        assert_eq!(tex.qualifiers[0].name, "Note");
        assert_eq!(d.skipped_subtrees.kept_whole, 1);
    }

    #[test]
    fn a_container_mixed_with_other_content_is_reported_and_kept() {
        let d = read(&doc(
            "",
            r#"<crs:ToneCurvePV2012 crs:Extra="1"><rdf:Seq><rdf:li>0, 0</rdf:li><crs:NotLi>x</crs:NotLi></rdf:Seq><crs:Hidden>h</crs:Hidden><rdf:Seq><rdf:li>1, 1</rdf:li></rdf:Seq></crs:ToneCurvePV2012>"#,
        ));
        assert_eq!(kinds(&d), ["WrongType"]);
        assert_eq!(d.warnings[0].path, "ToneCurvePV2012");
        assert_eq!(d.develop.get_by_name("ToneCurvePV2012"), None);
        let [kept] = d.develop.opaque.as_slice() else {
            panic!("{:?}", d.develop.opaque);
        };
        let Opaque::Xmp(node) = &kept.value else {
            panic!("xmp");
        };
        let XmpValue::Struct(fields) = &node.value else {
            panic!("kept as a structure of everything");
        };
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Extra", "Seq", "Hidden", "Seq"]);

        // A lone container with something other than rdf:li inside.
        let d = read(&doc(
            "",
            r#"<crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><crs:NotLi>x</crs:NotLi></rdf:Seq></crs:ToneCurvePV2012>"#,
        ));
        assert_eq!(kinds(&d), ["WrongType"]);
    }

    #[test]
    fn text_next_to_elements_is_reported_once_per_property() {
        let d = read(&doc(
            "",
            r#"<crs:LensBlur rdf:parseType="Resource">junk<crs:Active>true</crs:Active>more</crs:LensBlur>
               <crs:ToneCurvePV2012>stray<rdf:Seq><rdf:li>0, 0</rdf:li></rdf:Seq></crs:ToneCurvePV2012>"#,
        ));
        assert_eq!(kinds(&d), ["MalformedRdf", "MalformedRdf"]);
        assert_eq!(d.warnings[0].path, "LensBlur");
        assert_eq!(d.warnings[0].raw.as_deref(), Some("junk"));
        assert_eq!(d.warnings[1].path, "ToneCurvePV2012");
        let Some(Value::Struct(blur)) = d.develop.get_by_name("LensBlur") else {
            panic!("LensBlur still typed");
        };
        assert_eq!(blur.get("Active"), Some(&Value::Bool(true)));
        assert_eq!(
            d.develop.get_by_name("ToneCurvePV2012"),
            Some(&Value::Curve(vec![(
                Finite::new_const(0.0),
                Finite::new_const(0.0)
            )]))
        );

        // Deeper: the path names the first place, the rest are counted.
        let d = read(&doc(
            "",
            r#"<crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li rdf:parseType="Resource">x<crs:What>Correction</crs:What>
                 <crs:CorrectionMasks><rdf:Seq><rdf:li crs:What="Mask/Image" crs:MaskSubType="1" crs:MaskBlendMode="0" crs:MaskValue="1" crs:MaskInverted="false">y</rdf:li></rdf:Seq></crs:CorrectionMasks>
               </rdf:li></rdf:Seq></crs:MaskGroupBasedCorrections>"#,
        ));
        assert_eq!(kinds(&d), ["MalformedRdf"]);
        assert_eq!(d.warnings[0].path, "MaskGroupBasedCorrections[0]");
        assert_eq!(
            d.warnings[0].raw.as_deref(),
            Some("x (and 1 more text run(s))")
        );
    }

    #[test]
    fn every_outermost_rdf_root_is_read() {
        let crs = r#"xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/""#;
        let rdf = r#"xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#""#;
        let text = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF {rdf}/><rdf:RDF {rdf}><rdf:Description {crs} crs:ProcessVersion="15.4"/></rdf:RDF></x:xmpmeta>"#
        );
        let d = read(&text);
        assert_eq!(d.kind, XmpKind::Sidecar);
    }

    #[test]
    fn kinds_follow_preset_type_and_process_version() {
        let kind = |attrs: &str| read(&doc(attrs, "")).kind;
        assert_eq!(kind(r#"crs:ProcessVersion="15.4""#), XmpKind::Sidecar);
        assert_eq!(
            kind(r#"crs:PresetType="Normal" crs:ProcessVersion="15.4""#),
            XmpKind::Preset
        );
        assert_eq!(kind(r#"crs:PresetType="Look""#), XmpKind::Profile);
        assert_eq!(kind(r#"crs:PresetType="Other""#), XmpKind::NotDevelop);
        assert_eq!(kind(r#"crs:Exposure2012="0""#), XmpKind::NotDevelop);
        assert_eq!(kind(""), XmpKind::NotDevelop);
    }

    #[test]
    fn crss_is_skipped_entirely() {
        let text = format!(
            r#"{HEAD}<rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmlns:crss="http://ns.adobe.com/camera-raw-saved-settings/1.0/" crs:ProcessVersion="15.4"><crss:SavedSettings><rdf:Bag><rdf:li rdf:parseType="Resource"><crss:Name>Snapshot</crss:Name></rdf:li></rdf:Bag></crss:SavedSettings></rdf:Description>{TAIL}"#
        );
        let d = read(&text);
        assert!(d.warnings.is_empty());
        assert_eq!(d.skipped_subtrees.crss, 1);
        assert!(d.develop.opaque.is_empty());
    }

    #[test]
    fn element_form_and_attribute_form_read_the_same() {
        let attrs = read(&doc(
            r#"crs:Exposure2012="+0.50" crs:WhiteBalance="Custom" crs:Temperature="5500""#,
            "",
        ));
        let elements = read(&doc(
            "",
            "<crs:Exposure2012>+0.50</crs:Exposure2012><crs:WhiteBalance>Custom</crs:WhiteBalance><crs:Temperature>5500</crs:Temperature>",
        ));
        assert_eq!(attrs.develop, elements.develop);
        assert_eq!(attrs.develop.file_kind, Some(FileKind::Raw));
    }

    #[test]
    fn struct_forms_read_the_same() {
        let desc = read(&doc(
            "",
            r#"<crs:LensBlur><rdf:Description crs:Active="true" crs:BlurAmount="50"/></crs:LensBlur>"#,
        ));
        let attrs = read(&doc(
            "",
            r#"<crs:LensBlur crs:Active="true" crs:BlurAmount="50"/>"#,
        ));
        let resource = read(&doc(
            "",
            r#"<crs:LensBlur rdf:parseType="Resource"><crs:Active>true</crs:Active><crs:BlurAmount>50</crs:BlurAmount></crs:LensBlur>"#,
        ));
        assert!(desc.warnings.is_empty(), "{:?}", desc.warnings);
        assert_eq!(desc.develop, attrs.develop);
        assert_eq!(desc.develop, resource.develop);
        let Some(Value::Struct(blur)) = desc.develop.get_by_name("LensBlur") else {
            panic!("LensBlur");
        };
        assert_eq!(blur.get("Active"), Some(&Value::Bool(true)));
    }

    #[test]
    fn alt_bag_and_qualifiers_are_kept_whole() {
        let d = read(&doc(
            r#"crs:PresetType="Normal""#,
            r#"<crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Name</rdf:li><rdf:li xml:lang="de-DE">Name DE</rdf:li></rdf:Alt></crs:Name>
               <crs:Group><rdf:Alt><rdf:li xml:lang="x-default"/></rdf:Alt></crs:Group>
               <crs:ToneCurvePV2012><rdf:Bag><rdf:li>0, 0</rdf:li></rdf:Bag></crs:ToneCurvePV2012>
               <crs:Texture rdf:parseType="Resource"><rdf:value>5</rdf:value><crs:Note>q</crs:Note></crs:Texture>"#,
        ));
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        // Name (a second language), ToneCurvePV2012 (a Bag), Texture
        // (qualifiers): typing lost, counted.
        assert_eq!(d.skipped_subtrees.kept_whole, 3);
        let h = d.header.unwrap();
        assert_eq!(h.name, None);
        assert_eq!(h.group.as_deref(), Some(""));
        let Some(Value::Opaque(Opaque::Xmp(name))) = h.rest.get(Level::Header, "Name") else {
            panic!("Name kept whole");
        };
        assert_eq!(name.alt_default(), Some("Name"));
        let Some(Value::Opaque(Opaque::Xmp(tex))) = d.develop.get_by_name("Texture") else {
            panic!("Texture kept whole");
        };
        assert_eq!(tex.value, XmpValue::Text("5".into()));
        assert_eq!(tex.qualifiers.len(), 1);
        assert!(matches!(
            d.develop.get_by_name("ToneCurvePV2012"),
            Some(Value::Opaque(Opaque::Xmp(_)))
        ));
    }

    #[test]
    fn corrections_land_in_the_shared_model_with_paths() {
        let d = read(&doc(
            "",
            r#"<crs:MaskGroupBasedCorrections><rdf:Seq>
                 <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionAmount="1" crs:LocalExposure2012="0.0625">
                   <crs:CorrectionMasks><rdf:Seq>
                     <rdf:li crs:What="Mask/Image" crs:MaskSubType="2" crs:MaskBlendMode="0" crs:MaskValue="1" crs:MaskInverted="false"/>
                     <rdf:li crs:What="Mask/Image" crs:MaskSubType="1" crs:MaskBlendMode="1" crs:MaskValue="0" crs:MaskInverted="true" crs:Oddity="1"/>
                   </rdf:Seq></crs:CorrectionMasks>
                 </rdf:Description></rdf:li>
               </rdf:Seq></crs:MaskGroupBasedCorrections>"#,
        ));
        assert_eq!(kinds(&d), ["UnknownKey"]);
        assert_eq!(
            d.warnings[0].path,
            "MaskGroupBasedCorrections[0].CorrectionMasks[1].Oddity"
        );
        let c = &d.develop.corrections[0];
        assert_eq!(c.amount, Some(Finite::new_const(1.0)));
        assert_eq!(
            c.local_value("LocalExposure2012"),
            Some(&Value::Real(Finite::new_const(0.0625)))
        );
        assert_eq!(c.masks[0].tool, MaskTool::Semantic(Semantic::Sky));
        assert_eq!(c.masks[1].tool, MaskTool::Semantic(Semantic::Subject));
        assert_eq!(c.masks[1].combine, Combine::Intersect);
    }

    #[test]
    fn look_and_preset_parameters_stay_opaque() {
        let d = read(&doc(
            r#"crs:ProcessVersion="15.4""#,
            r#"<crs:Look><rdf:Description crs:Name="Synthetic Profile" crs:Amount="1.000000" crs:UUID="00000000000000000000000000000001">
                 <crs:Parameters><rdf:Description crs:ConvertToGrayscale="False" crs:Unheard="1"/></crs:Parameters>
               </rdf:Description></crs:Look>
               <crs:Preset><rdf:Description crs:Name="Synthetic Text" crs:Amount="1">
                 <crs:Parameters><rdf:Description crs:Exposure2012="+1.00" crs:Unheard="1"/></crs:Parameters>
               </rdf:Description></crs:Preset>"#,
        ));
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        assert_eq!(d.skipped_subtrees.look_parameters, 1);
        assert_eq!(d.skipped_subtrees.preset_parameters, 1);
        let look = d.develop.look.as_ref().unwrap();
        assert_eq!(look.amount, Some(Finite::new_const(1.0)));
        assert!(matches!(
            look.rest.get("Parameters"),
            Some(Value::Opaque(Opaque::Xmp(_)))
        ));
        let Some(Value::Struct(preset)) = d.develop.get_by_name("Preset") else {
            panic!("Preset");
        };
        assert!(matches!(
            preset.get("Parameters"),
            Some(Value::Opaque(Opaque::Xmp(_)))
        ));
    }
}
