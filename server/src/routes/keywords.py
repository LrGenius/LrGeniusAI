from flask import Blueprint, request, jsonify
import numpy as np
import torch
import torch.nn.functional as F

from config import logger, TORCH_DEVICE
import server_lifecycle

keywords_bp = Blueprint("keywords", __name__)

_MAX_KEYWORDS = 500


@keywords_bp.route("/keywords/cluster", methods=["POST"])
def cluster_keywords():
    """
    Embed keyword names with the CLIP text encoder and return clusters of
    semantically similar terms (cosine similarity >= threshold).

    Body JSON:
        keywords  list[str]   Keyword names to cluster
        threshold float       Cosine similarity threshold (default 0.88)

    Response:
        results   list[list[str]]  Each inner list is one cluster of >=2 names
        warning   str|null         Set when CLIP model is unavailable
    """
    data = request.get_json() or {}
    keyword_names = data.get("keywords", [])
    threshold = float(data.get("threshold", 0.88))
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

    if len(unique) > _MAX_KEYWORDS:
        unique = unique[:_MAX_KEYWORDS]
        logger.warning(f"cluster_keywords: input capped to {_MAX_KEYWORDS} keywords")

    tokenizer = server_lifecycle.get_tokenizer()
    model = server_lifecycle.get_model()
    if tokenizer is None or model is None:
        return jsonify(
            {
                "results": [],
                "error": None,
                "warning": "CLIP model not available; semantic clustering skipped.",
            }
        ), 200

    try:
        with torch.no_grad():
            tokens = tokenizer(unique).to(TORCH_DEVICE)
            features = model.encode_text(tokens)
            embeddings = F.normalize(features, p=2, dim=1).cpu().numpy()
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

    # Group indices by root
    groups: dict[int, list[int]] = {}
    for i in range(n):
        root = find(i)
        groups.setdefault(root, []).append(i)

    clusters = [
        [unique[i] for i in sorted(members)]
        for members in groups.values()
        if len(members) >= 2
    ]

    logger.info(
        f"cluster_keywords: {len(unique)} keywords → {len(clusters)} semantic cluster(s) "
        f"(threshold={threshold})"
    )
    return jsonify({"results": clusters, "error": None, "warning": None}), 200
