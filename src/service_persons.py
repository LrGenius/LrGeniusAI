"""
Person grouping for face embeddings: cluster faces into persons and store display names.
Persons are identified by cluster ids (person_0, person_1, ...); names are stored in a JSON file.

Distance scale: API accepts cosine distance (1 - cosine_similarity), same as Immich.
- Immich default 0.7, typical range 0.4–0.8 (higher = merge more).
- Converted to L2 for sklearn: L2 = sqrt(2 * cosine_distance) for unit vectors.
"""
from __future__ import annotations

import json
import math
import os
import re
from typing import List, Dict, Any, Optional, Set

import numpy as np
from sklearn.cluster import AgglomerativeClustering, DBSCAN

from config import DB_PATH, logger
import service_chroma as chroma_service

PERSON_NAMES_FILENAME = "person_names.json"


def _person_names_path() -> str:
    return os.path.join(DB_PATH, PERSON_NAMES_FILENAME)


def _load_person_names() -> Dict[str, str]:
    path = _person_names_path()
    if not os.path.isfile(path):
        return {}
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception as e:
        logger.warning(f"Could not load person names from {path}: {e}")
        return {}


def _save_person_names(names: Dict[str, str]) -> None:
    path = _person_names_path()
    try:
        with open(path, "w", encoding="utf-8") as f:
            json.dump(names, f, ensure_ascii=False, indent=2)
    except Exception as e:
        logger.error(f"Could not save person names to {path}: {e}")
        raise


def set_person_name(person_id: str, name: str) -> None:
    """Set or update the display name for a person. Empty name clears it."""
    names = _load_person_names()
    if name:
        names[person_id] = name.strip()
    else:
        names.pop(person_id, None)
    _save_person_names(names)


def get_person_name(person_id: str) -> str:
    """Return the display name for a person, or empty string."""
    return _load_person_names().get(person_id, "")


_PERSON_ID_RE = re.compile(r"^person_(\d+)$")


def _max_person_index(person_faces: Dict[str, Set[str]]) -> int:
    """Return the highest person_N index in the given keys, or -1."""
    max_idx = -1
    for pid in person_faces:
        m = _PERSON_ID_RE.match(pid)
        if m:
            max_idx = max(max_idx, int(m.group(1)))
    return max_idx


def run_clustering(
    distance_threshold: float = 0.5,
    min_faces_per_person: Optional[int] = None,
    linkage: str = "complete",
) -> Dict[str, Any]:
    """
    Cluster all face embeddings into persons and update face metadata with person_id.
    Uses cosine distance (Immich-compatible scale); converted to L2 internally.

    Args:
        distance_threshold: Cosine distance (1 - cosine_similarity) below which two faces
            are merged. Same scale as Immich "Maximum recognition distance".
            Default 0.5 (stricter). Use 0.45–0.5 to avoid different people in one cluster;
            0.55–0.65 if same person is split.
        min_faces_per_person: If set, use DBSCAN: only form a person if at least this many
            faces match. Singleton faces become "person_unassigned". Immich uses 3-20.
            None = use AgglomerativeClustering (every face gets a person).
        linkage: "complete" (default) = tighter clusters, fewer false merges; "average" = more merging.

    Returns:
        Summary: { "person_count": N, "face_count": M, "updated": M, "unassigned": U }.
    """
    # Cosine distance -> L2 for unit vectors: L2 = sqrt(2 * cos_dist)
    l2_threshold = math.sqrt(2.0 * float(distance_threshold))
    agg_linkage = "complete" if linkage != "average" else "average"
    data = chroma_service.get_all_faces(include_embeddings=True)
    ids = data.get("ids", [])
    embeddings = data.get("embeddings", [])
    metadatas = data.get("metadatas", [])

    if len(ids) == 0 or embeddings is None or len(embeddings) == 0:
        logger.info("No faces to cluster.")
        return {"person_count": 0, "face_count": 0, "updated": 0, "unassigned": 0}

    X = np.array(embeddings, dtype=np.float32)
    n = len(ids)

    if n == 1:
        labels = [0] if min_faces_per_person is None or min_faces_per_person <= 1 else [-1]
    else:
        if min_faces_per_person is not None and min_faces_per_person >= 2:
            clustering = DBSCAN(
                eps=l2_threshold,
                min_samples=min_faces_per_person,
                metric="euclidean",
                n_jobs=-1,
            )
            labels = clustering.fit_predict(X)
        else:
            clustering = AgglomerativeClustering(
                n_clusters=None,
                distance_threshold=l2_threshold,
                metric="euclidean",
                linkage=agg_linkage,
            )
            labels = clustering.fit_predict(X)

    # Build old person_id -> face_ids (exclude person_unassigned for matching)
    old_person_faces: Dict[str, Set[str]] = {}
    for i, meta in enumerate(metadatas or []):
        pid = meta.get("person_id", "")
        if not pid or pid == "person_unassigned":
            continue
        fid = ids[i] if i < len(ids) else ""
        if fid:
            old_person_faces.setdefault(pid, set()).add(fid)

    # Build new label -> face_ids
    new_label_faces: Dict[int, Set[str]] = {}
    for i, lb in enumerate(labels):
        if lb < 0:
            continue
        fid = ids[i] if i < len(ids) else ""
        if fid:
            new_label_faces.setdefault(lb, set()).add(fid)

    # Match new clusters to existing person_ids by face overlap (stable IDs across re-clusters)
    next_new_idx = _max_person_index(old_person_faces) + 1
    used_old_ids: Set[str] = set()
    label_to_person: Dict[int, str] = {}

    for lb in sorted(new_label_faces.keys(), key=lambda l: -len(new_label_faces[l])):
        cluster_faces = new_label_faces[lb]
        best_pid = None
        best_overlap = 0
        for pid, face_set in old_person_faces.items():
            if pid in used_old_ids:
                continue
            overlap = len(cluster_faces & face_set)
            if overlap > best_overlap:
                best_overlap = overlap
                best_pid = pid
        if best_pid and best_overlap > 0:
            label_to_person[lb] = best_pid
            used_old_ids.add(best_pid)
        else:
            label_to_person[lb] = f"person_{next_new_idx}"
            next_new_idx += 1

    unassigned_count = 0
    new_metadatas = []
    for i, meta in enumerate(metadatas or []):
        lb = labels[i]
        if lb < 0:
            person_id = "person_unassigned"
            unassigned_count += 1
        else:
            person_id = label_to_person.get(lb, f"person_{next_new_idx}")
        new_meta = {
            "photo_uuid": meta.get("photo_uuid", ""),
            "thumbnail": meta.get("thumbnail", ""),
            "person_id": person_id,
        }
        new_metadatas.append(new_meta)

    chroma_service.update_face_metadatas(ids, new_metadatas)
    person_count = len(label_to_person) + (1 if unassigned_count > 0 else 0)
    logger.info(
        f"Clustering: {len(label_to_person)} persons, {unassigned_count} unassigned, "
        f"{n} faces total (cosine_dist={distance_threshold}, L2={l2_threshold:.3f}, linkage={agg_linkage}, min_faces={min_faces_per_person})."
    )
    return {
        "person_count": person_count,
        "face_count": n,
        "updated": n,
        "unassigned": unassigned_count,
    }


def list_persons() -> List[Dict[str, Any]]:
    """
    List all persons: for each person_id, return name, face_count, photo_count, and sample thumbnail.
    """
    data = chroma_service.get_all_faces(include_embeddings=False)
    ids = data.get("ids", [])
    metadatas = data.get("metadatas", [])

    # Group by person_id
    by_person: Dict[str, Dict[str, Any]] = {}
    names = _load_person_names()

    for i, meta in enumerate(metadatas or []):
        pid = meta.get("person_id", "")
        if pid == "":
            pid = "_unassigned"
        if pid not in by_person:
            by_person[pid] = {"person_id": pid if pid != "_unassigned" else "", "face_ids": [], "photo_uuids": set(), "thumbnail": meta.get("thumbnail", "")}
        by_person[pid]["face_ids"].append(ids[i] if i < len(ids) else "")
        by_person[pid]["photo_uuids"].add(meta.get("photo_uuid", ""))

    result = []
    def _sort_key(item):
        pid, info = item[0], item[1]
        photo_count = len(info["photo_uuids"])
        if pid == "_unassigned" or pid == "person_unassigned":
            return (1, 0, pid)  # unassigned at end
        return (0, -photo_count, pid)  # most photos first
    for pid, info in sorted(by_person.items(), key=_sort_key):
        person_id = info["person_id"]
        result.append({
            "person_id": person_id,
            "name": names.get(person_id, "") if person_id else "",
            "face_count": len(info["face_ids"]),
            "photo_count": len(info["photo_uuids"]),
            "thumbnail": info["thumbnail"] or "",
        })
    return result


def get_photo_uuids_for_person(person_id: str) -> List[str]:
    """Return list of photo UUIDs that contain this person (unique)."""
    data = chroma_service.get_all_faces(include_embeddings=False)
    metadatas = data.get("metadatas", [])
    uuids = set()
    for meta in metadatas or []:
        if meta.get("person_id") == person_id:
            uuids.add(meta.get("photo_uuid", ""))
    return sorted(uuids)
