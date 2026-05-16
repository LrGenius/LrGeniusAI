"""Tool schemas and dispatch for the catalog chat agent."""

from __future__ import annotations

import uuid
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Callable

from config import logger

if TYPE_CHECKING:
    from services.agent import Session

RESULT_REF_THRESHOLD = 50


@dataclass
class ToolSpec:
    name: str
    description: str
    json_schema: dict
    handler: Callable[[dict, "Session"], dict]
    requires_catalog: bool = True


TOOLS: dict[str, ToolSpec] = {}


def _register(spec: ToolSpec) -> ToolSpec:
    TOOLS[spec.name] = spec
    return spec


def _mint_result_ref(sess: "Session", photo_ids: list[str]) -> str:
    ref = f"r_{uuid.uuid4().hex[:8]}"
    sess.result_refs[ref] = photo_ids
    sess.retrieved_photo_ids.update(photo_ids)
    return ref


def _resolve_scope(sess: "Session", scope_photo_ids: Any) -> list[str] | None:
    if scope_photo_ids is None:
        return None
    if isinstance(scope_photo_ids, str) and scope_photo_ids.startswith("r_"):
        return sess.result_refs.get(scope_photo_ids, [])
    if isinstance(scope_photo_ids, list):
        return scope_photo_ids
    return None


def dispatch(name: str, args: dict, sess: "Session") -> dict:
    spec = TOOLS.get(name)
    if spec is None:
        return {"error": f"Unknown tool: {name!r}"}
    try:
        result = spec.handler(args, sess)
    except Exception as e:
        logger.error("Tool %s raised: %s", name, e, exc_info=True)
        return {"error": str(e)}
    return result


# ---------------------------------------------------------------------------
# Tool implementations
# ---------------------------------------------------------------------------


def _handle_semantic_search(args: dict, sess: "Session") -> dict:
    from services import search as search_service

    query = str(args.get("query", "")).strip()
    max_results = min(int(args.get("max_results", 30)), 200)
    strictness_label = str(args.get("strictness", "medium")).lower()
    strictness_map = {"low": 20, "medium": 50, "high": 80}
    strictness = strictness_map.get(strictness_label, 50)
    quality_sort = "prettiest" if args.get("quality_sort") else "relevance"
    scope_ids = _resolve_scope(sess, args.get("scope_photo_ids"))

    results = search_service.search_images(
        term=query,
        quality_sort=quality_sort,
        photo_ids_to_search=scope_ids,
        search_sources={
            "semantic_siglip": True,
            "semantic_vertex": True,
            "metadata": False,
        },
        catalog_id=sess.catalog_id,
        relevance_strictness=strictness,
        max_results=max_results,
    )
    photo_ids = [r["photo_id"] for r in results]
    top_scores = [round(1.0 - r["distance"], 4) for r in results[:5]]

    sess.retrieved_photo_ids.update(photo_ids)

    result_ref = None
    if len(photo_ids) > RESULT_REF_THRESHOLD:
        result_ref = _mint_result_ref(sess, photo_ids)

    return {
        "photo_ids": photo_ids
        if len(photo_ids) <= RESULT_REF_THRESHOLD
        else photo_ids[:RESULT_REF_THRESHOLD],
        "summary": {"count": len(photo_ids), "top_scores": top_scores},
        "result_ref": result_ref,
    }


_register(
    ToolSpec(
        name="semantic_search",
        description="Search for photos by semantic similarity to a text query.",
        json_schema={
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language search query",
                },
                "max_results": {"type": "integer", "default": 30},
                "strictness": {
                    "type": "string",
                    "enum": ["low", "medium", "high"],
                    "default": "medium",
                },
                "quality_sort": {"type": "boolean", "default": False},
                "scope_photo_ids": {
                    "oneOf": [
                        {"type": "array", "items": {"type": "string"}},
                        {
                            "type": "string",
                            "description": "result_ref from a previous tool call",
                        },
                        {"type": "null"},
                    ],
                    "description": "Limit search to this set of photo IDs or a result_ref",
                },
            },
            "required": ["query"],
        },
        handler=_handle_semantic_search,
    )
)


def _handle_similar_to(args: dict, sess: "Session") -> dict:
    from services import chroma as chroma_service

    photo_id = str(args.get("photo_id", "")).strip()
    max_results = min(int(args.get("max_results", 30)), 200)

    if not photo_id:
        return {"error": "photo_id is required"}

    data = chroma_service.get_image(photo_id)
    embeddings = data.get("embeddings", [])
    if not embeddings or embeddings[0] is None:
        return {"error": f"No embedding found for photo_id {photo_id!r}"}

    embedding = embeddings[0]
    results = chroma_service.query_images(
        query_embedding=embedding,
        n_results=max_results + 1,
        catalog_id=sess.catalog_id,
    )
    ids = (results.get("ids") or [[]])[0]
    distances = (results.get("distances") or [[]])[0]

    photo_ids = [i for i in ids if i != photo_id][:max_results]
    dist_map = {i: d for i, d in zip(ids, distances)}
    top_scores = [
        round(1.0 - dist_map[pid], 4) for pid in photo_ids[:5] if pid in dist_map
    ]

    sess.retrieved_photo_ids.update(photo_ids)

    result_ref = None
    if len(photo_ids) > RESULT_REF_THRESHOLD:
        result_ref = _mint_result_ref(sess, photo_ids)

    return {
        "photo_ids": photo_ids
        if len(photo_ids) <= RESULT_REF_THRESHOLD
        else photo_ids[:RESULT_REF_THRESHOLD],
        "summary": {"count": len(photo_ids), "top_scores": top_scores},
        "result_ref": result_ref,
    }


_register(
    ToolSpec(
        name="similar_to",
        description="Find photos visually similar to a given photo.",
        json_schema={
            "type": "object",
            "properties": {
                "photo_id": {"type": "string"},
                "max_results": {"type": "integer", "default": 30},
            },
            "required": ["photo_id"],
        },
        handler=_handle_similar_to,
    )
)


def _handle_list_persons(args: dict, sess: "Session") -> dict:
    from services import persons as persons_service

    all_persons = persons_service.list_persons()
    all_persons.sort(key=lambda p: p.get("photo_count", 0), reverse=True)
    truncated = len(all_persons) > 50
    persons = all_persons[:50]

    return {
        "persons": persons,
        "summary": {
            "count": len(persons),
            "truncated": truncated,
            "total": len(all_persons),
        },
    }


_register(
    ToolSpec(
        name="list_persons",
        description="List known persons in the catalog with their names and photo counts.",
        json_schema={"type": "object", "properties": {}},
        handler=_handle_list_persons,
        requires_catalog=False,
    )
)


def _handle_photos_of_person(args: dict, sess: "Session") -> dict:
    from services import persons as persons_service

    person_id = args.get("person_id")
    name = args.get("name")
    scope_ids = _resolve_scope(sess, args.get("scope_photo_ids"))

    if not person_id and not name:
        return {"error": "Either person_id or name is required"}

    if not person_id and name:
        names = persons_service._load_person_names()
        name_lower = name.lower()
        matches = [pid for pid, n in names.items() if n.lower() == name_lower]
        partial = (
            [pid for pid, n in names.items() if name_lower in n.lower()]
            if not matches
            else []
        )

        if len(matches) == 1:
            person_id = matches[0]
        elif len(matches) > 1:
            return {
                "error": "ambiguous",
                "candidates": [{"person_id": p, "name": names[p]} for p in matches],
            }
        elif len(partial) == 1:
            person_id = partial[0]
        elif len(partial) > 1:
            return {
                "error": "ambiguous",
                "candidates": [{"person_id": p, "name": names[p]} for p in partial[:5]],
            }
        else:
            return {"error": f"No person found with name {name!r}"}

    photo_ids = persons_service.get_photo_ids_for_person(person_id)
    if scope_ids is not None:
        scope_set = set(scope_ids)
        photo_ids = [pid for pid in photo_ids if pid in scope_set]

    sess.retrieved_photo_ids.update(photo_ids)

    result_ref = None
    if len(photo_ids) > RESULT_REF_THRESHOLD:
        result_ref = _mint_result_ref(sess, photo_ids)

    person_name = ""
    try:
        person_name = persons_service.get_person_name(person_id) or person_id
    except Exception:
        person_name = person_id

    return {
        "photo_ids": photo_ids
        if len(photo_ids) <= RESULT_REF_THRESHOLD
        else photo_ids[:RESULT_REF_THRESHOLD],
        "summary": {
            "count": len(photo_ids),
            "person_id": person_id,
            "person_name": person_name,
        },
        "result_ref": result_ref,
    }


_register(
    ToolSpec(
        name="photos_of_person",
        description="Get photo IDs for a specific person by name or person_id.",
        json_schema={
            "type": "object",
            "properties": {
                "person_id": {
                    "type": "string",
                    "description": "Exact person_id (e.g. person_0)",
                },
                "name": {
                    "type": "string",
                    "description": "Display name to look up (case-insensitive)",
                },
                "scope_photo_ids": {
                    "oneOf": [
                        {"type": "array", "items": {"type": "string"}},
                        {"type": "string"},
                        {"type": "null"},
                    ],
                },
            },
        },
        handler=_handle_photos_of_person,
    )
)


def _handle_metadata_query(args: dict, sess: "Session") -> dict:
    from services import chroma as chroma_service

    filters = args.get("filters", {})
    max_results = min(int(args.get("max_results", 200)), 500)

    # Pre-scope
    scope_ids = _resolve_scope(sess, filters.get("photo_ids"))

    # Query ChromaDB for all metadata and apply Python-side filters
    # (ChromaDB's where filter supports only simple equality/range on scalar fields)
    try:
        if scope_ids is not None:
            chunks = [scope_ids[i : i + 500] for i in range(0, len(scope_ids), 500)]
            all_rows = []
            for chunk in chunks:
                data = chroma_service.collection.get(ids=chunk, include=["metadatas"])
                ids_ = data.get("ids", [])
                metas_ = data.get("metadatas", [])
                all_rows.extend(zip(ids_, metas_))
        else:
            from services.chroma import STATS_GET_LIMIT

            data = chroma_service.collection.get(
                include=["metadatas"], limit=STATS_GET_LIMIT
            )
            ids_ = data.get("ids", [])
            metas_ = data.get("metadatas", [])
            all_rows = list(zip(ids_, metas_))
    except Exception as e:
        logger.error("metadata_query ChromaDB fetch failed: %s", e, exc_info=True)
        return {"error": str(e)}

    # Apply filters
    catalog_id = filters.get("catalog_id") or sess.catalog_id

    def _passes(photo_id: str, meta: dict) -> bool:
        if meta is None:
            return False
        # catalog filter
        if catalog_id:
            from services.chroma import _parse_catalog_ids

            cids = _parse_catalog_ids(meta)
            if str(catalog_id) not in cids and cids:  # if cids is empty, don't filter
                if cids:
                    return False

        # has_embedding
        if filters.get("has_indexed_description") is not None:
            has_desc = bool((meta.get("caption") or meta.get("title") or "").strip())
            if bool(filters["has_indexed_description"]) != has_desc:
                return False

        if filters.get("has_keywords") is not None:
            has_kw = bool(
                (meta.get("flattened_keywords") or meta.get("keywords") or "").strip()
            )
            if bool(filters["has_keywords"]) != has_kw:
                return False

        # date range — capture_time is stored as string "YYYY-MM-DD HH:MM:SS" or ISO
        date_range = filters.get("date_range")
        if date_range and isinstance(date_range, list) and len(date_range) == 2:
            ct = (meta.get("capture_time") or "").strip()
            if ct:
                from_dt = str(date_range[0])[:10]
                to_dt = str(date_range[1])[:10]
                ct_date = ct[:10]
                if ct_date < from_dt or ct_date > to_dt:
                    return False

        # keyword text filters
        keywords_any = filters.get("keywords_any")
        if keywords_any and isinstance(keywords_any, list):
            flat_kw = (
                meta.get("flattened_keywords") or meta.get("keywords") or ""
            ).lower()
            if not any(k.lower() in flat_kw for k in keywords_any):
                return False

        keywords_all = filters.get("keywords_all")
        if keywords_all and isinstance(keywords_all, list):
            flat_kw = (
                meta.get("flattened_keywords") or meta.get("keywords") or ""
            ).lower()
            if not all(k.lower() in flat_kw for k in keywords_all):
                return False

        return True

    matched = []
    for photo_id, meta in all_rows:
        if _passes(photo_id, meta or {}):
            matched.append(
                {
                    "photo_id": photo_id,
                    "capture_time": (meta or {}).get("capture_time", ""),
                    "keywords": (meta or {}).get("flattened_keywords", ""),
                }
            )
        if len(matched) >= max_results:
            break

    photo_ids = [r["photo_id"] for r in matched]
    sess.retrieved_photo_ids.update(photo_ids)

    result_ref = None
    if len(photo_ids) > RESULT_REF_THRESHOLD:
        result_ref = _mint_result_ref(sess, photo_ids)

    return {
        "photo_ids": photo_ids
        if len(photo_ids) <= RESULT_REF_THRESHOLD
        else photo_ids[:RESULT_REF_THRESHOLD],
        "results": matched
        if len(matched) <= RESULT_REF_THRESHOLD
        else matched[:RESULT_REF_THRESHOLD],
        "summary": {"count": len(photo_ids)},
        "result_ref": result_ref,
    }


_register(
    ToolSpec(
        name="metadata_query",
        description=(
            "Query photos by metadata filters. Available filters: "
            "photo_ids (list, pre-scope), date_range ([ISO date, ISO date]), "
            "keywords_any (list of strings, OR), keywords_all (list of strings, AND), "
            "has_keywords (bool), has_indexed_description (bool)."
        ),
        json_schema={
            "type": "object",
            "properties": {
                "filters": {
                    "type": "object",
                    "properties": {
                        "photo_ids": {
                            "oneOf": [
                                {"type": "array", "items": {"type": "string"}},
                                {"type": "string"},
                                {"type": "null"},
                            ]
                        },
                        "date_range": {
                            "type": "array",
                            "items": {"type": "string"},
                            "minItems": 2,
                            "maxItems": 2,
                        },
                        "keywords_any": {"type": "array", "items": {"type": "string"}},
                        "keywords_all": {"type": "array", "items": {"type": "string"}},
                        "has_keywords": {"type": "boolean"},
                        "has_indexed_description": {"type": "boolean"},
                    },
                },
                "max_results": {"type": "integer", "default": 200},
            },
        },
        handler=_handle_metadata_query,
    )
)


def _handle_photo_details(args: dict, sess: "Session") -> dict:
    from services import chroma as chroma_service

    photo_ids = args.get("photo_ids", [])
    if isinstance(photo_ids, str) and photo_ids.startswith("r_"):
        photo_ids = sess.result_refs.get(photo_ids, [])
    photo_ids = list(photo_ids)[:25]

    if not photo_ids:
        return {"error": "photo_ids is required"}

    try:
        data = chroma_service.collection.get(ids=photo_ids, include=["metadatas"])
    except Exception as e:
        return {"error": str(e)}

    ids_ = data.get("ids", [])
    metas_ = data.get("metadatas", [])

    # Get person associations
    details = []
    for pid, meta in zip(ids_, metas_):
        m = meta or {}
        details.append(
            {
                "photo_id": pid,
                "filename": m.get("filename", ""),
                "capture_time": m.get("capture_time", ""),
                "title": m.get("title", ""),
                "caption": m.get("caption", ""),
                "alt_text": m.get("alt_text", ""),
                "keywords": m.get("flattened_keywords", ""),
                "has_embedding": m.get("has_embedding", False),
                "provider": m.get("provider", ""),
                "model": m.get("model", ""),
            }
        )

    sess.retrieved_photo_ids.update(ids_)

    return {"photos": details, "summary": {"count": len(details)}}


_register(
    ToolSpec(
        name="photo_details",
        description="Get detailed metadata for up to 25 photos by ID.",
        json_schema={
            "type": "object",
            "properties": {
                "photo_ids": {
                    "oneOf": [
                        {"type": "array", "items": {"type": "string"}, "maxItems": 25},
                        {"type": "string", "description": "result_ref"},
                    ],
                },
            },
            "required": ["photo_ids"],
        },
        handler=_handle_photo_details,
    )
)


def _handle_index_stats(args: dict, sess: "Session") -> dict:
    from services import db as db_service

    try:
        stats = db_service.get_database_stats(catalog_id=sess.catalog_id)
    except Exception as e:
        return {"error": str(e)}
    return stats


_register(
    ToolSpec(
        name="index_stats",
        description="Return statistics about the indexed catalog: total photos, with descriptions, with keywords, etc.",
        json_schema={"type": "object", "properties": {}},
        handler=_handle_index_stats,
        requires_catalog=False,
    )
)


def _handle_propose_action(args: dict, sess: "Session") -> dict:
    kind = args.get("kind", "")
    payload = args.get("payload", {})
    dry_run_summary = str(args.get("dry_run_summary", ""))

    valid_kinds = {
        "create_collection",
        "set_rating",
        "set_flag",
        "set_color_label",
        "export_csv",
    }
    if kind not in valid_kinds:
        return {"error": f"Invalid action kind {kind!r}. Valid: {sorted(valid_kinds)}"}

    # Validate photo_ids in payload against retrieved set
    payload_ids = set(payload.get("photo_ids", []))
    if payload_ids and not payload_ids.issubset(sess.retrieved_photo_ids):
        unknown = list(payload_ids - sess.retrieved_photo_ids)[:5]
        return {
            "error": f"Proposal references photo_ids not retrieved this session: {unknown}"
        }

    proposal_id = f"p_{uuid.uuid4().hex[:12]}"
    return {
        "proposal_id": proposal_id,
        "kind": kind,
        "payload": payload,
        "dry_run_summary": dry_run_summary,
    }


_register(
    ToolSpec(
        name="propose_action",
        description=(
            "Propose a catalog action for the user to confirm. "
            "kind must be one of: create_collection, set_rating, set_flag, set_color_label, export_csv. "
            "payload should include photo_ids and action-specific params. "
            "dry_run_summary is shown to the user."
        ),
        json_schema={
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": [
                        "create_collection",
                        "set_rating",
                        "set_flag",
                        "set_color_label",
                        "export_csv",
                    ],
                },
                "payload": {"type": "object"},
                "dry_run_summary": {"type": "string"},
            },
            "required": ["kind", "payload", "dry_run_summary"],
        },
        handler=_handle_propose_action,
    )
)
