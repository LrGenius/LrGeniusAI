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
//! LRG_REQUIRE_GOLDENS=all          # every family (the nightly full-assets job)
//! ```
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

/// Whether `LRG_REQUIRE_GOLDENS` names this family, either explicitly or via
/// `all`. Entries are trimmed and matched case-insensitively so that
/// `LRG_REQUIRE_GOLDENS="face, SigLIP"` behaves the way it reads.
fn required(family: &str) -> bool {
    let Ok(raw) = std::env::var("LRG_REQUIRE_GOLDENS") else {
        return false;
    };
    raw.split(',')
        .map(str::trim)
        .any(|entry| entry.eq_ignore_ascii_case("all") || entry.eq_ignore_ascii_case(family))
}
