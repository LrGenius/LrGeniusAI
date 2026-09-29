//! Shared gate for golden tests that need model files on disk.
//!
//! These tests used to `return` early when their ONNX assets were missing,
//! which the harness reports as a pass. That made `cargo test` green on a
//! machine — CI included — where the numerical checks never ran at all, and a
//! wrong embedding is exactly the failure that produces no error of its own:
//! search quietly gets worse, face clusters quietly get muddier. A skip that
//! reports success is the same class of bug the backend's `warnings` plumbing
//! exists to prevent, one level up.
//!
//! Skipping is still the right default for a working copy without multi-GB
//! downloads, so the gate keeps it — but makes it *enforceable*. Set
//! `LRG_REQUIRE_GOLDENS` to the families whose assets are expected to be
//! present and a missing one becomes a failure instead of a silent pass:
//!
//! ```text
//! LRG_REQUIRE_GOLDENS=face         # just the 89 MB face pair (what PR CI provisions)
//! LRG_REQUIRE_GOLDENS=face,siglip  # a comma-separated subset
//! LRG_REQUIRE_GOLDENS=all          # every family except the local-only ones (nightly)
//! ```
//!
//! Some families can never be provisioned in CI because their data is
//! private or not redistributable (the maintainer's training dump, later the
//! Lightroom preset bundle and a sidecar corpus). They are listed in
//! `LOCAL_ONLY`: `all` does not include them, so the nightly job stays green
//! without them, and only naming one explicitly
//! (`LRG_REQUIRE_GOLDENS=training-dump`) makes its absence a failure. Their
//! tests read a path from an environment variable and skip, through this
//! gate, when it is unset.
//!
//! Per-family rather than one global on/off because the families differ in
//! cost by more than an order of magnitude — face is 89 MB, BioCLIP 834 MB,
//! SigLIP2 2.2 GB. A single switch would force PR CI to choose between
//! downloading 3 GB per run and enforcing nothing.

/// Decides whether a golden test body should run.
///
/// Returns `true` when `present` (the caller's own "are my files here?"
/// check) holds. Otherwise either panics — if `family` is named in
/// `LRG_REQUIRE_GOLDENS` — or logs `detail` and returns `false` for the
/// caller to return on.
///
/// `detail` should name the paths that were looked for, so a CI failure says
/// where the download was expected to land rather than just that it is absent.
#[must_use]
pub fn assets_ready(family: &str, present: bool, detail: &str) -> bool {
    if present {
        return true;
    }
    if required(family) {
        panic!(
            "LRG_REQUIRE_GOLDENS demands the `{family}` golden assets, but they are missing: \
             {detail}. Either provision them (the plugin's model download, or the CI step that \
             fetches this family's release assets) or drop `{family}` from LRG_REQUIRE_GOLDENS."
        );
    }
    eprintln!(
        "skipping `{family}` golden test: {detail}. \
         (Set LRG_REQUIRE_GOLDENS={family} to make this a failure instead.)"
    );
    false
}

/// Families whose data only exists on a maintainer's machine (private or not
/// redistributable). `LRG_REQUIRE_GOLDENS=all` leaves them out; only naming
/// one explicitly makes its absence a failure.
const LOCAL_ONLY: &[&str] = &["training-dump"];

/// Whether `LRG_REQUIRE_GOLDENS` names this family, either explicitly or via
/// `all` (which excludes [`LOCAL_ONLY`] families). Entries are trimmed and
/// matched case-insensitively so that `LRG_REQUIRE_GOLDENS="face, SigLIP"`
/// behaves the way it reads.
fn required(family: &str) -> bool {
    required_by(std::env::var("LRG_REQUIRE_GOLDENS").ok().as_deref(), family)
}

/// [`required`] for a given `LRG_REQUIRE_GOLDENS` value (`None` = unset).
fn required_by(setting: Option<&str>, family: &str) -> bool {
    let Some(raw) = setting else {
        return false;
    };
    let local_only = LOCAL_ONLY.iter().any(|f| f.eq_ignore_ascii_case(family));
    raw.split(',').map(str::trim).any(|entry| {
        entry.eq_ignore_ascii_case(family) || (!local_only && entry.eq_ignore_ascii_case("all"))
    })
}

#[cfg(test)]
mod tests {
    use super::required_by;

    #[test]
    fn all_covers_every_family_except_the_local_only_ones() {
        assert!(required_by(Some("all"), "face"));
        assert!(required_by(Some("all"), "siglip"));
        assert!(!required_by(Some("all"), "training-dump"));
    }

    #[test]
    fn local_only_families_are_required_only_when_named() {
        assert!(required_by(Some("training-dump"), "training-dump"));
        assert!(required_by(Some("face, Training-Dump"), "training-dump"));
        assert!(!required_by(Some("face"), "training-dump"));
    }

    #[test]
    fn unset_requires_nothing() {
        assert!(!required_by(None, "face"));
        assert!(!required_by(None, "training-dump"));
        assert!(!required_by(Some(""), "face"));
    }
}
