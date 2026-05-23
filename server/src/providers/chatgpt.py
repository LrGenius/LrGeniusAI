"""
ChatGPT/OpenAI Provider for metadata generation using OpenAI API
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


# Vision-capable family prefixes. Update when a new vision family ships.
_ALLOWED_PREFIXES: tuple[str, ...] = (
    "gpt-4.1",
    "gpt-5",
)

# Drop any model id containing one of these substrings.
_BLOCKED_SUBSTRINGS: tuple[str, ...] = (
    "-instruct",
    "-audio",
    "-realtime",
    "-transcribe",
    "-tts",
    "-search",
    "-moderation",
    "-codex",
    "-chat",
)

# Drop dated snapshot ids: trailing -YYYY-MM-DD or -NNNN (e.g. -0125).
_SNAPSHOT_RE = re.compile(r"-(\d{4}-\d{2}-\d{2}|\d{4})$")

# Explicit excludes: product aliases and known non-vision models.
_EXCLUDED_IDS: frozenset[str] = frozenset({"chatgpt-4o-latest"})
_EXCLUDED_PREFIXES: tuple[str, ...] = ("o1-mini",)

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
    # Lower = sorted first. Newer families ranked higher (= lower number).
    for i, prefix in enumerate(("gpt-5", "gpt-4.1", "gpt-4o", "o4", "o3")):
        if model_id.startswith(prefix):
            return i
    return 99


class ChatGPTProvider(LLMProviderBase):
    """
    Provider for OpenAI ChatGPT API.
    Supports GPT-4o, GPT-4-turbo, and other vision-capable models.
    """

    @override
    def __init__(self, config: dict[str, Any]):
        super().__init__(config)
        self.api_key = config.get("api_key")
        self.timeout = config.get("timeout", 120)
        self.client = None

        if self.api_key:
            self._initialize_client()

    def _initialize_client(self):
        """Initialize OpenAI client"""
        try:
            from openai import OpenAI

            self.client = OpenAI(api_key=self.api_key, timeout=self.timeout)
            logger.info("OpenAI client initialized")
        except Exception as e:
            logger.error(f"Failed to initialize OpenAI client: {e}")
            self.client = None

    @override
    def is_available(self) -> bool:
        """Check if OpenAI API is configured"""
        return self.client is not None and bool(self.api_key)

    @override
    def generate_metadata(
        self, request: MetadataGenerationRequest
    ) -> MetadataGenerationResponse:
        """
        Generate metadata using OpenAI API.

        Args:
            request: MetadataGenerationRequest with image and options

        Returns:
            MetadataGenerationResponse with generated metadata
        """
        if not self.is_available():
            if request.api_key:
                # Try to initialize client with provided API key
                self.api_key = request.api_key
                self._initialize_client()
                if not self.is_available():
                    return MetadataGenerationResponse(
                        uuid=request.uuid,
                        success=False,
                        error="OpenAI API initialization failed with provided API key",
                    )
                else:
                    logger.info(
                        "OpenAI client initialized with provided API key for metadata generation"
                    )
            else:
                return MetadataGenerationResponse(
                    uuid=request.uuid, success=False, error="OpenAI API not configured"
                )

        try:
            # Convert image to base64 data URI
            image_b64 = self._image_to_base64(request.image_data)
            data_uri = f"data:image/jpeg;base64,{image_b64}"

            # Prepare prompts
            system_prompt = self._prepare_system_prompt(request)
            user_prompt = self._prepare_user_prompt(request)

            # Prepare response format
            response_format = self._prepare_openai_response_format(request)

            # Handle GPT-5 models (they don't support temperature)
            temperature = (
                1.0 if request.model.startswith("gpt-5") else request.temperature
            )

            # Prepare messages
            messages = [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
                {
                    "role": "user",
                    "content": [{"type": "image_url", "image_url": {"url": data_uri}}],
                },
            ]

            # Make API call
            logger.debug(f"Sending request to OpenAI: {request.model}")

            completion_params = {
                "model": request.model,
                "messages": messages,
                "response_format": response_format,
                "temperature": temperature,
                "max_tokens": request.max_tokens or DEFAULT_MAX_TOKENS,
            }

            # GPT-5 models require reasoning_effort
            if request.model.startswith("gpt-5"):
                completion_params["reasoning_effort"] = "low"

            response = self.client.chat.completions.create(**completion_params)

            # Check finish reason
            choice = response.choices[0]
            if choice.finish_reason != "stop":
                if choice.finish_reason == "length":
                    error_msg = (
                        f"OpenAI stopped before finishing the response because the token "
                        f"limit was reached (max_tokens={request.max_tokens or DEFAULT_MAX_TOKENS}). "
                        f"Please raise the Max Tokens setting in the plugin "
                        f"(General tab → AI Model section) — try 4096 or higher."
                    )
                else:
                    error_msg = f"OpenAI generation failed: {choice.finish_reason}"
                logger.error(error_msg)
                return MetadataGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error=error_msg,
                    input_tokens=response.usage.prompt_tokens if response.usage else 0,
                    output_tokens=response.usage.completion_tokens
                    if response.usage
                    else 0,
                )

            # Extract message content
            content = choice.message.content
            logger.debug(f"OpenAI raw response: {content}")

            # Parse JSON
            parsed_data = json.loads(content)

            # Extract metadata
            keywords = self._normalize_keywords_structure(
                parsed_data.get("keywords", [])
            )

            caption = parsed_data.get("caption") if request.generate_caption else None
            title = parsed_data.get("title") if request.generate_title else None
            alt_text = (
                parsed_data.get("alt_text") if request.generate_alt_text else None
            )

            # Token usage
            input_tokens = response.usage.prompt_tokens if response.usage else 0
            output_tokens = response.usage.completion_tokens if response.usage else 0

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
            logger.error(f"Failed to parse JSON from OpenAI response: {e}")
            return MetadataGenerationResponse(
                uuid=request.uuid, success=False, error=f"JSON parsing error: {str(e)}"
            )
        except Exception as e:
            logger.error(f"Error generating metadata with OpenAI: {e}", exc_info=True)
            return MetadataGenerationResponse(
                uuid=request.uuid, success=False, error=str(e)
            )

    @override
    def generate_edit_recipe(
        self, request: EditGenerationRequest
    ) -> EditGenerationResponse:
        if not self.is_available():
            if request.api_key:
                self.api_key = request.api_key
                self._initialize_client()
                if not self.is_available():
                    return EditGenerationResponse(
                        uuid=request.uuid,
                        success=False,
                        error="OpenAI API initialization failed with provided API key",
                    )
            else:
                return EditGenerationResponse(
                    uuid=request.uuid, success=False, error="OpenAI API not configured"
                )

        try:
            image_b64 = self._image_to_base64(request.image_data)
            data_uri = f"data:image/jpeg;base64,{image_b64}"
            system_prompt = self._prepare_edit_system_prompt(request)
            user_prompt = self._prepare_edit_user_prompt(request)
            response_format = self._prepare_openai_edit_response_format()
            temperature = (
                1.0 if request.model.startswith("gpt-5") else request.temperature
            )

            messages = [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {"url": data_uri},
                        }
                    ],
                },
            ]
            completion_params = {
                "model": request.model,
                "messages": messages,
                "response_format": response_format,
                "temperature": temperature,
                "max_tokens": request.max_tokens or DEFAULT_MAX_TOKENS,
            }
            if request.model.startswith("gpt-5"):
                completion_params["reasoning_effort"] = "low"

            response = self.client.chat.completions.create(**completion_params)
            choice = response.choices[0]
            if choice.finish_reason != "stop":
                if choice.finish_reason == "length":
                    edit_error_msg = (
                        f"OpenAI stopped before finishing the response because the token "
                        f"limit was reached (max_tokens={request.max_tokens or DEFAULT_MAX_TOKENS}). "
                        f"Please raise the Max Tokens setting in the plugin "
                        f"(General tab → AI Model section) — try 4096 or higher."
                    )
                else:
                    edit_error_msg = f"OpenAI generation failed: {choice.finish_reason}"
                return EditGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error=edit_error_msg,
                    input_tokens=response.usage.prompt_tokens if response.usage else 0,
                    output_tokens=response.usage.completion_tokens
                    if response.usage
                    else 0,
                )

            parsed_data = json.loads(choice.message.content)
            recipe = self._normalize_edit_recipe(parsed_data)
            return EditGenerationResponse(
                uuid=request.uuid,
                success=True,
                recipe=recipe,
                input_tokens=response.usage.prompt_tokens if response.usage else 0,
                output_tokens=response.usage.completion_tokens if response.usage else 0,
            )
        except json.JSONDecodeError as e:
            logger.error(f"Failed to parse edit JSON from OpenAI response: {e}")
            return EditGenerationResponse(
                uuid=request.uuid, success=False, error=f"JSON parsing error: {str(e)}"
            )
        except Exception as e:
            logger.error(
                f"Error generating edit recipe with OpenAI: {e}", exc_info=True
            )
            return EditGenerationResponse(
                uuid=request.uuid, success=False, error=str(e)
            )

    def _prepare_openai_response_format(
        self, request: MetadataGenerationRequest
    ) -> dict[str, Any]:
        """Prepare OpenAI-style response format with JSON schema"""
        schema = self._prepare_response_structure(request)
        # Ensure the schema is strictly compliant with OpenAI requirements
        schema = self._make_schema_strict(schema)

        return {
            "type": "json_schema",
            "json_schema": {
                "name": "metadata_response",
                "schema": schema,
                "strict": True,
            },
        }

    def _prepare_openai_edit_response_format(self) -> dict[str, Any]:
        schema = self._prepare_edit_response_structure()
        # Ensure the schema is strictly compliant with OpenAI requirements
        schema = self._make_schema_strict(schema)

        return {
            "type": "json_schema",
            "json_schema": {
                "name": "lightroom_edit_recipe",
                "schema": schema,
                "strict": True,
            },
        }

    def _make_schema_strict(self, schema: dict[str, Any]) -> dict[str, Any]:
        """
        Recursively modify a JSON schema to be strictly compliant with OpenAI Requirements:
        1. Every object must have additionalProperties: False
        2. Every property defined in 'properties' must be in the 'required' list
        """
        # If it's not a dict, we can't process it as a schema object
        if not isinstance(schema, dict):
            return schema

        schema_type = schema.get("type")

        # Handle objects
        if schema_type == "object" or "properties" in schema:
            schema["type"] = "object"  # Ensure type is set
            schema["additionalProperties"] = False

            properties = schema.get("properties", {})
            if properties:
                # Initialize required list if missing
                if "required" not in schema:
                    schema["required"] = []

                # All properties must be in required
                for prop_name in properties.keys():
                    if prop_name not in schema["required"]:
                        schema["required"].append(prop_name)

                # Recursively process each property
                for prop_name, prop_schema in properties.items():
                    schema["properties"][prop_name] = self._make_schema_strict(
                        prop_schema
                    )

        # Handle arrays
        elif schema_type == "array" or "items" in schema:
            schema["type"] = "array"  # Ensure type is set
            if "items" in schema:
                schema["items"] = self._make_schema_strict(schema["items"])

        return schema

    @override
    def list_available_models(self) -> list[str]:
        """
        List vision-capable OpenAI models by querying /v1/models and applying
        an allowlist (vision families) + blocklist (snapshots, audio/tts/etc.).

        Returns an empty list if no API key is configured or the API call fails.
        Results are cached in-memory for 1 hour, keyed by api_key.
        """
        if not self.api_key:
            return []

        cached = _get_cached(self.api_key)
        if cached is not None:
            logger.debug(f"ChatGPT model list cache hit ({len(cached)} models)")
            return cached

        if self.client is None:
            self._initialize_client()
        if self.client is None:
            return []

        try:
            raw = self.client.models.list()
        except Exception as e:
            logger.warning(f"Failed to list OpenAI models: {e}", exc_info=True)
            return []

        filtered: list[str] = []
        for model in raw:
            model_id = model.id
            if not model_id.startswith(_ALLOWED_PREFIXES):
                continue
            if model_id in _EXCLUDED_IDS:
                continue
            if model_id.startswith(_EXCLUDED_PREFIXES):
                continue
            if any(sub in model_id for sub in _BLOCKED_SUBSTRINGS):
                continue
            if _SNAPSHOT_RE.search(model_id):
                continue
            filtered.append(model_id)

        filtered.sort(key=lambda m: (_family_rank(m), m))

        logger.info(f"Listed {len(filtered)} ChatGPT vision models from API")
        _set_cached(self.api_key, filtered)
        return filtered
