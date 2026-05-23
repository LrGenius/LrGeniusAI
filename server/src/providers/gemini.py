"""
Gemini Provider for metadata generation using Google Generative AI API
"""

import json
import re
import time
from typing import Any, override
from .base import (
    LLMProviderBase,
    EditGenerationRequest,
    EditGenerationResponse,
    MetadataGenerationRequest,
    MetadataGenerationResponse,
)
from config import logger, DEFAULT_MAX_TOKENS
from utils.edit_recipe import GEMINI_EDIT_RECIPE_SCHEMA


# Vision-capable family prefix. Bare id must start with this.
_ALLOWED_PREFIX = "gemini-"

# Drop if id starts with one of these (deprecated / non-vision families).
_BLOCKED_PREFIXES: tuple[str, ...] = ("gemini-1.0",)

# Drop if id contains any of these substrings.
_BLOCKED_SUBSTRINGS: tuple[str, ...] = (
    "tuning",
    "embedding",
    "aqa",
    "imagen",
    "bison",
    "tts",
    "image",
    "computer",
    "customtools",
    "robotics",
    "latest",
)

# Drop numbered snapshot suffix (-001, -002, etc.) — prefer alias.
_SNAPSHOT_RE = re.compile(r"-\d{3}$")

# In-memory cache keyed by api_key. Value: (expires_at_epoch, model_ids).
_CACHE_TTL_SECONDS = 3600
_CACHE: dict[str, tuple[float, list[str]]] = {}


def _get_cached(key: str) -> list[str] | None:
    entry = _CACHE.get(key)
    if entry is None:
        return None
    expires_at, models = entry
    if time.time() >= expires_at:
        _CACHE.pop(key, None)
        return None
    return models


def _set_cached(key: str, models: list[str]) -> None:
    if not models:
        return
    _CACHE[key] = (time.time() + _CACHE_TTL_SECONDS, models)


def _family_rank(model_id: str) -> int:
    # Newer families ranked first; previews ranked after stable within family.
    for i, prefix in enumerate(("gemini-3.", "gemini-2.5", "gemini-2.")):
        if model_id.startswith(prefix):
            return i * 2 + (1 if "preview" in model_id else 0)
    return 99


class GeminiProvider(LLMProviderBase):
    """
    Provider for Google Gemini API.
    Supports Gemini 2.0, Gemini 1.5 Pro, and other vision-capable models.
    """

    @override
    def __init__(self, config: dict[str, Any]):
        super().__init__(config)
        self.api_key = config.get("api_key")
        self.timeout = config.get("timeout", 300)
        # client will be a google.genai.Client instance when initialized
        self.client = None
        self.rate_limit_hit = 0

        if self.api_key:
            self._initialize_client()

    def _initialize_client(self):
        """Initialize Google Generative AI client"""
        try:
            import google.genai as genai
            from google.genai.types import HttpOptions

            self.client = genai.Client(
                api_key=self.api_key,
                http_options=HttpOptions(timeout=self.timeout * 1000),
            )
            logger.info("Google GenAI client initialized")
        except Exception as e:
            logger.error(f"Failed to initialize Gemini client: {e}")
            self.client = None

    @override
    def is_available(self) -> bool:
        """Check if Gemini API is configured"""
        return self.client is not None and bool(self.api_key)

    @override
    def generate_metadata(
        self, request: MetadataGenerationRequest
    ) -> MetadataGenerationResponse:
        """
        Generate metadata using Gemini API.

        Args:
            request: MetadataGenerationRequest with image and options

        Returns:
            MetadataGenerationResponse with generated metadata
        """
        if request.api_key:
            # Re-initialize client with provided API key
            self.api_key = request.api_key
            self._initialize_client()
            if not self.is_available():
                return MetadataGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error="Gemini API not configured with provided API key",
                )
            else:
                logger.info("Gemini client initialized with request API key")
        else:
            return MetadataGenerationResponse(
                uuid=request.uuid, success=False, error="Gemini API not configured"
            )

        try:
            # Prepare prompts
            system_instruction = self._prepare_system_prompt(request)
            user_prompt = self._prepare_user_prompt(request)

            # Prepare generation config
            generation_config = self._prepare_gemini_generation_config(request)

            model_name = request.model

            # Use the new client-based API for generation
            from google.genai import types

            # Prepare thinking config for certain models
            thinking_config = None
            if model_name == "gemini-2.5-pro":
                thinking_config = types.ThinkingConfig(thinking_budget=128)
            elif (
                model_name == "gemini-2.5-flash"
                or model_name == "gemini-2.5-flash-lite"
            ):
                thinking_config = types.ThinkingConfig(thinking_budget=0)
            elif model_name == "gemini-3-pro-preview":
                thinking_config = types.ThinkingConfig(thinking_level="low")

            # Build a typed GenerateContentConfig from our generation_config dict
            config = types.GenerateContentConfig(
                system_instruction=system_instruction,
                response_mime_type=generation_config.get("response_mime_type"),
                response_schema=generation_config.get("response_schema"),
                temperature=generation_config.get("temperature"),
                max_output_tokens=request.max_tokens or DEFAULT_MAX_TOKENS,
                thinking_config=thinking_config if thinking_config else None,
            )

            logger.info(
                f"Sending metadata request to Gemini: {model_name} (timeout: {self.timeout}s)"
            )
            contents = [
                user_prompt,
                types.Part.from_bytes(data=request.image_data, mime_type="image/jpeg"),
            ]
            response = self.client.models.generate_content(
                model=model_name,
                contents=contents,
                config=config,
            )
            logger.debug("Gemini metadata response received")

            # Check for truncation
            _candidates = getattr(response, "candidates", None)
            if _candidates:
                _finish_reason = str(getattr(_candidates[0], "finish_reason", "") or "")
                if "MAX_TOKENS" in _finish_reason:
                    _max_tokens = request.max_tokens or DEFAULT_MAX_TOKENS
                    raise ValueError(
                        f"Gemini stopped before finishing the response because the token "
                        f"limit was reached (max_output_tokens={_max_tokens}). Please raise "
                        f"the Max Tokens setting in the plugin (General tab → AI Model section) "
                        f"— try 4096 or higher. If you use hierarchical keywords, a large "
                        f"taxonomy increases token usage significantly."
                    )

            # Check for prompt feedback (blocking)
            if hasattr(response, "prompt_feedback") and getattr(
                response.prompt_feedback, "block_reason", None
            ):
                error_msg = (
                    f"Gemini blocked request: {response.prompt_feedback.block_reason}"
                )
                logger.error(error_msg)
                usage_metadata = (
                    getattr(response, "usage", None)
                    or getattr(response, "metadata", None)
                    or getattr(response, "usage_metadata", None)
                )
                return MetadataGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error=error_msg,
                    input_tokens=getattr(usage_metadata, "prompt_token_count", 0)
                    if usage_metadata
                    else 0,
                    output_tokens=getattr(usage_metadata, "candidates_token_count", 0)
                    if usage_metadata
                    else 0,
                )

            # Extract text from response (support text, parsed, or parts)
            if not getattr(response, "text", None):
                parsed = getattr(response, "parsed", None)
                if parsed:
                    text = json.dumps(parsed) if not isinstance(parsed, str) else parsed
                else:
                    parts = getattr(response, "parts", None) or getattr(
                        response, "candidates", None
                    )
                    if parts:
                        collected = []
                        for p in parts:
                            if hasattr(p, "text") and p.text:
                                collected.append(p.text)
                            elif hasattr(p, "content") and isinstance(p.content, str):
                                collected.append(p.content)
                        text = "\n".join(collected)
                    else:
                        error_msg = "Gemini returned no usable text in response"
                        logger.error(error_msg)
                        return MetadataGenerationResponse(
                            uuid=request.uuid, success=False, error=error_msg
                        )
            else:
                text = response.text

            # Clean Gemini-specific artifacts
            text = self._clean_gemini_response(text)

            # Parse JSON
            parsed_data = json.loads(text)

            # Extract metadata
            keywords = self._normalize_keywords_structure(
                parsed_data.get("keywords", [])
            )
            # logger.debug(f"Extracted keywords: {keywords} .. type: {type(keywords)}")

            caption = parsed_data.get("caption") if request.generate_caption else None
            title = parsed_data.get("title") if request.generate_title else None
            alt_text = (
                parsed_data.get("alt_text") if request.generate_alt_text else None
            )

            # Token usage
            usage_metadata = (
                getattr(response, "usage", None)
                or getattr(response, "metadata", None)
                or getattr(response, "usage_metadata", None)
            )
            input_tokens = (
                getattr(usage_metadata, "prompt_token_count", None)
                or getattr(usage_metadata, "input_tokens", None)
                or getattr(usage_metadata, "input_token_count", 0)
            )
            output_tokens = (
                getattr(usage_metadata, "candidates_token_count", None)
                or getattr(usage_metadata, "output_tokens", None)
                or getattr(usage_metadata, "output_token_count", 0)
            )

            # Reset rate limit counter on success
            self.rate_limit_hit = 0

            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=True,
                keywords=keywords,
                caption=caption,
                title=title,
                alt_text=alt_text,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
            )

        except json.JSONDecodeError as e:
            logger.error(f"Failed to parse JSON from Gemini response: {e}")
            return MetadataGenerationResponse(
                uuid=request.uuid, success=False, error=f"JSON parsing error: {str(e)}"
            )
        except Exception as e:
            error_str = str(e)
            error_type = type(e).__name__

            # Handle DeadlineExceeded (504 errors)
            if "DeadlineExceeded" in error_type or "504" in error_str:
                logger.warning(
                    "Gemini API deadline exceeded (timeout) for metadata generation"
                )
                return MetadataGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error="Gemini API timeout (504 Deadline Exceeded). Try again later or use a different provider.",
                )

            # Handle rate limiting
            if (
                "429" in error_str
                or "RATE_LIMIT" in error_str
                or "quota" in error_str.lower()
            ):
                self.rate_limit_hit += 1
                logger.warning(f"Gemini rate limit hit {self.rate_limit_hit} times")

                if self.rate_limit_hit >= 10:
                    return MetadataGenerationResponse(
                        uuid=request.uuid,
                        success=False,
                        error="Rate limit exhausted after 10 retries",
                    )

                # Wait and retry
                time.sleep(5)
                return self.generate_metadata(request)

            logger.exception(f"Error generating metadata with Gemini: {e}")
            return MetadataGenerationResponse(
                uuid=request.uuid, success=False, error=str(e)
            )

    @override
    def generate_edit_recipe(
        self, request: EditGenerationRequest
    ) -> EditGenerationResponse:
        if request.api_key:
            self.api_key = request.api_key
            self._initialize_client()
            if not self.is_available():
                return EditGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error="Gemini API not configured with provided API key",
                )
        else:
            return EditGenerationResponse(
                uuid=request.uuid, success=False, error="Gemini API not configured"
            )

        try:
            system_instruction = self._prepare_edit_system_prompt(request)
            user_prompt = self._prepare_edit_user_prompt(request)
            generation_config = self._prepare_gemini_edit_generation_config(request)
            model_name = request.model

            from google.genai import types

            thinking_config = None
            if model_name == "gemini-2.5-pro":
                thinking_config = types.ThinkingConfig(thinking_budget=128)
            elif (
                model_name == "gemini-2.5-flash"
                or model_name == "gemini-2.5-flash-lite"
            ):
                thinking_config = types.ThinkingConfig(thinking_budget=0)
            elif model_name == "gemini-3-pro-preview":
                thinking_config = types.ThinkingConfig(thinking_level="low")

            config = types.GenerateContentConfig(
                system_instruction=system_instruction,
                response_mime_type=generation_config.get("response_mime_type"),
                response_schema=generation_config.get("response_schema"),
                temperature=generation_config.get("temperature"),
                max_output_tokens=request.max_tokens or DEFAULT_MAX_TOKENS,
                thinking_config=thinking_config if thinking_config else None,
            )

            response = self.client.models.generate_content(
                model=model_name,
                contents=[
                    user_prompt,
                    types.Part.from_bytes(
                        data=request.image_data, mime_type="image/jpeg"
                    ),
                ],
                config=config,
            )

            _candidates = getattr(response, "candidates", None)
            if _candidates:
                _finish_reason = str(getattr(_candidates[0], "finish_reason", "") or "")
                if "MAX_TOKENS" in _finish_reason:
                    _max_tokens = request.max_tokens or DEFAULT_MAX_TOKENS
                    raise ValueError(
                        f"Gemini stopped before finishing the response because the token "
                        f"limit was reached (max_output_tokens={_max_tokens}). Please raise "
                        f"the Max Tokens setting in the plugin (General tab → AI Model section) "
                        f"— try 4096 or higher."
                    )

            if not getattr(response, "text", None):
                parsed = getattr(response, "parsed", None)
                if parsed:
                    text = json.dumps(parsed) if not isinstance(parsed, str) else parsed
                else:
                    raise ValueError("Gemini returned no usable text in response")
            else:
                text = response.text

            parsed_data = json.loads(self._clean_gemini_response(text))
            recipe = self._normalize_edit_recipe(parsed_data)
            usage_metadata = (
                getattr(response, "usage", None)
                or getattr(response, "metadata", None)
                or getattr(response, "usage_metadata", None)
            )
            input_tokens = (
                getattr(usage_metadata, "prompt_token_count", None)
                or getattr(usage_metadata, "input_tokens", None)
                or getattr(usage_metadata, "input_token_count", 0)
            )
            output_tokens = (
                getattr(usage_metadata, "candidates_token_count", None)
                or getattr(usage_metadata, "output_tokens", None)
                or getattr(usage_metadata, "output_token_count", 0)
            )
            self.rate_limit_hit = 0
            return EditGenerationResponse(
                uuid=request.uuid,
                success=True,
                recipe=recipe,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
            )
        except json.JSONDecodeError as e:
            logger.error(f"Failed to parse edit JSON from Gemini response: {e}")
            return EditGenerationResponse(
                uuid=request.uuid, success=False, error=f"JSON parsing error: {str(e)}"
            )
        except Exception as e:
            error_str = str(e)
            error_type = type(e).__name__
            if "DeadlineExceeded" in error_type or "504" in error_str:
                return EditGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error="Gemini API timeout (504 Deadline Exceeded). Try again later or use a different provider.",
                )
            if (
                "429" in error_str
                or "RATE_LIMIT" in error_str
                or "quota" in error_str.lower()
            ):
                self.rate_limit_hit += 1
                if self.rate_limit_hit >= 10:
                    return EditGenerationResponse(
                        uuid=request.uuid,
                        success=False,
                        error="Rate limit exhausted after 10 retries",
                    )
                time.sleep(5)
                return self.generate_edit_recipe(request)
            logger.exception(f"Error generating edit recipe with Gemini: {e}")
            return EditGenerationResponse(
                uuid=request.uuid, success=False, error=str(e)
            )

    def _prepare_gemini_generation_config(
        self, request: MetadataGenerationRequest
    ) -> dict[str, Any]:
        """Prepare Gemini-specific generation config"""
        schema = self._prepare_gemini_response_schema(request)

        return {
            "response_mime_type": "application/json",
            "response_schema": schema,
            "temperature": request.temperature,
        }

    def _prepare_gemini_edit_generation_config(
        self, request: EditGenerationRequest
    ) -> dict[str, Any]:
        return {
            "response_mime_type": "application/json",
            "response_schema": GEMINI_EDIT_RECIPE_SCHEMA,
            "temperature": request.temperature,
        }

    def _prepare_gemini_response_schema(
        self, request: MetadataGenerationRequest
    ) -> dict[str, Any]:
        """Prepare Gemini-style response schema (uses different format than OpenAI)"""
        schema = {
            "type": "OBJECT",  # Gemini uses uppercase
            "properties": {},
        }

        if request.generate_title:
            schema["properties"]["title"] = {"type": "STRING"}

        if request.generate_caption:
            schema["properties"]["caption"] = {"type": "STRING"}

        if request.generate_alt_text:
            schema["properties"]["alt_text"] = {"type": "STRING"}

        if request.generate_keywords:
            if request.keyword_categories:
                # Structured keywords (handles both flat and nested)
                if isinstance(request.keyword_categories, dict):
                    # Nested structure - recursively build Gemini schema
                    keywords_schema = self._build_nested_gemini_keyword_schema(
                        request.keyword_categories,
                        request.bilingual_keywords,
                        request.generate_aliases,
                    )
                else:
                    # Flat list
                    keywords_schema = {"type": "OBJECT", "properties": {}}
                    for category in request.keyword_categories:
                        keywords_schema["properties"][category] = {
                            "type": "ARRAY",
                            "items": self._gemini_keyword_leaf_item_schema(
                                request.bilingual_keywords,
                                request.generate_aliases,
                            ),
                        }
                schema["properties"]["keywords"] = keywords_schema
            else:
                # Simple array
                schema["properties"]["keywords"] = {
                    "type": "ARRAY",
                    "items": self._gemini_keyword_leaf_item_schema(
                        request.bilingual_keywords, request.generate_aliases
                    ),
                }

        return schema

    def _build_nested_gemini_keyword_schema(
        self,
        categories: dict[str, Any],
        bilingual: bool = False,
        aliases: bool = False,
    ) -> dict[str, Any]:
        """
        Recursively build Gemini JSON schema for nested keyword categories.

        Args:
            categories: Dict where keys are category names and values are sub-dicts

        Returns:
            Gemini-format JSON schema for nested structure
        """
        schema = {"type": "OBJECT", "properties": {}}

        for category_name, subcategories in categories.items():
            if isinstance(subcategories, dict) and len(subcategories) > 0:
                # Nested structure - recursively build
                schema["properties"][category_name] = (
                    self._build_nested_gemini_keyword_schema(
                        subcategories, bilingual, aliases
                    )
                )
            else:
                # Leaf node - array of keywords
                schema["properties"][category_name] = {
                    "type": "ARRAY",
                    "items": self._gemini_keyword_leaf_item_schema(bilingual, aliases),
                }

        return schema

    def _gemini_keyword_leaf_item_schema(
        self, bilingual: bool, aliases: bool = False
    ) -> dict[str, Any]:
        if not bilingual and not aliases:
            return {"type": "STRING"}

        properties: dict[str, Any] = {"name": {"type": "STRING"}}
        required = ["name"]
        if aliases:
            properties["aliases"] = {"type": "ARRAY", "items": {"type": "STRING"}}
            required.append("aliases")
        if bilingual:
            properties["synonyms"] = {"type": "ARRAY", "items": {"type": "STRING"}}
            required.append("synonyms")
            if aliases:
                properties["synonym_aliases"] = {
                    "type": "ARRAY",
                    "items": {"type": "STRING"},
                }
                required.append("synonym_aliases")

        return {
            "type": "OBJECT",
            "properties": properties,
            "required": required,
        }

    def _clean_gemini_response(self, text: str) -> str:
        """Clean Gemini-specific response artifacts"""
        # Remove markdown code blocks
        if text.startswith("```json"):
            text = text[7:]
        if text.startswith("```"):
            text = text[3:]
        if text.endswith("```"):
            text = text[:-3]

        # Trim whitespace
        text = text.strip()

        return text

    @override
    def list_available_models(self) -> list[str]:
        """
        List vision-capable Gemini models by querying the ListModels endpoint
        and applying an allowlist (gemini- families that support generateContent)
        + blocklist (embeddings, imagen, bison, tuning, snapshots, 1.0).

        Returns an empty list if no API key is configured or the API call fails.
        Results are cached in-memory for 1 hour, keyed by api_key.
        """
        if not self.api_key:
            return []

        cached = _get_cached(self.api_key)
        if cached is not None:
            logger.debug(f"Gemini model list cache hit ({len(cached)} models)")
            return cached

        if self.client is None:
            self._initialize_client()
        if self.client is None:
            return []

        try:
            raw = list(self.client.models.list())
        except Exception as e:
            logger.warning(f"Failed to list Gemini models: {e}", exc_info=True)
            return []

        candidates: dict[str, str] = {}  # stem -> chosen id
        for model in raw:
            actions = getattr(model, "supported_actions", None) or []
            if "generateContent" not in actions:
                continue

            full_name = getattr(model, "name", "") or ""
            model_id = full_name.removeprefix("models/")

            if not model_id.startswith(_ALLOWED_PREFIX):
                continue
            if model_id.startswith(_BLOCKED_PREFIXES):
                continue
            if any(sub in model_id for sub in _BLOCKED_SUBSTRINGS):
                continue
            if _SNAPSHOT_RE.search(model_id):
                continue

            # Collapse `<stem>` vs `<stem>-latest` to the shorter form.
            stem = model_id.removesuffix("-latest")
            existing = candidates.get(stem)
            if existing is None or len(model_id) < len(existing):
                candidates[stem] = model_id

        filtered = sorted(candidates.values(), key=lambda m: (_family_rank(m), m))

        logger.info(
            f"Listed {len(filtered)} Gemini vision models from API "
            f"(filtered from {len(raw)} total)"
        )
        _set_cached(self.api_key, filtered)
        return filtered
