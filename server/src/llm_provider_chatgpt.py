"""
ChatGPT/OpenAI Provider for metadata generation using OpenAI API
"""
import json
from typing import Dict, Any
from llm_provider_base import LLMProviderBase, MetadataGenerationRequest, MetadataGenerationResponse, QualityScoreRequest, QualityScoreResponse
from config import logger


class ChatGPTProvider(LLMProviderBase):
    """
    Provider for OpenAI ChatGPT API.
    Supports GPT-4o, GPT-4-turbo, and other vision-capable models.
    """
    
    def __init__(self, config: Dict[str, Any]):
        super().__init__(config)
        self.api_key = config.get('api_key')
        self.timeout = config.get('timeout', 120)
        self.client = None
        
        if self.api_key:
            self._initialize_client()
    
    def _initialize_client(self):
        """Initialize OpenAI client"""
        try:
            from openai import OpenAI
            self.client = OpenAI(
                api_key=self.api_key,
                timeout=self.timeout
            )
            logger.info("OpenAI client initialized")
        except Exception as e:
            logger.error(f"Failed to initialize OpenAI client: {e}")
            self.client = None
    
    def is_available(self) -> bool:
        """Check if OpenAI API is configured"""
        return self.client is not None and bool(self.api_key)
    
    def generate_metadata(self, request: MetadataGenerationRequest) -> MetadataGenerationResponse:
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
                        error="OpenAI API initialization failed with provided API key"
                    )
                else:
                    logger.info("OpenAI client initialized with provided API key for metadata generation")
            else:
                return MetadataGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error="OpenAI API not configured"
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
            temperature = 1.0 if request.model.startswith("gpt-5") else request.temperature
            
            # Prepare messages
            messages = [
                {
                    "role": "system",
                    "content": system_prompt
                },
                {
                    "role": "user",
                    "content": user_prompt
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": data_uri
                            }
                        }
                    ]
                }
            ]
            
            # Make API call
            logger.debug(f"Sending request to OpenAI: {request.model}")
            
            completion_params = {
                "model": request.model,
                "messages": messages,
                "response_format": response_format,
                "temperature": temperature,
            }
            
            # GPT-5 models require reasoning_effort
            if request.model.startswith("gpt-5"):
                completion_params["reasoning_effort"] = "low"
            
            response = self.client.chat.completions.create(**completion_params)
            
            # Check finish reason
            choice = response.choices[0]
            if choice.finish_reason != "stop":
                error_msg = f"OpenAI generation failed: {choice.finish_reason}"
                logger.error(error_msg)
                return MetadataGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error=error_msg,
                    input_tokens=response.usage.prompt_tokens if response.usage else 0,
                    output_tokens=response.usage.completion_tokens if response.usage else 0
                )
            
            # Extract message content
            content = choice.message.content
            logger.debug(f"OpenAI raw response: {content}")
            
            # Parse JSON
            parsed_data = json.loads(content)
            
            # Extract metadata
            keywords = parsed_data.get("keywords", [])
            
            caption = parsed_data.get("caption") if request.generate_caption else None
            title = parsed_data.get("title") if request.generate_title else None
            alt_text = parsed_data.get("alt_text") if request.generate_alt_text else None
            
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
                output_tokens=output_tokens
            )
            
        except json.JSONDecodeError as e:
            logger.error(f"Failed to parse JSON from OpenAI response: {e}")
            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=False,
                error=f"JSON parsing error: {str(e)}"
            )
        except Exception as e:
            logger.error(f"Error generating metadata with OpenAI: {e}", exc_info=True)
            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=False,
                error=str(e)
            )
    
    def generate_quality_scores(self, request: QualityScoreRequest) -> QualityScoreResponse:
        """
        Generate quality scores using ChatGPT API.
        
        Args:
            request: QualityScoreRequest with image
            
        Returns:
            QualityScoreResponse with quality scores and critique
        """
        
        if not self.is_available():
            if request.api_key:
                # Try to initialize client with provided API key
                self.api_key = request.api_key
                self._initialize_client()
                if not self.is_available():
                    return QualityScoreResponse(
                        uuid=request.uuid,
                        success=False,
                        error="OpenAI API initialization failed with provided API key"
                    )
                else:
                    logger.info("OpenAI client initialized with provided API key for quality scoring")
            else:
                return QualityScoreResponse(
                    uuid=request.uuid,
                    success=False,
                    error="OpenAI API not configured"
                )

        try:
            # Convert image to base64 data URI
            image_b64 = self._image_to_base64(request.image_data)
            data_uri = f"data:image/jpeg;base64,{image_b64}"
            
            # Prepare quality scoring prompts using base class methods
            system_prompt = self._prepare_quality_system_prompt(request)
            user_prompt = self._prepare_quality_user_prompt(request)
            
            # Prepare OpenAI response schema
            quality_schema = {
                "type": "object",
                "properties": {
                    "overall_score": {"type": "number"},
                    "composition_score": {"type": "number"},
                    "lighting_score": {"type": "number"},
                    "motiv_score": {"type": "number"},
                    "colors_score": {"type": "number"},
                    "emotion_score": {"type": "number"},
                    "critique": {"type": "string"}
                },
                "required": ["overall_score", "composition_score", "lighting_score", "motiv_score", "colors_score", "emotion_score", "critique"],
                "additionalProperties": False
            }
            
            response_format = {
                "type": "json_schema",
                "json_schema": {
                    "name": "quality_scores",
                    "schema": quality_schema,
                    "strict": True
                }
            }
            
            # Handle GPT-5 models
            temperature = 1.0 if request.model.startswith("gpt-5") else request.temperature
            
            # Prepare messages
            messages = [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {"url": data_uri}
                        }
                    ]
                }
            ]
            
            # Make API call
            logger.debug(f"Sending quality scoring request to OpenAI: {request.model}")
            
            completion_params = {
                "model": request.model,
                "messages": messages,
                "response_format": response_format,
                "temperature": temperature,
            }
            
            if request.model.startswith("gpt-5"):
                completion_params["reasoning_effort"] = "low"
            
            response = self.client.chat.completions.create(**completion_params)
            
            # Check finish reason
            choice = response.choices[0]
            if choice.finish_reason != "stop":
                error_msg = f"Generation failed: {choice.finish_reason}"
                logger.error(error_msg)
                return QualityScoreResponse(
                    uuid=request.uuid,
                    success=False,
                    error=error_msg,
                    input_tokens=response.usage.prompt_tokens if response.usage else 0,
                    output_tokens=response.usage.completion_tokens if response.usage else 0
                )
            
            # Parse response
            content = choice.message.content
            logger.debug(f"OpenAI quality response: {content}")
            
            parsed_data = json.loads(content)
            
            # Extract scores
            input_tokens = response.usage.prompt_tokens if response.usage else 0
            output_tokens = response.usage.completion_tokens if response.usage else 0
            
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
                input_tokens=input_tokens,
                output_tokens=output_tokens
            )
            
        except json.JSONDecodeError as e:
            logger.error(f"Failed to parse JSON from OpenAI quality response: {e}")
            return QualityScoreResponse(uuid=request.uuid, success=False, error=f"JSON parsing error: {str(e)}")
        except Exception as e:
            logger.error(f"Error generating quality scores with OpenAI: {e}", exc_info=True)
            return QualityScoreResponse(uuid=request.uuid, success=False, error=str(e))
    
    def _prepare_openai_response_format(self, request: MetadataGenerationRequest) -> Dict[str, Any]:
        """Prepare OpenAI-style response format with JSON schema"""
        schema = self._prepare_response_structure(request)
        schema["additionalProperties"] = False
        
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
        List available OpenAI models.
        
        Args:
            only_multimodal: If True, return only vision-capable models
            
        Returns:
            List of model identifiers
        """
        # Return hardcoded list even if API key is not configured
        # This allows users to see which models are available
        # The actual API key will be provided when making requests
        
        # Hardcoded list of vision-capable ChatGPT models
        # SDK-based filtering commented out as it returns too many irrelevant models
        vision_models = [
            'gpt-4.1',
            'gpt-4.1-mini',
            'gpt-5',
            'gpt-5-mini',
            'gpt-5-nano',
            'gpt-5.1',
            'gpt-5.2',
        ]
        
        logger.info(f"Returning {len(vision_models)} hardcoded ChatGPT models")
        return vision_models