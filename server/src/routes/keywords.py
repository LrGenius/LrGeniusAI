from flask import Blueprint, request, jsonify
import numpy as np

from config import logger
import server_lifecycle
from services.keywords import embed_keywords_batched, validate_clusters_with_llm

keywords_bp = Blueprint("keywords", __name__)

_KNOWN_PROVIDERS = {"chatgpt", "gemini", "ollama", "lmstudio"}


@keywords_bp.route("/keywords/cluster", methods=["POST"])
def cluster_keywords():
    """
    Embed keyword names with the CLIP text encoder, cluster semantically similar
    terms, then optionally validate clusters with an LLM for higher precision.

    Body JSON:
        keywords          list[str]   Keyword names to cluster
        threshold         float       CLIP cosine similarity threshold
                                      (default 0.85 with LLM, 0.88 without)
        provider          str|null    LLM provider ('chatgpt','gemini','ollama','lmstudio')
        model             str|null    LLM model name
        api_key           str|null    API key for cloud providers
        ollama_base_url   str|null    Ollama server URL
        lmstudio_base_url str|null    LM Studio server URL

    Response:
        results   list[list[str]]  Each inner list is one cluster; first entry is canonical
        warning   str|null
    """
    data = request.get_json() or {}
    keyword_names = data.get("keywords", [])
    provider = data.get("provider") or None
    model = data.get("model") or None
    api_key = data.get("api_key") or None
    ollama_base_url = data.get("ollama_base_url") or None
    lmstudio_base_url = data.get("lmstudio_base_url") or None

    use_llm = provider in _KNOWN_PROVIDERS
    default_threshold = 0.85 if use_llm else 0.88
    threshold = float(data.get("threshold", default_threshold))
    threshold = max(0.5, min(threshold, 1.0))

    if not isinstance(keyword_names, list):
        return jsonify(
            {"error": "keywords must be a list", "results": [], "warning": None}
        ), 400

    # Deduplicate preserving original casing + order
    seen: set[str] = set()
    unique: list[str] = []
    for name in keyword_names:
        if not isinstance(name, str):
            continue
        norm = name.strip().lower()
        if norm and norm not in seen:
            seen.add(norm)
            unique.append(name.strip())

    if len(unique) < 2:
        return jsonify({"results": [], "error": None, "warning": None}), 200

    tokenizer = server_lifecycle.get_tokenizer()
    clip_model = server_lifecycle.get_model()
    if tokenizer is None or clip_model is None:
        return jsonify(
            {
                "results": [],
                "error": None,
                "warning": "CLIP model not available; semantic clustering skipped.",
            }
        ), 200

    try:
        embeddings = embed_keywords_batched(unique, clip_model, tokenizer)
    except Exception as e:
        logger.error(f"cluster_keywords: embedding failed: {e}", exc_info=True)
        return jsonify(
            {
                "results": [],
                "error": None,
                "warning": f"Embedding failed: {e}",
            }
        ), 200

    # Pairwise cosine similarity via dot product of L2-normalised vectors
    sim_matrix: np.ndarray = np.dot(embeddings, embeddings.T)

    # Union-find with path compression
    parent = list(range(len(unique)))

    def find(x: int) -> int:
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    n = len(unique)
    for i in range(n):
        for j in range(i + 1, n):
            if float(sim_matrix[i, j]) >= threshold:
                pi, pj = find(i), find(j)
                if pi != pj:
                    parent[pi] = pj

    groups: dict[int, list[int]] = {}
    for i in range(n):
        root = find(i)
        groups.setdefault(root, []).append(i)

    candidates = [
        [unique[i] for i in sorted(members)]
        for members in groups.values()
        if len(members) >= 2
    ]

    logger.info(
        f"cluster_keywords: {len(unique)} keywords → {len(candidates)} CLIP candidate(s) "
        f"(threshold={threshold}, llm={provider or 'none'})"
    )

    if use_llm and candidates:
        try:
            clusters = validate_clusters_with_llm(
                candidates,
                provider,
                model,
                api_key,
                ollama_base_url,
                lmstudio_base_url,
            )
            logger.info(
                f"cluster_keywords: LLM reduced {len(candidates)} candidates → {len(clusters)} confirmed clusters"
            )
        except Exception as e:
            logger.error(f"cluster_keywords: LLM validation error: {e}", exc_info=True)
            clusters = candidates
    else:
        clusters = candidates

    return jsonify({"results": clusters, "error": None, "warning": None}), 200
