//! XMP subtrees the model keeps verbatim ([`Opaque::Xmp`](super::value::Opaque::Xmp)).
//!
//! An [`XmpNode`] is one value in the XMP data model, independent of how the
//! file spelled it: attribute or element form, a nested `rdf:Description`,
//! `rdf:parseType="Resource"` and attributes on the property element all
//! become the same [`XmpValue::Struct`]. What the model does not interpret
//! (unknown keys, an `rdf:Bag`, an `rdf:Alt` with more than the `x-default`
//! language, qualifiers, `Look.Parameters`, `Preset.Parameters`, a property
//! element mixing forms RDF does not allow) is kept as such a node, so
//! nothing is lost except text next to elements, which the reader reports;
//! the syntax form is not kept, since the writer chooses its own.

use std::sync::Arc;

/// One XMP value with its `xml:lang` and qualifiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmpNode {
    /// The value.
    pub value: XmpValue,
    /// `xml:lang` (the language of an `rdf:Alt` item).
    pub lang: Option<String>,
    /// Qualifiers other than `xml:lang` (the siblings of an `rdf:value`).
    pub qualifiers: Vec<XmpField>,
}

/// The value of an [`XmpNode`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XmpValue {
    /// A simple value, as text (entities decoded).
    Text(String),
    /// A structure: its fields in document order.
    Struct(Vec<XmpField>),
    /// An array.
    Array {
        /// `rdf:Seq`, `rdf:Bag` or `rdf:Alt`.
        kind: XmpArrayKind,
        /// The `rdf:li` items in document order.
        items: Vec<XmpNode>,
    },
}

/// The three RDF containers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum XmpArrayKind {
    /// `rdf:Seq`: ordered.
    Seq,
    /// `rdf:Bag`: unordered.
    Bag,
    /// `rdf:Alt`: alternatives (language alternatives when the items carry
    /// `xml:lang`).
    Alt,
}

impl XmpArrayKind {
    /// `"Seq"`, `"Bag"` or `"Alt"`.
    pub fn name(self) -> &'static str {
        match self {
            XmpArrayKind::Seq => "Seq",
            XmpArrayKind::Bag => "Bag",
            XmpArrayKind::Alt => "Alt",
        }
    }
}

/// A named field of a structure (or a qualifier): namespace URI, local name
/// and value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmpField {
    /// Namespace URI (`""` for an attribute without a prefix). Shared: the
    /// reader keeps each URI once per file, so a long URI costs its length
    /// once, not once per field.
    pub ns: Arc<str>,
    /// Local name.
    pub name: String,
    /// The value.
    pub node: XmpNode,
}

impl XmpNode {
    /// A plain text node.
    pub fn text(s: impl Into<String>) -> XmpNode {
        XmpNode {
            value: XmpValue::Text(s.into()),
            lang: None,
            qualifiers: Vec::new(),
        }
    }

    /// True when the node has neither a language nor qualifiers.
    pub fn is_plain(&self) -> bool {
        self.lang.is_none() && self.qualifiers.is_empty()
    }

    /// The text of a plain text node.
    pub fn as_text(&self) -> Option<&str> {
        match &self.value {
            XmpValue::Text(s) if self.is_plain() => Some(s),
            _ => None,
        }
    }

    /// The `x-default` item of an `rdf:Alt` (also when it has further
    /// languages).
    pub fn alt_default(&self) -> Option<&str> {
        let XmpValue::Array {
            kind: XmpArrayKind::Alt,
            items,
        } = &self.value
        else {
            return None;
        };
        items.iter().find_map(|item| match &item.value {
            XmpValue::Text(s) if item.lang.as_deref() == Some("x-default") => Some(s.as_str()),
            _ => None,
        })
    }

    /// A short description for a warning: the text (cut to 80 characters),
    /// or the shape of a structure or array.
    pub fn summary(&self) -> String {
        const MAX: usize = 80;
        match &self.value {
            XmpValue::Text(s) if s.chars().count() <= MAX => s.clone(),
            XmpValue::Text(s) => format!("{}…", s.chars().take(MAX).collect::<String>()),
            XmpValue::Struct(fields) => format!("a structure with {} field(s)", fields.len()),
            XmpValue::Array { kind, items } => {
                format!("an rdf:{} with {} item(s)", kind.name(), items.len())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alt(items: &[(&str, &str)]) -> XmpNode {
        XmpNode {
            value: XmpValue::Array {
                kind: XmpArrayKind::Alt,
                items: items
                    .iter()
                    .map(|(lang, s)| XmpNode {
                        lang: Some((*lang).to_owned()),
                        ..XmpNode::text(*s)
                    })
                    .collect(),
            },
            lang: None,
            qualifiers: Vec::new(),
        }
    }

    #[test]
    fn alt_default_finds_the_default_among_languages() {
        assert_eq!(
            alt(&[("de-DE", "Himmel"), ("x-default", "Sky")]).alt_default(),
            Some("Sky")
        );
        assert_eq!(alt(&[("de-DE", "Himmel")]).alt_default(), None);
        assert_eq!(XmpNode::text("x").alt_default(), None);
    }

    #[test]
    fn as_text_refuses_qualified_nodes() {
        assert_eq!(XmpNode::text("5").as_text(), Some("5"));
        let qualified = XmpNode {
            qualifiers: vec![XmpField {
                ns: "urn:q".into(),
                name: "q".into(),
                node: XmpNode::text("1"),
            }],
            ..XmpNode::text("5")
        };
        assert_eq!(qualified.as_text(), None);
    }

    #[test]
    fn summary_is_short() {
        assert_eq!(XmpNode::text("+0.50").summary(), "+0.50");
        assert_eq!(XmpNode::text("x".repeat(200)).summary().chars().count(), 81);
        assert_eq!(
            alt(&[("x-default", "")]).summary(),
            "an rdf:Alt with 1 item(s)"
        );
    }
}
