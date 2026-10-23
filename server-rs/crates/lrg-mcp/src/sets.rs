//! Result sets: photo lists that stay on the server and travel between tool
//! calls as a short id.
//!
//! A search over a large catalog can match thousands of photos. Handing those
//! IDs to the agent costs context and invites copying mistakes, so a tool that
//! finds photos stores them here and returns `s7` plus a preview; the next tool
//! takes `set: "s7"`. Sets are per backend process and not persisted — after a
//! restart the agent searches again — and the oldest are dropped past a cap.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

/// Enough for a long agent session; each set is a list of short strings.
const MAX_SETS: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub struct ResultSet {
    pub id: String,
    /// What produced it, in words — shown back to the agent so it can keep
    /// several sets apart.
    pub label: String,
    pub photo_ids: Vec<String>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    sets: HashMap<String, ResultSet>,
    order: VecDeque<String>,
}

#[derive(Default)]
pub struct ResultSets {
    inner: Mutex<Inner>,
}

impl ResultSets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `photo_ids` (duplicates removed, order kept) as a new set.
    pub fn create(&self, label: impl Into<String>, photo_ids: Vec<String>) -> ResultSet {
        let mut inner = self.inner.lock().unwrap();
        inner.next += 1;
        let set = ResultSet {
            id: format!("s{}", inner.next),
            label: label.into(),
            photo_ids: dedup(photo_ids),
        };
        inner.sets.insert(set.id.clone(), set.clone());
        inner.order.push_back(set.id.clone());
        while inner.order.len() > MAX_SETS {
            if let Some(oldest) = inner.order.pop_front() {
                inner.sets.remove(&oldest);
            }
        }
        set
    }

    pub fn get(&self, id: &str) -> Option<ResultSet> {
        self.inner.lock().unwrap().sets.get(id.trim()).cloned()
    }
}

/// How two sets combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOperation {
    Union,
    Intersection,
    Difference,
}

/// Combines two ID lists, keeping `a`'s order (then `b`'s, for a union).
pub fn combine(a: &[String], b: &[String], operation: SetOperation) -> Vec<String> {
    let b_set: HashSet<&str> = b.iter().map(String::as_str).collect();
    match operation {
        SetOperation::Union => {
            let mut out = a.to_vec();
            out.extend(b.iter().cloned());
            dedup(out)
        }
        SetOperation::Intersection => dedup(
            a.iter()
                .filter(|id| b_set.contains(id.as_str()))
                .cloned()
                .collect(),
        ),
        SetOperation::Difference => dedup(
            a.iter()
                .filter(|id| !b_set.contains(id.as_str()))
                .cloned()
                .collect(),
        ),
    }
}

fn dedup(ids: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.into_iter()
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn sets_get_short_ids_and_drop_duplicates() {
        let sets = ResultSets::new();
        let a = sets.create("search \"dog\"", ids(&["p1", "p2", "p1", ""]));
        assert_eq!(a.id, "s1");
        assert_eq!(a.photo_ids, ids(&["p1", "p2"]));
        assert_eq!(sets.get(" s1 ").unwrap().label, "search \"dog\"");
        assert_eq!(sets.create("x", vec![]).id, "s2");
        assert!(sets.get("s9").is_none());
    }

    #[test]
    fn the_oldest_sets_fall_off_past_the_cap() {
        let sets = ResultSets::new();
        for _ in 0..MAX_SETS + 5 {
            sets.create("x", ids(&["p"]));
        }
        assert!(sets.get("s1").is_none());
        assert!(sets.get("s6").is_some());
    }

    #[test]
    fn combining_keeps_order() {
        let a = ids(&["p1", "p2", "p3"]);
        let b = ids(&["p3", "p4", "p1"]);
        assert_eq!(
            combine(&a, &b, SetOperation::Union),
            ids(&["p1", "p2", "p3", "p4"])
        );
        assert_eq!(
            combine(&a, &b, SetOperation::Intersection),
            ids(&["p1", "p3"])
        );
        assert_eq!(combine(&a, &b, SetOperation::Difference), ids(&["p2"]));
    }
}
