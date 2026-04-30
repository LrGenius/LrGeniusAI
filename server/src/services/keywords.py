"""Keyword clustering, LLM-based synonym validation, and keyword merge application."""

import json
from typing import Any

import numpy as np
import torch
import torch.nn.functional as F

from config import logger, TORCH_DEVICE


def embed_keywords_batched(
    keyword_names: list[str],
    model: Any,
    tokenizer: Any,
    batch_size: int = 256,
) -> np.ndarray:
    ctx_len = getattr(model, "context_length", 77)
    parts: list[np.ndarray] = []
    for i in range(0, len(keyword_names), batch_size):
        batch = keyword_names[i : i + batch_size]
        with torch.no_grad():
            tokens = tokenizer(batch, context_length=ctx_len).to(TORCH_DEVICE)
            features = model.encode_text(tokens)
            parts.append(F.normalize(features, p=2, dim=1).cpu().numpy())
    return np.vstack(parts)


def _call_llm_text(
    provider: str,
    model: str | None,
    api_key: str | None,
    ollama_base_url: str | None,
    lmstudio_base_url: str | None,
    system_prompt: str,
    user_prompt: str,
) -> str | None:
    """Make a text-only LLM call. Returns raw text or None on failure."""
    try:
        if provider == "chatgpt":
            from openai import OpenAI

            client = OpenAI(api_key=api_key, timeout=120)
            resp = client.chat.completions.create(
                model=model or "gpt-4.1",
                messages=[
                    {"role": "system", "content": system_prompt},
                    {"role": "user", "content": user_prompt},
                ],
                temperature=0.1,
                max_tokens=4096,
            )
            return resp.choices[0].message.content

        elif provider == "gemini":
            import google.genai as genai
            from google.genai import types

            client = genai.Client(api_key=api_key)
            resp = client.models.generate_content(
                model=model or "gemini-2.0-flash",
                contents=user_prompt,
                config=types.GenerateContentConfig(
                    system_instruction=system_prompt,
                    temperature=0.1,
                    max_output_tokens=4096,
                ),
            )
            return resp.text

        elif provider == "ollama":
            from ollama import Client  # type: ignore[import]

            base = ollama_base_url or "http://localhost:11434"
            client = Client(host=base, timeout=120)
            resp = client.chat(
                model=model or "llama3",
                messages=[
                    {"role": "system", "content": system_prompt},
                    {"role": "user", "content": user_prompt},
                ],
            )
            return resp.message.content

        elif provider == "lmstudio":
            from openai import OpenAI

            base = lmstudio_base_url or "localhost:1234"
            if not base.startswith("http"):
                base = f"http://{base}/v1"
            client = OpenAI(base_url=base, api_key="lm-studio", timeout=120)
            resp = client.chat.completions.create(
                model=model or "local-model",
                messages=[
                    {"role": "system", "content": system_prompt},
                    {"role": "user", "content": user_prompt},
                ],
                temperature=0.1,
                max_tokens=4096,
            )
            return resp.choices[0].message.content

    except Exception as e:
        logger.error(f"_call_llm_text ({provider}): {e}", exc_info=True)
    return None


_VALIDATION_SYSTEM = (
    "You are a photo library metadata expert. "
    "Decide which groups of similar-sounding keywords from a photography catalog "
    "are true synonyms that should be merged into one keyword."
)


def validate_clusters_with_llm(
    candidate_clusters: list[list[str]],
    provider: str,
    model: str | None,
    api_key: str | None,
    ollama_base_url: str | None,
    lmstudio_base_url: str | None,
    chunk_size: int = 15,
) -> list[list[str]]:
    """
    Validate CLIP candidate clusters with an LLM.
    Returns refined clusters, each with the best canonical name first.
    Falls back to raw CLIP candidates if an LLM call fails.
    """
    if not candidate_clusters:
        return []

    validated: list[list[str]] = []

    for start in range(0, len(candidate_clusters), chunk_size):
        chunk = candidate_clusters[start : start + chunk_size]

        groups_text = "\n".join(
            f"{i + 1}. {json.dumps(group)}" for i, group in enumerate(chunk)
        )

        user_prompt = (
            "Below are groups of keywords that a similarity model found to be related. "
            "For each group, decide if the members describe the exact same concept "
            "(true synonyms → merge them) or distinct concepts (keep separate).\n\n"
            "Rules:\n"
            "- Merge only true synonyms (e.g. 'Car' and 'Automobile').\n"
            "- Do NOT merge related-but-different concepts "
            "(e.g. 'Cat' and 'Kitten' are different life stages).\n"
            "- You may split a group: only include the members that are genuine synonyms.\n"
            "- Put the clearest, most common name first — that becomes the canonical keyword.\n"
            "- If no members of a group should be merged, return an empty list [].\n\n"
            f"Groups:\n{groups_text}\n\n"
            "Return a JSON array with exactly one element per group, in the same order. "
            "Each element:\n"
            '  - Merge: ["BestName", "synonym1", ...] — canonical name first, at least 2 items\n'
            "  - No merge: []\n\n"
            "Return only the JSON array, no other text."
        )

        raw = _call_llm_text(
            provider,
            model,
            api_key,
            ollama_base_url,
            lmstudio_base_url,
            _VALIDATION_SYSTEM,
            user_prompt,
        )

        if raw is None:
            logger.warning(
                f"validate_clusters_with_llm: LLM call failed for chunk at {start}, keeping CLIP candidates"
            )
            validated.extend(g for g in chunk if len(g) >= 2)
            continue

        try:
            text = raw.strip()
            # Strip markdown code fences if present
            if text.startswith("```"):
                text = "\n".join(text.split("\n")[1:])
            if text.endswith("```"):
                text = text[: text.rfind("```")].strip()

            parsed = json.loads(text)
            if not isinstance(parsed, list):
                raise ValueError("response is not a JSON array")

            # Align with chunk length in case LLM over/under-counts
            if len(parsed) != len(chunk):
                logger.warning(
                    f"validate_clusters_with_llm: got {len(parsed)} results for {len(chunk)} groups"
                )
                parsed = (parsed + [[]] * len(chunk))[: len(chunk)]

            for item in parsed:
                if isinstance(item, list) and len(item) >= 2:
                    clean = [str(s).strip() for s in item if str(s).strip()]
                    if len(clean) >= 2:
                        validated.append(clean)

        except Exception as e:
            logger.error(f"validate_clusters_with_llm: parse error: {e}", exc_info=True)
            validated.extend(g for g in chunk if len(g) >= 2)

    return validated


def _replace_in_keyword_structure(
    kw_data: Any, merge_map: dict[str, str]
) -> tuple[Any, bool]:
    """Recursively replace keyword names in a list/dict/str keyword structure.
    Returns (updated_data, changed).
    """
    if isinstance(kw_data, list):
        new_list: list = []
        seen: set[str] = set()
        changed = False
        for item in kw_data:
            new_item, item_changed = _replace_in_keyword_structure(item, merge_map)
            if item_changed:
                changed = True
            norm = new_item.lower() if isinstance(new_item, str) else None
            if norm is None or norm not in seen:
                if norm:
                    seen.add(norm)
                new_list.append(new_item)
            else:
                changed = True  # duplicate removed
        return new_list, changed

    if isinstance(kw_data, dict):
        new_dict: dict = {}
        changed = False
        for k, v in kw_data.items():
            new_v, sub_changed = _replace_in_keyword_structure(v, merge_map)
            new_dict[k] = new_v
            if sub_changed:
                changed = True
        return new_dict, changed

    if isinstance(kw_data, str):
        replacement = merge_map.get(kw_data.lower(), kw_data)
        return replacement, replacement != kw_data

    return kw_data, False


def apply_keyword_merges(merges: list[dict]) -> dict:
    """Replace duplicate keyword names with their canonical equivalents in all
    photo metadata stored in ChromaDB.

    Args:
        merges: list of {duplicate: str, canonical: str} dicts

    Returns:
        {updated_photos: int}
    """
    from services import chroma as chroma_service

    if not merges:
        return {"updated_photos": 0}

    merge_map: dict[str, str] = {}
    for m in merges:
        dup = (m.get("duplicate") or "").strip()
        can = (m.get("canonical") or "").strip()
        if dup and can and dup.lower() != can.lower():
            merge_map[dup.lower()] = can

    if not merge_map:
        return {"updated_photos": 0}

    col = chroma_service.collection
    if col is None:
        return {"updated_photos": 0}

    data = col.get(include=["metadatas"], limit=chroma_service.STATS_GET_LIMIT)
    ids: list[str] = data.get("ids", [])
    metadatas: list[dict] = data.get("metadatas", [])

    updated_ids: list[str] = []
    updated_metas: list[dict] = []

    for photo_id, meta in zip(ids, metadatas):
        if not meta:
            continue
        changed = False
        new_meta = dict(meta)

        # flattened_keywords — comma-separated string
        flat = new_meta.get("flattened_keywords", "") or ""
        if flat:
            parts = [p.strip() for p in flat.split(",") if p.strip()]
            new_parts: list[str] = []
            seen_flat: set[str] = set()
            for p in parts:
                rep = merge_map.get(p.lower(), p)
                norm = rep.lower()
                if norm not in seen_flat:
                    seen_flat.add(norm)
                    new_parts.append(rep)
                    if rep != p:
                        changed = True
                else:
                    changed = True  # deduplicated
            new_meta["flattened_keywords"] = ", ".join(new_parts)

        # keywords — JSON-encoded list or dict
        kw_json = new_meta.get("keywords", "") or ""
        if kw_json:
            try:
                kw_data = json.loads(kw_json)
                kw_data, kw_changed = _replace_in_keyword_structure(kw_data, merge_map)
                if kw_changed:
                    new_meta["keywords"] = json.dumps(kw_data)
                    changed = True
            except (json.JSONDecodeError, TypeError):
                pass

        if changed:
            updated_ids.append(photo_id)
            updated_metas.append(new_meta)

    _BATCH = 500
    for start in range(0, len(updated_ids), _BATCH):
        col.update(
            ids=updated_ids[start : start + _BATCH],
            metadatas=updated_metas[start : start + _BATCH],
        )

    logger.info(
        f"apply_keyword_merges: updated {len(updated_ids)} photo(s) for {len(merge_map)} merge(s)"
    )
    return {"updated_photos": len(updated_ids)}
