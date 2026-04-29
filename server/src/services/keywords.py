"""Keyword clustering and LLM-based synonym validation."""

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
    chunk_size: int = 30,
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
