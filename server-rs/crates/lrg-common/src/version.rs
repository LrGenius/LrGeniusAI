//! Backend version info and the plugin<->backend compatibility check.
//! Port of `services/version.py` + `version_info.py`. CI overrides the
//! placeholder values at build time via the LRG_BACKEND_* env vars.

use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};

pub fn backend_version() -> &'static str {
    option_env!("LRG_BACKEND_VERSION").unwrap_or("0.0.0-dev")
}

pub fn backend_release_tag() -> &'static str {
    option_env!("LRG_BACKEND_RELEASE_TAG").unwrap_or("v0.0.0-dev")
}

pub fn backend_build() -> i64 {
    match option_env!("LRG_BACKEND_BUILD") {
        Some(v) => v.parse().unwrap_or(0),
        None => 0,
    }
}

#[derive(Serialize)]
pub struct BackendVersionInfo {
    pub backend_version: &'static str,
    pub backend_release_tag: &'static str,
    pub backend_build: i64,
}

pub fn get_backend_version_info() -> BackendVersionInfo {
    BackendVersionInfo {
        backend_version: backend_version(),
        backend_release_tag: backend_release_tag(),
        backend_build: backend_build(),
    }
}

/// Exact-match version check with the dev fallback (backend "dev" +
/// plugin placeholder 9.9.9 accepted).
pub fn check_plugin_backend_version(
    plugin_version: Option<&str>,
    plugin_build: Option<i64>,
    plugin_release_tag: Option<&str>,
) -> Value {
    check_plugin_backend_version_against(
        &get_backend_version_info(),
        plugin_version,
        plugin_build,
        plugin_release_tag,
    )
}

fn check_plugin_backend_version_against(
    backend: &BackendVersionInfo,
    plugin_version: Option<&str>,
    plugin_build: Option<i64>,
    plugin_release_tag: Option<&str>,
) -> Value {
    let normalized_backend = normalize_version(Some(backend.backend_version));
    let normalized_plugin = normalize_version(plugin_version);

    let base = json!({
        "backend_version": backend.backend_version,
        "backend_release_tag": backend.backend_release_tag,
        "backend_build": backend.backend_build,
        "plugin_version": plugin_version,
        "plugin_release_tag": plugin_release_tag,
        "plugin_build": plugin_build,
    });

    let mut result = base;
    let obj = result.as_object_mut().unwrap();

    let Some(normalized_plugin) = normalized_plugin else {
        obj.insert("compatible".into(), json!(false));
        obj.insert(
            "reason".into(),
            json!("plugin_version is missing or invalid"),
        );
        return result;
    };

    if is_dev_backend(backend.backend_version, backend.backend_release_tag)
        && normalized_plugin == "9.9.9"
    {
        obj.insert("compatible".into(), json!(true));
        obj.insert(
            "reason".into(),
            json!("dev fallback: placeholder plugin version accepted for dev backend"),
        );
        return result;
    }

    let compatible = Some(normalized_plugin) == normalized_backend;
    obj.insert("compatible".into(), json!(compatible));
    obj.insert(
        "reason".into(),
        json!(if compatible {
            "exact version match"
        } else {
            "plugin and backend version differ"
        }),
    );
    result
}

fn normalize_version(raw: Option<&str>) -> Option<String> {
    let candidate = raw?.trim();
    let candidate = candidate.strip_prefix('v').unwrap_or(candidate);
    let re = Regex::new(r"^(\d+)\.(\d+)\.(\d+)").unwrap();
    let caps = re.captures(candidate)?;
    Some(format!("{}.{}.{}", &caps[1], &caps[2], &caps[3]))
}

fn is_dev_backend(version: &str, release_tag: &str) -> bool {
    version.to_lowercase().contains("dev") || release_tag.to_lowercase().contains("dev")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_plugin_version_is_incompatible() {
        let r = check_plugin_backend_version(None, None, None);
        assert_eq!(r["compatible"], json!(false));
        assert_eq!(r["reason"], json!("plugin_version is missing or invalid"));
    }

    #[test]
    fn garbage_plugin_version_is_incompatible() {
        let r = check_plugin_backend_version(Some("not-a-version"), None, None);
        assert_eq!(r["compatible"], json!(false));
    }

    // These tests exercise the pure comparison logic against an injected
    // `BackendVersionInfo` rather than the real `check_plugin_backend_version`,
    // whose backend values are baked in at compile time via `LRG_BACKEND_*`
    // env vars. Release builds set those to the real tagged version (not
    // "dev"), so asserting against the compiled-in defaults would pass only
    // on dev builds and fail on every tagged release.
    fn dev_backend() -> BackendVersionInfo {
        BackendVersionInfo {
            backend_version: "0.0.0-dev",
            backend_release_tag: "v0.0.0-dev",
            backend_build: 0,
        }
    }

    #[test]
    fn dev_fallback_accepts_placeholder_plugin() {
        let r = check_plugin_backend_version_against(
            &dev_backend(),
            Some("9.9.9"),
            Some(0),
            Some("v9.9.9"),
        );
        assert_eq!(r["compatible"], json!(true));
        assert!(r["reason"].as_str().unwrap().contains("dev fallback"));
    }

    #[test]
    fn exact_match_against_backend_version() {
        let backend = BackendVersionInfo {
            backend_version: "1.4.2",
            backend_release_tag: "v1.4.2",
            backend_build: 20260726,
        };
        let r = check_plugin_backend_version_against(&backend, Some("v1.4.2"), None, None);
        assert_eq!(r["compatible"], json!(true));
        assert_eq!(r["reason"], json!("exact version match"));
    }

    #[test]
    fn mismatched_versions_are_incompatible() {
        let backend = BackendVersionInfo {
            backend_version: "1.4.2",
            backend_release_tag: "v1.4.2",
            backend_build: 20260726,
        };
        let r = check_plugin_backend_version_against(&backend, Some("1.2.3"), None, None);
        assert_eq!(r["compatible"], json!(false));
        assert_eq!(r["reason"], json!("plugin and backend version differ"));
    }

    #[test]
    fn normalize_strips_v_prefix_and_suffix() {
        assert_eq!(
            normalize_version(Some("v1.2.3-beta+45")),
            Some("1.2.3".to_string())
        );
        assert_eq!(normalize_version(Some("  1.2.3  ")), Some("1.2.3".into()));
        assert_eq!(normalize_version(Some("1.2")), None);
        assert_eq!(normalize_version(None), None);
    }
}
