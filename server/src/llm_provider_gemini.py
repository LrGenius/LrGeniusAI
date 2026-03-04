"""
Gemini Provider for metadata generation using Google Generative AI API
"""
import json
import time
from typing import Dict, Any, Union, List
from llm_provider_base import LLMProviderBase, MetadataGenerationRequest, MetadataGenerationResponse, QualityScoreRequest, QualityScoreResponse
from config import logger

class GeminiProvider(LLMProviderBase):
    """
    Provider for Google Gemini API.
    Supports Gemini 2.0, Gemini 1.5 Pro, and other vision-capable models.
    """
    
    def __init__(self, config: Dict[str, Any]):
        super().__init__(config)
        self.api_key = config.get('api_key')
        self.timeout = config.get('timeout', 300)
        # client will be a google.genai.Client instance when initialized
        self.client = None
        self.rate_limit_hit = 0
        
        if self.api_key:
            self._initialize_client()
    
    def _initialize_client(self):
        """Initialize Google Generative AI client"""
        try:
            # New Google GenAI Python SDK (package name: google-genai) exposes
            # the `genai` module. Use google.genai instead of google.generativeai.
            # The new google-genai SDK exposes a Client API.
            # Create a genai Client instance and store it on the provider.
            import google.genai as genai
            self.client = genai.Client(api_key=self.api_key)

            logger.info("Google GenAI client initialized")
        except Exception as e:
            logger.error(f"Failed to initialize Gemini client: {e}")
            self.client = None
    
    def is_available(self) -> bool:
        """Check if Gemini API is configured"""
        return self.client is not None and bool(self.api_key)
    
    def generate_metadata(self, request: MetadataGenerationRequest) -> MetadataGenerationResponse:
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
                    error="Gemini API not configured with provided API key"
                )
            else:
                logger.info("Gemini client initialized with request API key")
        else:
            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=False,
                error="Gemini API not configured"
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
                thinking_config=types.ThinkingConfig(thinking_budget=128)
            elif model_name == "gemini-2.5-flash" or model_name == "gemini-2.5-flash-lite":
                thinking_config=types.ThinkingConfig(thinking_budget=0)
            elif model_name == "gemini-3-pro-preview":
                thinking_config=types.ThinkingConfig(thinking_level="low")


            # Build a typed GenerateContentConfig from our generation_config dict
            config = types.GenerateContentConfig(
                response_mime_type=generation_config.get("response_mime_type"),
                response_schema=generation_config.get("response_schema"),
                temperature=generation_config.get("temperature"),
                thinking_config=thinking_config if thinking_config else None,
            )

            logger.info(f"Sending metadata request to Gemini: {model_name} (timeout: {self.timeout}s)")
            try:
                # contents may include the user prompt and an image part
                contents = [user_prompt, types.Part.from_bytes(data=request.image_data, mime_type='image/jpeg')]
                response = self.client.models.generate_content(
                    model=model_name,
                    contents=contents,
                    config=config,
                )
                logger.debug("Gemini metadata response received")
            except TimeoutError as te:
                error_msg = f"Gemini request timed out after {self.timeout}s"
                logger.warning(error_msg)
                return MetadataGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error=error_msg
                )
            except Exception as api_error:
                # Catch API errors early and re-raise for main handler
                if "DeadlineExceeded" in str(type(api_error).__name__) or "504" in str(api_error):
                    logger.warning(f"Gemini API deadline exceeded: {api_error}")
                raise  # Re-raise to be caught by the main exception handler

            # Check for prompt feedback (blocking)
            if hasattr(response, 'prompt_feedback') and getattr(response.prompt_feedback, 'block_reason', None):
                error_msg = f"Gemini blocked request: {response.prompt_feedback.block_reason}"
                logger.error(error_msg)
                usage_metadata = getattr(response, 'usage', None) or getattr(response, 'metadata', None) or getattr(response, 'usage_metadata', None)
                return MetadataGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error=error_msg,
                    input_tokens=getattr(usage_metadata, 'prompt_token_count', 0) if usage_metadata else 0,
                    output_tokens=getattr(usage_metadata, 'candidates_token_count', 0) if usage_metadata else 0
                )
            
            # Extract text from response (support text, parsed, or parts)
            if not getattr(response, 'text', None):
                parsed = getattr(response, 'parsed', None)
                if parsed:
                    text = json.dumps(parsed) if not isinstance(parsed, str) else parsed
                else:
                    parts = getattr(response, 'parts', None) or getattr(response, 'candidates', None)
                    if parts:
                        collected = []
                        for p in parts:
                            if hasattr(p, 'text') and p.text:
                                collected.append(p.text)
                            elif hasattr(p, 'content') and isinstance(p.content, str):
                                collected.append(p.content)
                        text = '\n'.join(collected)
                    else:
                        error_msg = "Gemini returned no usable text in response"
                        logger.error(error_msg)
                        return MetadataGenerationResponse(uuid=request.uuid, success=False, error=error_msg)
            else:
                text = response.text

            # Clean Gemini-specific artifacts
            text = self._clean_gemini_response(text)
            
            # Parse JSON
            parsed_data = json.loads(text)
            
            # Extract metadata
            keywords = parsed_data.get("keywords", [])
            # logger.debug(f"Extracted keywords: {keywords} .. type: {type(keywords)}")
            
            caption = parsed_data.get("caption") if request.generate_caption else None
            title = parsed_data.get("title") if request.generate_title else None
            alt_text = parsed_data.get("alt_text") if request.generate_alt_text else None
            
            # Token usage
            usage_metadata = getattr(response, 'usage', None) or getattr(response, 'metadata', None) or getattr(response, 'usage_metadata', None)
            input_tokens = getattr(usage_metadata, 'prompt_token_count', None) or getattr(usage_metadata, 'input_tokens', None) or getattr(usage_metadata, 'input_token_count', 0)
            output_tokens = getattr(usage_metadata, 'candidates_token_count', None) or getattr(usage_metadata, 'output_tokens', None) or getattr(usage_metadata, 'output_token_count', 0)
            
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
                output_tokens=output_tokens
            )
            
        except json.JSONDecodeError as e:
            logger.error(f"Failed to parse JSON from Gemini response: {e}")
            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=False,
                error=f"JSON parsing error: {str(e)}"
            )
        except Exception as e:
            error_str = str(e)
            error_type = type(e).__name__
            
            # Handle DeadlineExceeded (504 errors)
            if "DeadlineExceeded" in error_type or "504" in error_str:
                logger.warning(f"Gemini API deadline exceeded (timeout) for metadata generation")
                return MetadataGenerationResponse(
                    uuid=request.uuid,
                    success=False,
                    error=f"Gemini API timeout (504 Deadline Exceeded). Try again later or use a different provider."
                )
            
            # Handle rate limiting
            if "429" in error_str or "RATE_LIMIT" in error_str or "quota" in error_str.lower():
                self.rate_limit_hit += 1
                logger.warning(f"Gemini rate limit hit {self.rate_limit_hit} times")
                
                if self.rate_limit_hit >= 10:
                    return MetadataGenerationResponse(
                        uuid=request.uuid,
                        success=False,
                        error="Rate limit exhausted after 10 retries"
                    )
                
                # Wait and retry
                time.sleep(5)
                return self.generate_metadata(request)
            
            logger.error(f"Error generating metadata with Gemini: {e}", exc_info=True)
            return MetadataGenerationResponse(
                uuid=request.uuid,
                success=False,
                error=str(e)
            )
    
    def generate_quality_scores(self, request: QualityScoreRequest) -> QualityScoreResponse:
        """
        Generate quality scores using Gemini API.
        
        Args:
            request: QualityScoreRequest with image
            
        Returns:
            QualityScoreResponse with quality scores and critique
        """
        
        if not self.is_available():
            if request.api_key:
                # Re-initialize client with provided API key
                self.api_key = request.api_key
                self._initialize_client()
                if not self.is_available():
                    return QualityScoreResponse(
                        uuid=request.uuid,
                        success=False,
                        error="Gemini API not configured with provided API key"
                    )
                else:
                    logger.info("Gemini client initialized with request API key")
            else:
                return QualityScoreResponse(
                    uuid=request.uuid,
                    success=False,
                    error="Gemini API not configured"
                )
        
        try:
            # Load image
            from PIL import Image
            import io
            logger.debug(f"Loading image for quality scoring (size: {len(request.image_data)} bytes)")
            image = Image.open(io.BytesIO(request.image_data)).convert("RGB")
            logger.debug(f"Image loaded: {image.size[0]}x{image.size[1]} pixels")
            
            # Prepare quality scoring prompts using base class methods
            system_instruction = self._prepare_quality_system_prompt(request)
            user_prompt = self._prepare_quality_user_prompt(request)
            
            # Prepare Gemini response schema for quality scores
            quality_schema = {
                "type": "OBJECT",
                "properties": {
                    "overall_score": {"type": "NUMBER"},
                    "composition_score": {"type": "NUMBER"},
                    "lighting_score": {"type": "NUMBER"},
                    "motiv_score": {"type": "NUMBER"},
                    "colors_score": {"type": "NUMBER"},
                    "emotion_score": {"type": "NUMBER"},
                    "critique": {"type": "STRING"}
                }
            }
            
            generation_config = {
                "response_mime_type": "application/json",
                "response_schema": quality_schema,
                "temperature": request.temperature,
            }
            
            model_name = request.model
            
            # Use the new client-based API for generation
            from google.genai import types

            config = types.GenerateContentConfig(
                response_mime_type=generation_config.get("response_mime_type"),
                response_schema=generation_config.get("response_schema"),
                temperature=generation_config.get("temperature"),
            )

            timeout = self.timeout
            logger.info(f"Sending quality scoring request to Gemini: {model_name} (timeout: {timeout}s)")
            logger.info(f"Request parameters - temperature: {generation_config['temperature']}, language: {request.language}")

            try:
                contents = [user_prompt, types.Part.from_bytes(data=request.image_data, mime_type='image/jpeg')]
                response = self.client.models.generate_content(
                    model=model_name,
                    contents=contents,
                    config=config,
                )
                logger.info(f"✓ Gemini quality scoring response received successfully")
                logger.debug("Gemini quality scoring response received")
            except TimeoutError as te:
                error_msg = f"Gemini request timed out after {self.timeout}s"
                logger.warning(error_msg)
                return QualityScoreResponse(uuid=request.uuid, success=False, error=error_msg)
            except Exception as api_error:
                # Catch API errors early (like DeadlineExceeded) and re-raise for main handler
                if "DeadlineExceeded" in str(type(api_error).__name__) or "504" in str(api_error):
                    logger.warning(f"Gemini API deadline exceeded: {api_error}")
                raise  # Re-raise to be caught by the main exception handler
            
            # Check for blocking
            logger.debug("Checking response for blocking...")
            if hasattr(response, 'prompt_feedback') and getattr(response.prompt_feedback, 'block_reason', None):
                error_msg = f"Gemini blocked request: {response.prompt_feedback.block_reason}"
                logger.error(error_msg)
                return QualityScoreResponse(uuid=request.uuid, success=False, error=error_msg)
            
            # Extract and parse response text
            if not getattr(response, 'text', None):
                parsed = getattr(response, 'parsed', None)
                if parsed:
                    text = json.dumps(parsed) if not isinstance(parsed, str) else parsed
                else:
                    parts = getattr(response, 'parts', None) or getattr(response, 'candidates', None)
                    if parts:
                        collected = []
                        for p in parts:
                            if hasattr(p, 'text') and p.text:
                                collected.append(p.text)
                            elif hasattr(p, 'content') and isinstance(p.content, str):
                                collected.append(p.content)
                        text = '\n'.join(collected)
                    else:
                        return QualityScoreResponse(uuid=request.uuid, success=False, error="No candidates returned")
            else:
                text = response.text

            logger.debug(f"Gemini quality response (length: {len(text)}): {text[:200]}...")

            text = self._clean_gemini_response(text)
            parsed_data = json.loads(text)
            
            # Extract scores
            usage_metadata = getattr(response, 'usage', None) or getattr(response, 'metadata', None) or getattr(response, 'usage_metadata', None)
            
            # Reset rate limit counter on success
            self.rate_limit_hit = 0
            
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
                input_tokens=usage_metadata.prompt_token_count if usage_metadata else 0,
                output_tokens=usage_metadata.candidates_token_count if usage_metadata else 0
            )
            
        except json.JSONDecodeError as e:
            logger.error(f"Failed to parse JSON from Gemini quality response: {e}")
            return QualityScoreResponse(uuid=request.uuid, success=False, error=f"JSON parsing error: {str(e)}")
        except TimeoutError as e:
            error_msg = f"Gemini request timed out: {str(e)}"
            logger.error(error_msg)
            return QualityScoreResponse(uuid=request.uuid, success=False, error=error_msg)
        except Exception as e:
            error_str = str(e)
            error_type = type(e).__name__
            
            # Handle DeadlineExceeded (504 errors) - Gemini API timeout
            if "DeadlineExceeded" in error_type or "504" in error_str:
                logger.warning(f"Gemini API deadline exceeded (timeout). This usually means the API is slow or overloaded.")
                return QualityScoreResponse(
                    uuid=request.uuid, 
                    success=False, 
                    error=f"Gemini API timeout (504 Deadline Exceeded). Try again later or use a different provider."
                )
            
            # Handle timeout-related errors
            if "timeout" in error_str.lower() or "timed out" in error_str.lower():
                error_msg = f"Gemini request timed out: {error_str}"
                logger.error(error_msg)
                return QualityScoreResponse(uuid=request.uuid, success=False, error=error_msg)
            
            # Handle rate limiting
            if "429" in error_str or "RATE_LIMIT" in error_str or "quota" in error_str.lower():
                self.rate_limit_hit += 1
                logger.warning(f"Gemini rate limit hit {self.rate_limit_hit} times")
                
                if self.rate_limit_hit >= 10:
                    return QualityScoreResponse(uuid=request.uuid, success=False, error="Rate limit exhausted")
                
                time.sleep(5)
                return self.generate_quality_scores(request)
            
            logger.error(f"Error generating quality scores with Gemini: {e}", exc_info=True)
            return QualityScoreResponse(uuid=request.uuid, success=False, error=str(e))
    
    def _prepare_gemini_generation_config(self, request: MetadataGenerationRequest) -> Dict[str, Any]:
        """Prepare Gemini-specific generation config"""
        schema = self._prepare_gemini_response_schema(request)
        
        return {
            "response_mime_type": "application/json",
            "response_schema": schema,
            "temperature": request.temperature,
        }
    
    def _prepare_gemini_response_schema(self, request: MetadataGenerationRequest) -> Dict[str, Any]:
        """Prepare Gemini-style response schema (uses different format than OpenAI)"""
        schema = {
            "type": "OBJECT",  # Gemini uses uppercase
            "properties": {}
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
                    keywords_schema = self._build_nested_gemini_keyword_schema(request.keyword_categories)
                else:
                    # Flat list
                    keywords_schema = {
                        "type": "OBJECT",
                        "properties": {}
                    }
                    for category in request.keyword_categories:
                        keywords_schema["properties"][category] = {
                            "type": "ARRAY",
                            "items": {"type": "STRING"}
                        }
                schema["properties"]["keywords"] = keywords_schema
            else:
                # Simple array
                schema["properties"]["keywords"] = {
                    "type": "ARRAY",
                    "items": {"type": "STRING"}
                }
        
        return schema
    
    def _build_nested_gemini_keyword_schema(self, categories: Dict[str, Any]) -> Dict[str, Any]:
        """
        Recursively build Gemini JSON schema for nested keyword categories.
        
        Args:
            categories: Dict where keys are category names and values are sub-dicts
            
        Returns:
            Gemini-format JSON schema for nested structure
        """
        schema = {
            "type": "OBJECT",
            "properties": {}
        }
        
        for category_name, subcategories in categories.items():
            if isinstance(subcategories, dict) and len(subcategories) > 0:
                # Nested structure - recursively build
                schema["properties"][category_name] = self._build_nested_gemini_keyword_schema(subcategories)
            else:
                # Leaf node - array of keywords
                schema["properties"][category_name] = {
                    "type": "ARRAY",
                    "items": {"type": "STRING"}
                }
        
        return schema
    
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
    
    
    def list_available_models(self) -> list:
        """
        List available Gemini models.
        
        Args:
            only_multimodal: If True, return only vision-capable models
            
        Returns:
            List of model identifiers
        """
        # Return hardcoded list even if API key is not configured
        # This allows users to see which models are available
        # The actual API key will be provided when making requests
        
        # Hardcoded list of vision-capable Gemini models
        # SDK-based filtering commented out as it returns too many irrelevant models
        vision_models = [
            'gemini-2.5-flash-lite',
            'gemini-2.5-flash',
            'gemini-2.5-pro',
            'gemini-3-flash-preview',
            'gemini-3-pro-preview',
        ]
        
        logger.info(f"Returning {len(vision_models)} hardcoded Gemini models")
        return vision_models