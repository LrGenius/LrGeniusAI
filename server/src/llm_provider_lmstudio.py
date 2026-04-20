"""
LM Studio Provider for metadata generation using the lmstudio-python library
"""
import json
import lmstudio as lms
from typing import Dict, Any
from llm_provider_base import (
    LLMProviderBase,
    EditGenerationRequest,
    EditGenerationResponse,
    MetadataGenerationRequest,
    MetadataGenerationResponse,
)
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
        # lmstudio-python's synchronous API defaults to timing out after ~60s of
        # inactivity when waiting for a response/stream event. Wire our configured
        # timeout through so metadata generation can run longer (e.g. 720s).
        #
        # Note: this timeout is global to the lmstudio-python sync API.
        try:
            set_sync_timeout = getattr(lms, "set_sync_api_timeout", None)
            if callable(set_sync_timeout):
                set_sync_timeout(self.timeout)
                logger.info(f"LM Studio sync API timeout set to {self.timeout}s")
            else:
                logger.debug("lmstudio-python set_sync_api_timeout not available; using SDK default timeout")
        except Exception as e:
            logger.warning(f"Failed to set lmstudio-python sync API timeout: {e}", exc_info=True)


    def is_available(self) -> bool:
        """Check if LM Studio server is reachable with a short timeout"""
        try:
            # First, a basic validation of host format
            if not self.host or ":" not in self.host:
                return False
                
            # Use the SDK's validation but be aware it might block if the host is a dead IP.
            # In a future version, we might add a socket-level pre-check here.
            return lms.Client.is_valid_api_host(self.host)
        except Exception as e:
            logger.warning(f"LM Studio availability check failed for {self.host}: {e}")
            return False
    
    def generate_metadata(self, request: MetadataGenerationRequest) -> MetadataGenerationResponse:
        """
        Generate metadata using LM Studio API.
        
        Args:
            request: MetadataGenerationRequest with image and options
            
        Returns:
            MetadataGenerationResponse with generated metadata
        """
        try:
            # Resolve host: request override -> provider default
            host = getattr(request, "lmstudio_base_url", None) or self.host

            # Use a scoped client for this host instead of global default client
            with lms.Client(host) as client:
                # Prepare image via client so we don't depend on the default client
                image_handle = client.files.prepare_image(request.image_data)
                model = client.llm.model(request.model)
                
                # Prepare prompts
                system_prompt = self._prepare_system_prompt(request)
                user_prompt = self._prepare_user_prompt(request)
                
                # Prepare OpenAI-style response format
                response_schema = self._prepare_response_structure(request)
                
                # Make request to LM Studio
                logger.debug("Sending request to LM Studio")

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
            keywords = self._normalize_keywords_structure(content.get("keywords", []))
            
            caption = content.get("caption") if request.generate_caption else None
            title = content.get("title") if request.generate_title else None
            alt_text = content.get("alt_text") if request.generate_alt_text else None
         
            # Token usage reporting
            input_tokens = 0
            output_tokens = 0
            try:
                # 1. Try to get usage from the response object directly (lms 0.4.x+)
                stats = getattr(response, "stats", None) or getattr(response, "usage", None)
                if stats:
                    input_tokens = getattr(stats, "prompt_tokens", 0) or getattr(stats, "input_tokens", 0)
                    output_tokens = getattr(stats, "completion_tokens", 0) or getattr(stats, "output_tokens", 0)
                
                # 2. Fallback: Manual tokenization for accuracy
                if input_tokens == 0 and hasattr(model, "tokenize"):
                    # For input, we should tokenize the full prompt as seen by the model
                    try:
                        # model.apply_prompt_template(chat) returns the raw string if available
                        full_prompt = model.apply_prompt_template(chat) if hasattr(model, "apply_prompt_template") else user_prompt
                        input_tokens = len(model.tokenize(full_prompt))
                    except Exception:
                        input_tokens = len(model.tokenize(user_prompt))
                
                if output_tokens == 0 and hasattr(model, "tokenize") and isinstance(content, dict):
                    output_tokens = len(model.tokenize(json.dumps(content)))
                
                logger.info(f"LM Studio token usage for {request.uuid}: input={input_tokens}, output={output_tokens}")
            except Exception as usage_err:
                logger.debug(f"Could not calculate LM Studio token usage: {usage_err}")

            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=True,
                keywords=keywords,
                caption=caption,
                title=title,
                alt_text=alt_text,
                input_tokens=input_tokens,
                output_tokens=output_tokens
            )
            
        except Exception as e:
            logger.error(f"Error generating metadata with LM Studio: {e}", exc_info=True)
            return MetadataGenerationResponse(uuid=request.uuid, success=False, error=str(e))

    def generate_edit_recipe(self, request: EditGenerationRequest) -> EditGenerationResponse:
        try:
            host = getattr(request, "lmstudio_base_url", None) or self.host
            with lms.Client(host) as client:
                image_handle = client.files.prepare_image(request.image_data)
                model = client.llm.model(request.model)
                system_prompt = self._prepare_edit_system_prompt(request)
                user_prompt = self._prepare_edit_user_prompt(request)
                response_schema = self._prepare_openai_edit_response_format()

                chat = lms.Chat(system_prompt)
                chat.add_user_message(user_prompt, images=[image_handle])
                response = model.respond(chat, response_format=response_schema, config={"temperature": request.temperature})

            content = response.parsed
            if isinstance(content, str):
                content = json.loads(content)
            if not isinstance(content, dict):
                raise ValueError(f"Unexpected response type from LM Studio: {type(content)}")

            recipe = self._normalize_edit_recipe(content)
            # Token usage reporting
            input_tokens = 0
            output_tokens = 0
            try:
                stats = getattr(response, "stats", None) or getattr(response, "usage", None)
                if stats:
                    input_tokens = getattr(stats, "prompt_tokens", 0) or getattr(stats, "input_tokens", 0)
                    output_tokens = getattr(stats, "completion_tokens", 0) or getattr(stats, "output_tokens", 0)
                
                if input_tokens == 0 and hasattr(model, "tokenize"):
                    try:
                        full_prompt = model.apply_prompt_template(chat) if hasattr(model, "apply_prompt_template") else user_prompt
                        input_tokens = len(model.tokenize(full_prompt))
                    except Exception:
                        input_tokens = len(model.tokenize(user_prompt))
                
                if output_tokens == 0 and hasattr(model, "tokenize"):
                    output_tokens = len(model.tokenize(json.dumps(content)))
            except Exception:
                pass

            return EditGenerationResponse(
                uuid=request.uuid,
                success=True,
                recipe=recipe,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
            )
        except Exception as e:
            logger.error(f"Error generating edit recipe with LM Studio: {e}", exc_info=True)
            return EditGenerationResponse(uuid=request.uuid, success=False, error=str(e))
    
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

    def _prepare_openai_edit_response_format(self) -> Dict[str, Any]:
        schema = self._prepare_edit_response_structure()
        return {
            "type": "json_schema",
            "json_schema": {
                "name": "lightroom_edit_recipe",
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
        try:
            # Use a scoped client so we respect the configured host and
            # avoid relying on a not-yet-resolved default API port.
            with lms.Client(self.host) as client:
                models = client.llm.list_downloaded()
                all_models = [model.model_key for model in models]
                return all_models
            
        except Exception as e:
            logger.error(f"An unexpected error occurred while listing LM Studio models: {e}", exc_info=True)
            return []
