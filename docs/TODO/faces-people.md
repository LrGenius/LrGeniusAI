# Implementation plan: manual people edits + stable clustering

This document describes a future feature set (not implemented yet): merge persons, remove false-positive face assignments, and ensuring **Cluster faces** does not arbitrarily undo manual edits while still allowing new indexed faces to join appropriate persons.

## 1. Goals

- **Merge persons**: Combine two (or more) `person_id`s into one; names and face lists stay consistent.
- **Remove false positives**: Move specific **faces** off a person (e.g. to `person_unassigned` or another person), not only hiding them in the UI.
- **Re-cluster safety**: A new **Cluster faces** run must **not** undo merges/excludes arbitrarily; it **should** still attach **new** faces (from indexing) to the right person when the model agrees.

## 2. Non-goals (unless explicitly added later)

- Full parity with Immich / Google Photos people UX.
- Cross-catalog identity or renaming beyond the current `person_id` scheme.

## 3. Design decision (choose before coding)

**Recommended default: metadata-driven pins** on each face row in Chroma:

- `assignment_source`: `"cluster"` | `"user"` (or a boolean `user_locked`).
- Optional later: `user_note`, timestamps.

### Clustering behavior (high level)

- Faces with `assignment_source == "user"`: **do not** change `person_id` in `run_clustering` (document any narrow exceptions).
- Faces with `assignment_source == "cluster"`: **participate** in global clustering as today.
- **New faces** from indexing: start as cluster-assigned; first cluster run assigns them (same as today unless a later “nearest person” shortcut is added).

**Merge:** Server-side mutation: reassign all faces from person A → B, merge names, retire A as needed.

**False positive:** User reassigns that face (unassigned or another person) and sets `assignment_source=user`.

An alternative is a **post-cluster override layer** (easier to bolt on, harder to reason about). This plan assumes **pins + explicit merge/reassign APIs**.

## 4. Phases

### Phase A — Data model & migration

- Add fields to face metadata (Chroma) for every face document:
  - `assignment_source` (string; default `"cluster"` for existing rows in a one-time migration).
- Semantics: missing/empty → treat as `"cluster"` for backward compatibility.
- One-time migration: scan the face collection and backfill defaults.

### Phase B — Backend APIs

- **`POST /faces/persons/merge`** (exact path TBD): body e.g. `{ "from_person_id", "into_person_id" }`
  - Move all faces with `from_person_id` → `into_person_id`.
  - Set `assignment_source=user` on affected faces (exact rule: all moved vs only conflicting—decide).
  - Merge `person_names.json` (rules: keep target name vs prompt—product decision).
  - Remove or clean name entry for `from_person_id`.
  - Prefer idempotent behavior where possible.

- **`POST /faces/faces/reassign`** (or equivalent): body e.g. `{ "face_id", "person_id" }` or unassigned sentinel
  - Update metadata + set `assignment_source=user`.
  - Requires stable **`face_id`** in APIs wherever the UI needs per-face actions.

- Ensure list/detail endpoints expose enough data for the plugin (face ids per person if not already).

### Phase C — `run_clustering` changes (`services.persons.run_clustering`)

Current behavior (simplified): load all embeddings, cluster everyone, map labels to `person_id` by overlap, **overwrite every face’s `person_id`**.

New behavior:

- Partition indices into **pinned** (`assignment_source == "user"`) vs **floating** (`cluster`).
- Run Agglomerative/DBSCAN on **floating** faces only (recommended), or run full matrix then **overwrite only floating**—floating-only is simpler and cheaper.
- Map new cluster labels to `person_id` using overlap logic **for floating faces** and existing persons that still participate.
- Pinned faces: keep `person_id` unchanged.
- New indexed faces: typically floating → receive cluster `person_id` as today.

Document edge cases in code:

- Person has only pinned faces.
- Merge left an empty `person_id`; cleanup names and references.

### Phase D — Indexing path

- On `add_face` / batch: default `assignment_source=cluster`, `person_id` empty until clustering (explicit or unchanged).
- Optional later: lightweight “nearest centroid” assignment without full cluster (defer).

### Phase E — Plugin UX (`TaskPeople.lua` and related)

- **Merge**: flow to merge person A into B → merge API → reload list.
- **Remove from person**: face-level UI (list faces per person with actions, or another surface).
- Confirmations and API error display.
- Optional: indicate manual vs auto assignments.

### Phase F — Testing & operations

- Unit tests: merge, reassign, migration defaults.
- Integration: cluster twice with pinned faces; pins stable; floats can change.
- Manual QA: merge → cluster → exclude → newly indexed face behavior.

## 5. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| Clustering only on “floating” set changes global geometry | Document; overlap matching only on floating; pins fixed |
| Bad merge / duplicate state | Transactional merge; single code path |
| Chroma metadata growth | Keep new fields minimal |
| UX complexity | Ship merge + single-face reassign first; defer bulk tools |

## 6. Open questions (resolve before implementation)

1. **False positive**: unassign only, or also “move to person X”?
2. **Merge naming**: keep target name, concatenate, or always prompt?
3. Exclude **user-pinned** face embeddings from the clustering matrix (recommended: yes, for correctness and CPU)?
4. **Undo** / audit log in v1?

## 7. Suggested implementation order

1. Migration + `assignment_source` on face metadata  
2. Reassign + merge APIs (test with HTTP/curl before UI)  
3. Update `run_clustering` to respect pins  
4. Plugin: merge + minimal face reassign  
5. Tests and polish  

## 8. References (codebase)

- `server/src/services/persons.py` — `run_clustering`, `list_persons`, names file  
- `server/src/services/chroma.py` — face collection, `update_face_metadatas`  
- `server/src/routes/faces.py` — HTTP surface  
- `plugin/.../TaskPeople.lua` — People UI  

---

*Status: planning only — not implemented.*
