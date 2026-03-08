"""
LM Studio Provider for metadata generation using the lmstudio-python library
"""
import json
import lmstudio as lms
from typing import Dict, Any
from llm_provider_base import LLMProviderBase, MetadataGenerationRequest, MetadataGenerationResponse
from config import logger, LMSTUDIO_HOST


class LMStudioProvider(LLMProviderBase):
    """
    Provider for LM Studio local inference.
    Uses the lmstudio-python library.
    """
    
    def __init__(self, config: Dict[str, Any]):
        super().__init__(config)
        self.host = config.get('base_url', LMSTUDIO_HOST)
        self.timeout = config.get('timeout', 720)
        lms.configure_default_client(self.host)


    def is_available(self) -> bool:
        """Check if LM Studio server is reachable"""
        return lms.Client.is_valid_api_host(self.host)
    
    def generate_metadata(self, request: MetadataGenerationRequest) -> MetadataGenerationResponse:
        """
        Generate metadata using LM Studio API.
        
        Args:
            request: MetadataGenerationRequest with image and options
            
        Returns:
            MetadataGenerationResponse with generated metadata
        """
        try:
            # Convert image to base64 data URI
            image_handle = lms.prepare_image(request.image_data)

            model = lms.llm(request.model)
            
            # Prepare prompts
            system_prompt = self._prepare_system_prompt(request)
            user_prompt = self._prepare_user_prompt(request)
            
            # Prepare OpenAI-style response format
            response_schema = self._prepare_response_structure(request)
            
            # Make request to LM Studio
            logger.debug(f"Sending request to LM Studio")

            chat = lms.Chat(system_prompt)
            chat.add_user_message(user_prompt, images=[image_handle])

            response = model.respond(chat, response_format=response_schema, config={"temperature": request.temperature })

            # Extract message content
            content = response.parsed
            logger.debug(f"LM Studio raw response: {content}")

            # The lmstudio-python client may return a JSON string instead of a dict.
            # Normalize to a dict so that `.get(...)` access below is always safe.
            if isinstance(content, str):
                try:
                    content = json.loads(content)
                except Exception as parse_err:
                    raise ValueError(f"Unexpected non-JSON response from LM Studio: {content}") from parse_err

            if not isinstance(content, dict):
                raise ValueError(f"Unexpected response type from LM Studio: {type(content)}")
            
            # Extract metadata
            keywords = content.get("keywords", [])
            
            caption = content.get("caption") if request.generate_caption else None
            title = content.get("title") if request.generate_title else None
            alt_text = content.get("alt_text") if request.generate_alt_text else None
         
            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=True,
                keywords=keywords,
                caption=caption,
                title=title,
                alt_text=alt_text,
                input_tokens=0,
                output_tokens=0
            )
            
        except Exception as e:
            logger.error(f"Error generating metadata with LM Studio: {e}", exc_info=True)
            return MetadataGenerationResponse(uuid=request.uuid, success=False, error=str(e))
    
    def _prepare_openai_response_format(self, request: MetadataGenerationRequest) -> Dict[str, Any]:
        """Prepare OpenAI-style response format with JSON schema"""
        schema = self._prepare_response_structure(request)
        
        return {
            "type": "json_schema",
            "json_schema": {
                "name": "metadata_response",
                "schema": schema,
                "strict": True
            }
        }
    
    def list_available_models(self) -> list:
        """
        List available LM Studio models using the lmstudio-python library.
        
        Returns:
            List of model identifiers for vision-capable models.
        """
        if not self.is_available():
            logger.warning("LM Studio not available for listing models")
            return []
        
        try:
            models = lms.list_downloaded_models("llm")
            all_models = [model.model_key for model in models]
            return all_models
            
        except Exception as e:
            logger.error(f"An unexpected error occurred while listing LM Studio models: {e}", exc_info=True)
            return []
