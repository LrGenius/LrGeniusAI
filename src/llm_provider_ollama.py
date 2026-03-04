"""
Ollama Provider for metadata generation using the official Ollama Python SDK
"""
import json
from typing import Dict, Any, Optional

try:
    from ollama import Client  # type: ignore
except Exception:  # ImportError or runtime issues
    Client = None  # type: ignore

from llm_provider_base import (
    LLMProviderBase,
    MetadataGenerationRequest,
    MetadataGenerationResponse,
    QualityScoreRequest,
    QualityScoreResponse
)
from config import logger, OLLAMA_BASE_URL


class OllamaProvider(LLMProviderBase):
    """
    Provider for Ollama local inference.
    Uses Ollama's chat completion API with vision models.
    """
    
    def __init__(self, config: Dict[str, Any]):
        super().__init__(config)
        self.base_url = config.get('base_url', OLLAMA_BASE_URL)
        self.timeout = config.get('timeout', 120)
        # Initialize Ollama client targeting the configured host
        try:
            self.client = Client(host=self.base_url) if Client else None
        except Exception as e:
            # Defer failures to is_available/generate methods
            logger.warning(f"Failed to initialize Ollama client: {e}")
            self.client = None  # type: ignore[assignment]
    
    def is_available(self) -> bool:
        """Check if Ollama server is reachable"""
        try:
            if Client is None:
                logger.warning("Ollama SDK not installed. Please install 'ollama' Python package.")
                return False
            if self.client is None:
                self.client = Client(host=self.base_url)
            # A lightweight call to verify connectivity
            _ = self.client.list()
            return True
        except Exception as e:
            logger.warning(f"Ollama not available: {e}")
            return False
    
    def _get_client(self, base_url_override: Optional[str] = None):
        """Get Ollama client, using base_url_override when provided (e.g. from request)."""
        url = base_url_override or self.base_url
        return Client(host=url) if Client else None

    def generate_metadata(self, request: MetadataGenerationRequest) -> MetadataGenerationResponse:
        """
        Generate metadata using Ollama API.
        
        Args:
            request: MetadataGenerationRequest with image and options
            
        Returns:
            MetadataGenerationResponse with generated metadata
        """
        try:
            if Client is None:
                return MetadataGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error="Ollama SDK not installed. Please install the 'ollama' Python package.",
                )
            client = self._get_client(getattr(request, 'ollama_base_url', None))

            # Convert image to base64
            image_b64 = self._image_to_base64(request.image_data)

            # Prepare prompts and JSON schema
            system_prompt = self._prepare_system_prompt(request)
            user_prompt = self._prepare_user_prompt(request)
            response_schema = self._prepare_response_structure(request)

            model_to_use = request.model
            logger.info(f"[Ollama] Using model: {model_to_use}")

            # Call Ollama via Python SDK
            logger.debug("Sending chat request to Ollama via SDK")
            result = client.chat(
                model=model_to_use,
                messages=[
                    {"role": "system", "content": system_prompt},
                    {"role": "user", "content": user_prompt, "images": [image_b64]},
                ],
                format=response_schema,
                options={
                    "temperature": request.temperature,
                    "top_p": 0.9,
                    "num_keep": -1,
                },
                stream=False,
            )

            # Extract message content (supports dict or typed SDK objects)
            if isinstance(result, dict):
                message = result.get("message") or {}
                content = message.get("content")
            else:
                message = getattr(result, "message", None)
                content = getattr(message, "content", None) if message is not None else None
            if not content:
                error_msg = "Empty response content from Ollama"
                logger.error(error_msg)
                return MetadataGenerationResponse(uuid=request.uuid, success=False, error=error_msg)

            logger.debug(f"Ollama raw response: {content}")

            # Parse JSON (Ollama returns JSON string in content)
            parsed_data = json.loads(content)

            # Extract metadata
            keywords = parsed_data.get("keywords", [])
            caption = parsed_data.get("caption") if request.generate_caption else None
            title = parsed_data.get("title") if request.generate_title else None
            alt_text = parsed_data.get("alt_text") if request.generate_alt_text else None

            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=True,
                keywords=keywords,
                caption=caption,
                title=title,
                alt_text=alt_text,
                input_tokens=0,  # Ollama SDK doesn't provide token counts
                output_tokens=0,
            )

        except json.JSONDecodeError as e:
            logger.error(f"Failed to parse JSON from Ollama response: {e}")
            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=False,
                error=f"JSON parsing error: {str(e)}",
            )
        except Exception as e:
            logger.error(f"Error generating metadata with Ollama: {e}", exc_info=True)
            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=False,
                error=str(e),
            )

    def generate_quality_scores(self, request: QualityScoreRequest) -> QualityScoreResponse:
        """
        Generate quality scores using Ollama.
        
        Args:
            request: QualityScoreRequest with image
            
        Returns:
            QualityScoreResponse with quality scores
        """
        
        try:
            if Client is None:
                return QualityScoreResponse(
                    uuid=request.uuid,
                    success=False,
                    error="Ollama SDK not installed. Please install the 'ollama' Python package.",
                )
            client = self._get_client(getattr(request, 'ollama_base_url', None))

            # Convert image to base64
            image_b64 = self._image_to_base64(request.image_data)

            # Prepare prompts and response schema
            system_prompt = self._prepare_quality_system_prompt(request)
            user_prompt = self._prepare_quality_user_prompt(request)

            response_schema = {
                "type": "object",
                "properties": {
                    "overall_score": {"type": "number"},
                    "composition_score": {"type": "number"},
                    "lighting_score": {"type": "number"},
                    "motiv_score": {"type": "number"},
                    "colors_score": {"type": "number"},
                    "emotion_score": {"type": "number"},
                    "critique": {"type": "string"},
                },
            }

            model_to_use = request.model
            logger.info(f"[Ollama] Using model for quality: {model_to_use}")

            result = client.chat(
                model=model_to_use,
                messages=[
                    {"role": "system", "content": system_prompt},
                    {"role": "user", "content": user_prompt, "images": [image_b64]},
                ],
                format=response_schema,
                options={"temperature": request.temperature, "top_p": 0.8},
                stream=False,
            )

            if isinstance(result, dict):
                message = result.get("message") or {}
                content = message.get("content")
            else:
                message = getattr(result, "message", None)
                content = getattr(message, "content", None) if message is not None else None
            if not content:
                error_msg = "Empty response content from Ollama"
                logger.error(error_msg)
                return QualityScoreResponse(uuid=request.uuid, success=False, error=error_msg)

            parsed_data = json.loads(content)

            return QualityScoreResponse(
                uuid=request.uuid,
                success=True,
                overall_score=float(parsed_data.get("overall_score", 0)),
                composition_score=float(parsed_data.get("composition_score", 0)),
                lighting_score=float(parsed_data.get("lighting_score", 0)),
                motiv_score=float(parsed_data.get("motiv_score", 0)),
                colors_score=float(parsed_data.get("colors_score", 0)),
                emotion_score=float(parsed_data.get("emotion_score", 0)),
                critique=parsed_data.get("critique", ""),
            )

        except Exception as e:
            logger.error(f"Error generating quality scores with Ollama: {e}", exc_info=True)
            return QualityScoreResponse(uuid=request.uuid, success=False, error=str(e))
    
    def list_available_models(self) -> list:
        """
        List available Ollama models using Ollama API.
        
        Args:
            only_multimodal: If True, return only vision-capable models
            
        Returns:
            List of model identifiers
        """
        if not self.is_available():
            logger.warning("Ollama not available for listing models")
            return []

        try:
            if self.client is None:
                self.client = Client(host=self.base_url)

            data = self.client.list()
            logger.debug(f"Ollama models response: {data}")

            # Support dict response or typed response with attribute `.models`
            if isinstance(data, dict):
                models = data.get("models", [])
            else:
                models = getattr(data, "models", []) or []

            names = []
            for m in models:
                if isinstance(m, dict):
                    name = m.get("name") or m.get("model") or m.get("tag")
                else:
                    name = (
                        getattr(m, "name", None)
                        or getattr(m, "model", None)
                        or getattr(m, "tag", None)
                    )
                if name:
                    names.append(name)
            return names

        except Exception as e:
            logger.error(f"Error listing Ollama models: {e}", exc_info=True)
            return []