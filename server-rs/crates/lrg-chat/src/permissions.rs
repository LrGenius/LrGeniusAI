//! What the chat may do without asking.
//!
//! The user picks how much to approve by hand. A mode sets the default per
//! kind of tool, a rule per tool overrides it, and a bulk limit catches the
//! change a model aims at far more photos than intended. The plugin's own
//! read-only switch sits underneath all of it: the agent bridge refuses a
//! change before it reaches Lightroom, whatever was allowed here.
//!
//! The kind of a tool comes from its MCP annotations, so a new tool is
//! classified where it is written, not here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rmcp::model::ToolAnnotations;
use serde::{Deserialize, Serialize};

/// Where the settings live, next to the index — the same place as
/// `person_names.json`.
const FILE_NAME: &str = "chat_permissions.json";

/// Above this many photos, a change asks even when it is allowed.
const DEFAULT_BULK_LIMIT: u32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    /// Only looks: searching, reading metadata, viewing a photo.
    Read,
    /// Changes something that is easy to take back and touches no photo's
    /// own data: a collection, the selection, the index.
    Change,
    /// Changes the photos themselves: metadata, ratings, develop settings.
    Destructive,
}

impl ToolClass {
    /// MCP's defaults apply when a hint is missing: a tool is assumed to
    /// change things, destructively.
    #[must_use]
    pub fn of(annotations: Option<&ToolAnnotations>) -> Self {
        match annotations {
            Some(a) if a.read_only_hint == Some(true) => ToolClass::Read,
            Some(a) if a.destructive_hint == Some(false) => ToolClass::Change,
            _ => ToolClass::Destructive,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Every call asks.
    AskAll,
    /// Looking runs, any change asks.
    #[default]
    AutoRead,
    /// Looking and easy-to-undo changes run; changes to photos ask.
    AutoSafe,
    /// Everything runs, up to the bulk limit.
    AutoAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRule {
    Allow,
    Ask,
    /// Not offered to the model at all.
    Block,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Permissions {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolRule>,
    /// A change to photos that would touch more than this many asks first,
    /// even when allowed. 0 turns the check off.
    #[serde(default = "default_bulk_limit")]
    pub bulk_limit: u32,
}

fn default_bulk_limit() -> u32 {
    DEFAULT_BULK_LIMIT
}

impl Default for Permissions {
    fn default() -> Self {
        Permissions {
            mode: Mode::default(),
            tools: BTreeMap::new(),
            bulk_limit: DEFAULT_BULK_LIMIT,
        }
    }
}

/// Why a call waits for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskReason {
    /// The mode asks for this kind of tool.
    Mode,
    /// The user set this tool to ask.
    Rule,
    /// Allowed, but aimed at more photos than the bulk limit.
    Bulk { photos: usize, limit: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Run,
    Ask(AskReason),
    Blocked,
}

impl Permissions {
    /// Whether the model is told about the tool at all.
    #[must_use]
    pub fn offers(&self, tool: &str) -> bool {
        self.tools.get(tool) != Some(&ToolRule::Block)
    }

    /// Whether a call runs now, waits for the user, or may not run.
    ///
    /// `allowed_in_chat` is the user's "allow for this chat" from an earlier
    /// approval. `photos` is how many photos the call touches, when that is
    /// known before running it.
    #[must_use]
    pub fn verdict(
        &self,
        tool: &str,
        class: ToolClass,
        allowed_in_chat: bool,
        photos: Option<usize>,
    ) -> Verdict {
        let allowed = match self.tools.get(tool) {
            Some(ToolRule::Block) => return Verdict::Blocked,
            _ if allowed_in_chat => true,
            Some(ToolRule::Allow) => true,
            Some(ToolRule::Ask) => return Verdict::Ask(AskReason::Rule),
            None => match (self.mode, class) {
                (Mode::AskAll, _) => false,
                (Mode::AutoRead, ToolClass::Read) => true,
                (Mode::AutoRead, _) => false,
                (Mode::AutoSafe, ToolClass::Destructive) => false,
                (Mode::AutoSafe, _) | (Mode::AutoAll, _) => true,
            },
        };
        if !allowed {
            return Verdict::Ask(AskReason::Mode);
        }
        // Only changes to the photos themselves: showing 500 search results
        // in a collection is not what the limit is for.
        if class == ToolClass::Destructive && self.bulk_limit > 0 {
            if let Some(photos) = photos.filter(|&n| n > self.bulk_limit as usize) {
                return Verdict::Ask(AskReason::Bulk {
                    photos,
                    limit: self.bulk_limit,
                });
            }
        }
        Verdict::Run
    }
}

/// The settings on disk, cached per index directory.
#[derive(Default)]
pub struct PermissionStore {
    cache: Mutex<Option<(Option<PathBuf>, Permissions)>>,
}

impl PermissionStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The settings for the index at `dir` — `None` before the plugin has
    /// told the backend where its index lives, in which case they are kept
    /// in memory only.
    pub fn get(&self, dir: Option<&Path>) -> Permissions {
        let mut cache = self.cache.lock().unwrap();
        if let Some((cached_dir, permissions)) = cache.as_ref() {
            if cached_dir.as_deref() == dir {
                return permissions.clone();
            }
        }
        let permissions = dir.map(load).unwrap_or_default();
        *cache = Some((dir.map(Path::to_path_buf), permissions.clone()));
        permissions
    }

    /// Store new settings. Returns whether they were written to disk.
    ///
    /// # Errors
    /// Reports a file that could not be written; the settings still apply
    /// for as long as the backend runs.
    pub fn set(&self, dir: Option<&Path>, permissions: Permissions) -> Result<bool, String> {
        *self.cache.lock().unwrap() = Some((dir.map(Path::to_path_buf), permissions.clone()));
        let Some(dir) = dir else {
            return Ok(false);
        };
        let text = serde_json::to_string_pretty(&permissions).map_err(|e| e.to_string())?;
        // Written beside and renamed over, so a crash mid-write cannot leave
        // half a file that would read back as the defaults.
        let path = dir.join(FILE_NAME);
        let partial = dir.join(format!("{FILE_NAME}.partial"));
        std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::write(&partial, text))
            .and_then(|()| std::fs::rename(&partial, &path))
            .map_err(|e| format!("could not save the chat permissions to {path:?}: {e}"))?;
        Ok(true)
    }
}

/// Settings that cannot be read fall back to the defaults, which ask before
/// any change — the safe direction to fail in.
fn load(dir: &Path) -> Permissions {
    let path = dir.join(FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("Ignoring unreadable chat permissions in {path:?}: {e}");
            Permissions::default()
        }),
        Err(_) => Permissions::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_mode(mode: Mode) -> Permissions {
        Permissions {
            mode,
            ..Permissions::default()
        }
    }

    #[test]
    fn classes_follow_the_annotations_and_default_to_destructive() {
        let mut a = ToolAnnotations::default();
        a.read_only_hint = Some(true);
        assert_eq!(ToolClass::of(Some(&a)), ToolClass::Read);
        a.read_only_hint = Some(false);
        a.destructive_hint = Some(false);
        assert_eq!(ToolClass::of(Some(&a)), ToolClass::Change);
        a.destructive_hint = None;
        assert_eq!(ToolClass::of(Some(&a)), ToolClass::Destructive);
        assert_eq!(ToolClass::of(None), ToolClass::Destructive);
    }

    #[test]
    fn each_mode_lets_through_what_it_says() {
        let run = |mode, class| with_mode(mode).verdict("t", class, false, None) == Verdict::Run;
        use ToolClass::*;
        assert!(!run(Mode::AskAll, Read));
        assert!(run(Mode::AutoRead, Read));
        assert!(!run(Mode::AutoRead, Change));
        assert!(run(Mode::AutoSafe, Change));
        assert!(!run(Mode::AutoSafe, Destructive));
        assert!(run(Mode::AutoAll, Destructive));
    }

    #[test]
    fn a_rule_beats_the_mode() {
        let mut p = with_mode(Mode::AutoAll);
        p.tools.insert("update_photos".into(), ToolRule::Ask);
        p.tools.insert("eval".into(), ToolRule::Block);
        assert_eq!(
            p.verdict("update_photos", ToolClass::Destructive, false, None),
            Verdict::Ask(AskReason::Rule)
        );
        assert_eq!(
            p.verdict("eval", ToolClass::Read, true, None),
            Verdict::Blocked
        );
        assert!(!p.offers("eval"));

        let mut p = with_mode(Mode::AskAll);
        p.tools.insert("search_photos".into(), ToolRule::Allow);
        assert_eq!(
            p.verdict("search_photos", ToolClass::Read, false, None),
            Verdict::Run
        );
    }

    #[test]
    fn allowing_for_this_chat_overrides_ask_but_not_block() {
        let mut p = with_mode(Mode::AskAll);
        p.tools.insert("update_photos".into(), ToolRule::Ask);
        assert_eq!(
            p.verdict("update_photos", ToolClass::Destructive, true, None),
            Verdict::Run
        );
    }

    #[test]
    fn the_bulk_limit_asks_even_when_allowed_but_only_for_photo_changes() {
        let p = with_mode(Mode::AutoAll);
        assert_eq!(
            p.verdict("update_photos", ToolClass::Destructive, true, Some(201)),
            Verdict::Ask(AskReason::Bulk {
                photos: 201,
                limit: 200
            })
        );
        assert_eq!(
            p.verdict("update_photos", ToolClass::Destructive, false, Some(200)),
            Verdict::Run
        );
        assert_eq!(
            p.verdict("show_in_lightroom", ToolClass::Change, false, Some(5000)),
            Verdict::Run
        );
        let off = Permissions {
            bulk_limit: 0,
            ..with_mode(Mode::AutoAll)
        };
        assert_eq!(
            off.verdict("update_photos", ToolClass::Destructive, false, Some(5000)),
            Verdict::Run
        );
    }

    #[test]
    fn settings_survive_a_round_trip_and_bad_files_mean_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = PermissionStore::new();
        assert_eq!(store.get(Some(dir.path())), Permissions::default());

        let mut p = with_mode(Mode::AutoSafe);
        p.tools.insert("update_photos".into(), ToolRule::Allow);
        assert!(store.set(Some(dir.path()), p.clone()).unwrap());
        assert_eq!(PermissionStore::new().get(Some(dir.path())), p);

        std::fs::write(dir.path().join(FILE_NAME), "{not json").unwrap();
        assert_eq!(
            PermissionStore::new().get(Some(dir.path())),
            Permissions::default()
        );
    }

    #[test]
    fn without_an_index_the_settings_stay_in_memory() {
        let store = PermissionStore::new();
        let p = with_mode(Mode::AskAll);
        assert!(!store.set(None, p.clone()).unwrap());
        assert_eq!(store.get(None), p);
    }
}
