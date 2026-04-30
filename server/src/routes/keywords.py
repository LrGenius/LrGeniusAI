import threading

import numpy as np
from flask import Blueprint, jsonify, request

from config import logger
import server_lifecycle
from services.jobs import complete_job, create_job, fail_job, get_job
from services.keywords import (
    apply_keyword_merges,
    embed_keywords_batched,
    validate_clusters_with_llm,
)

keywords_bp = Blueprint("keywords", __name__)

_KNOWN_PROVIDERS = {"chatgpt", "gemini", "ollama", "lmstudio"}


def _parse_cluster_request(
    data: dict,
) -> (
    tuple[list[str], float, str | None, str | None, str | None, str | None, str | None]
    | tuple[None, None, None, None, None, None, str]
):
    keyword_names = data.get("keywords", [])
    if not isinstance(keyword_names, list):
        return None, None, None, None, None, None, "keywords must be a list"

    provider = data.get("provider") or None
    model = data.get("model") or None
    api_key = data.get("api_key") or None
    ollama_base_url = data.get("ollama_base_url") or None
    lmstudio_base_url = data.get("lmstudio_base_url") or None

    use_llm = provider in _KNOWN_PROVIDERS
    default_threshold = 0.85 if use_llm else 0.88
    threshold = float(data.get("threshold", default_threshold))
    threshold = max(0.5, min(threshold, 1.0))

    seen: set[str] = set()
    unique: list[str] = []
    for name in keyword_names:
        if not isinstance(name, str):
            continue
        norm = name.strip().lower()
        if norm and norm not in seen:
            seen.add(norm)
            unique.append(name.strip())

    return (
        unique,
        threshold,
        provider,
        model,
        api_key,
        ollama_base_url,
        lmstudio_base_url,
    )


def _run_clustering(
    unique: list[str],
    threshold: float,
    provider: str | None,
    model: str | None,
    api_key: str | None,
    ollama_base_url: str | None,
    lmstudio_base_url: str | None,
) -> dict:
    """Core clustering logic. Returns a result dict {results, warning}."""
    if len(unique) < 2:
        return {"results": [], "warning": None}

    tokenizer = server_lifecycle.get_tokenizer()
    clip_model = server_lifecycle.get_model()
    if tokenizer is None or clip_model is None:
        return {
            "results": [],
            "warning": "CLIP model not available; semantic clustering skipped.",
        }

    try:
        embeddings = embed_keywords_batched(unique, clip_model, tokenizer)
    except Exception as e:
        logger.error(f"cluster_keywords: embedding failed: {e}", exc_info=True)
        return {"results": [], "warning": f"Embedding failed: {e}"}

    sim_matrix: np.ndarray = np.dot(embeddings, embeddings.T)
    n = len(unique)

    # Build adjacency using numpy (fast) then find cliques with complete-linkage:
    # a keyword joins a cluster only when it's above threshold with every existing
    # member, preventing the transitive-closure chains that union-find creates.
    mask = np.triu(sim_matrix, k=1) >= threshold
    i_arr, j_arr = np.where(mask)

    adj: dict[int, set[int]] = {i: set() for i in range(n)}
    for i, j in zip(i_arr.tolist(), j_arr.tolist()):
        adj[i].add(j)
        adj[j].add(i)

    assigned: set[int] = set()
    all_candidate_groups: list[list[str]] = []

    for seed in sorted(range(n), key=lambda x: -len(adj[x])):
        if seed in assigned or not adj[seed]:
            continue
        cluster: set[int] = {seed}
        for candidate in sorted(adj[seed], key=lambda x: -len(adj[x])):
            if candidate in assigned or candidate in cluster:
                continue
            if all(candidate in adj[m] for m in cluster):
                cluster.add(candidate)
        if len(cluster) >= 2:
            all_candidate_groups.append([unique[i] for i in sorted(cluster)])
            assigned.update(cluster)

    candidates = all_candidate_groups

    use_llm = provider in _KNOWN_PROVIDERS
    logger.info(
        f"cluster_keywords: {len(unique)} keywords → {len(candidates)} CLIP candidate(s) "
        f"(threshold={threshold}, llm={provider or 'none'})"
    )

    warning = None
    if use_llm and candidates:
        try:
            clusters = validate_clusters_with_llm(
                candidates, provider, model, api_key, ollama_base_url, lmstudio_base_url
            )
            logger.info(
                f"cluster_keywords: LLM reduced {len(candidates)} candidates → {len(clusters)} confirmed clusters"
            )
        except Exception as e:
            logger.exception(f"cluster_keywords: LLM validation error: {e}")
            clusters = candidates
            warning = f"LLM validation failed ({provider}); results are CLIP-only and may include false positives."
    else:
        clusters = candidates

    return {"results": clusters, "warning": warning}


@keywords_bp.route("/keywords/cluster", methods=["POST"])
def cluster_keywords():
    """Synchronous clustering — kept for backwards compatibility."""
    data = request.get_json() or {}
    unique, threshold, provider, model, api_key, ollama_base_url, lmstudio_base_url = (
        _parse_cluster_request(data)
    )
    if unique is None:
        return jsonify({"error": threshold, "results": [], "warning": None}), 400

    result = _run_clustering(
        unique, threshold, provider, model, api_key, ollama_base_url, lmstudio_base_url
    )
    return jsonify(
        {"results": result["results"], "error": None, "warning": result["warning"]}
    ), 200


@keywords_bp.route("/keywords/cluster/start", methods=["POST"])
def cluster_keywords_start():
    """Kick off an async clustering job. Returns {job_id} immediately."""
    data = request.get_json() or {}
    unique, threshold, provider, model, api_key, ollama_base_url, lmstudio_base_url = (
        _parse_cluster_request(data)
    )
    if unique is None:
        return jsonify({"error": threshold, "results": [], "warning": None}), 400

    job_id = create_job()

    def _worker():
        try:
            result = _run_clustering(
                unique,
                threshold,
                provider,
                model,
                api_key,
                ollama_base_url,
                lmstudio_base_url,
            )
            complete_job(job_id, result)
        except Exception as e:
            logger.error(
                f"cluster_keywords async job {job_id} failed: {e}", exc_info=True
            )
            fail_job(job_id, str(e))

    threading.Thread(target=_worker, daemon=True).start()
    logger.info(
        f"cluster_keywords: started async job {job_id} for {len(unique)} keywords"
    )
    return jsonify({"job_id": job_id, "error": None, "warning": None}), 202


@keywords_bp.route("/keywords/cluster/status/<job_id>", methods=["GET"])
def cluster_keywords_status(job_id: str):
    """Poll status of an async clustering job."""
    job = get_job(job_id)
    if job is None:
        return jsonify({"error": "job not found", "status": None, "result": None}), 404
    return jsonify(
        {
            "status": job["status"],
            "result": job["result"],
            "error": job["error"],
        }
    ), 200


@keywords_bp.route("/keywords/apply-merges", methods=["POST"])
def keywords_apply_merges():
    """Apply keyword merge pairs to all photo metadata in ChromaDB.

    Body JSON:
        merges  list[{duplicate: str, canonical: str}]  Pairs to apply

    Response:
        updated_photos  int   Number of photos whose metadata was updated
        error           str|null
        warning         str|null
    """
    data = request.get_json() or {}
    merges = data.get("merges", [])
    if not isinstance(merges, list):
        return jsonify(
            {"error": "merges must be a list", "updated_photos": 0, "warning": None}
        ), 400

    try:
        result = apply_keyword_merges(merges)
    except Exception as e:
        logger.error(f"keywords_apply_merges: {e}", exc_info=True)
        return jsonify({"error": str(e), "updated_photos": 0, "warning": None}), 500

    return jsonify(
        {"updated_photos": result["updated_photos"], "error": None, "warning": None}
    ), 200
