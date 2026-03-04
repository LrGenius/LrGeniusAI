from abc import ABC, abstractmethod
from typing import Dict, List, Optional, Any, Union
from dataclasses import dataclass
import base64
from PIL import Image
import io


# Import prompts from config
from config import (
    METADATA_GENERATION_SYSTEM_PROMPT,
    QUALITY_SCORING_SYSTEM_PROMPT,
    QUALITY_SCORING_USER_PROMPT
)

@dataclass
class MetadataGenerationRequest:
    """Request structure for metadata generation"""
    image_data: bytes
    uuid: str
    
    # Provider selection and model configuration
    provider: str
    model: str
    api_key: Optional[str]
    
    # Generation options (what to generate)
    generate_keywords: bool
    generate_caption: bool
    generate_title: bool
    generate_alt_text: bool
    
    # Output language for all generated metadata
    language: str
    
    # LLM parameters
    temperature: float
    max_tokens: Optional[int]
    
    # System and user prompts (can override defaults)
    system_prompt: Optional[str]
    user_prompt: Optional[str]
    
    # Context flags (whether to include additional context)
    submit_gps: bool
    submit_keywords: bool
    submit_folder_names: bool
    
    # Optional context data
    existing_keywords: Optional[List[str]]
    gps_coordinates: Optional[Dict[str, float]]
    folder_names: Optional[str]
    user_context: Optional[str]
    date_time: Optional[str]
    
    # Keyword hierarchy for structured output
    # Can be either a flat list of strings: ["People", "Activities"]
    # Or a nested dict: {"People": {"Family": {}, "Friends": {}}, "Activities": {}}
    keyword_categories: Optional[Union[List[str], Dict[str, Any]]]

    # Provider-specific overrides (e.g. Ollama on remote host)
    ollama_base_url: Optional[str] = None


@dataclass
class MetadataGenerationResponse:
    """Response structure for metadata generation"""
    uuid: str
    success: bool
    
    # Generated metadata
    keywords: Optional[dict[str, str]] = None
    caption: Optional[str] = None
    title: Optional[str] = None
    alt_text: Optional[str] = None
    
    # Token usage for tracking
    input_tokens: int = 0
    output_tokens: int = 0
    
    # Error information
    error: Optional[str] = None


@dataclass
class QualityScoreRequest:
    """Request structure for quality scoring"""
    image_data: bytes
    uuid: str
    
    # Provider selection and model configuration
    provider: str
    model: str
    api_key: Optional[str]
    
    # Output language for critique
    language: str
    
    # LLM parameters
    temperature: float
    max_tokens: Optional[int]
    
    # System and user prompts (can override defaults)
    system_prompt: Optional[str]
    user_prompt: Optional[str]

    # Provider-specific overrides (e.g. Ollama on remote host)
    ollama_base_url: Optional[str] = None


@dataclass
class QualityScoreResponse:
    """Response structure for quality scoring"""
    uuid: str
    success: bool
    
    # Quality scores (1.0 - 10.0)
    overall_score: Optional[float] = None
    composition_score: Optional[float] = None
    lighting_score: Optional[float] = None
    motiv_score: Optional[float] = None
    colors_score: Optional[float] = None
    emotion_score: Optional[float] = None
    
    # Detailed critique text
    critique: Optional[str] = None
    
    # Token usage for tracking
    input_tokens: int = 0
    output_tokens: int = 0
    
    # Error information
    error: Optional[str] = None


class LLMProviderBase(ABC):
    """
    Abstract base class for all LLM providers.
    Each provider (Qwen, Ollama, LM Studio, ChatGPT, Gemini) implements this interface.
    """
    
    def __init__(self, config: Dict[str, Any]):
        """
        Initialize provider with configuration.
        
        Args:
            config: Provider-specific configuration dictionary
        """
        self.config = config
        self.provider_name = self.__class__.__name__
    
    @abstractmethod
    def generate_metadata(self, request: MetadataGenerationRequest) -> MetadataGenerationResponse:
        """
        Generate metadata for a single image.
        
        Args:
            request: MetadataGenerationRequest containing image and generation options
            
        Returns:
            MetadataGenerationResponse with generated metadata or error
        """
        pass
    
    @abstractmethod
    def generate_quality_scores(self, request: QualityScoreRequest) -> QualityScoreResponse:
        """
        Generate quality scores and critique for a single image.
        
        Args:
            request: QualityScoreRequest containing image and options
            
        Returns:
            QualityScoreResponse with quality scores and critique or error
        """
        pass
    
    @abstractmethod
    def is_available(self) -> bool:
        """
        Check if the provider is available and properly configured.
        
        Returns:
            True if provider can be used, False otherwise
        """
        pass
    
    @abstractmethod
    def list_available_models(self) -> list:
        """
        List all available models for this provider.
        
        Args:
            only_multimodal: If True, return only vision-capable models
            
        Returns:
            List of model names/identifiers
        """
        pass
    
    def _prepare_system_prompt(self, request: MetadataGenerationRequest) -> str:
        """
        Prepare system instruction based on request options.
        Can be overridden by specific providers if needed.
        """
        # Use custom system prompt if provided
        if request.system_prompt:
            return request.system_prompt
        
        # Use default system prompt from config
        return METADATA_GENERATION_SYSTEM_PROMPT
    
    def _prepare_user_prompt(self, request: MetadataGenerationRequest) -> str:
        """
        Prepare user task/prompt based on what metadata to generate.
        Can be overridden by specific providers if needed.
        """
        # Use custom user prompt if provided
        if request.user_prompt:
            base_prompt = request.user_prompt
        else:
            # Default task prompt
            base_prompt = "Analyze the uploaded photo and generate the following data:\n"
            
            if request.generate_alt_text:
                base_prompt += "* Alt text (with context for screen readers)\n"
            
            if request.generate_caption:
                base_prompt += "* Image caption\n"
            
            if request.generate_title:
                base_prompt += "* Image title\n"
            
            if request.generate_keywords:
                base_prompt += "* Keywords\n"
        
        # Add language instruction
        base_prompt += f"\n\nAll results should be generated in {request.language}."
        
        # Add contextual information if provided and enabled
        context_additions = []
        
        if request.submit_gps and request.gps_coordinates:
            lat = request.gps_coordinates.get('latitude')
            lon = request.gps_coordinates.get('longitude')
            if lat is not None and lon is not None:
                context_additions.append(f"This photo was taken at the following coordinates: {lat}, {lon}")
        
        if request.submit_keywords and request.existing_keywords:
            keywords_str = ", ".join(request.existing_keywords)
            context_additions.append(f"Some keywords are: {keywords_str}")
        
        if request.user_context:
            context_additions.append(f"Some context for this photo: {request.user_context}")
        
        if request.submit_folder_names and request.folder_names:
            # Check if folder names contain alphabetic characters (not just numbers/special chars)
            if any(c.isalpha() for c in request.folder_names):
                context_additions.append(f"This photo is located in the following folders: {request.folder_names}")
        
        if request.date_time and request.date_time != "":
            context_additions.append(f"This photo was taken on: {request.date_time}")
        
        # Add keyword hierarchy information if provided
        if request.generate_keywords and request.keyword_categories:
            if isinstance(request.keyword_categories, dict):
                # Nested structure - provide instructions on how to use it
                categories_list = self._flatten_keyword_categories(request.keyword_categories)
                categories_str = ", ".join(categories_list)
                context_additions.append(f"Please organize keywords into these categories: {categories_str}. Use the hierarchical structure to organize keywords logically.")
            else:
                # Flat list
                categories_str = ", ".join(request.keyword_categories)
                context_additions.append(f"Please organize keywords into these categories: {categories_str}")
        
        # Append context if any
        if context_additions:
            base_prompt += "\n\n" + "\n".join(context_additions)
        
        return base_prompt
    
    def _prepare_quality_system_prompt(self, request: QualityScoreRequest) -> str:
        """
        Prepare system prompt for quality scoring.
        Can be overridden by specific providers if needed.
        """
        # Use custom system prompt if provided
        if request.system_prompt:
            return request.system_prompt
        
        # Use default quality scoring system prompt from config
        return QUALITY_SCORING_SYSTEM_PROMPT
    
    def _prepare_quality_user_prompt(self, request: QualityScoreRequest) -> str:
        """
        Prepare user prompt for quality scoring.
        Can be overridden by specific providers if needed.
        """
        # Use custom user prompt if provided
        if request.user_prompt:
            return request.user_prompt
        
        # Use default quality scoring user prompt from config and add language instruction
        prompt = QUALITY_SCORING_USER_PROMPT
        prompt += f'\n\nUse the full 1-10 scale. Be critical and specific about weaknesses. Write the critique in {request.language}.'
        
        return prompt
    
    def _build_nested_keyword_schema(self, categories: Dict[str, Any]) -> Dict[str, Any]:
        """
        Recursively build JSON schema for nested keyword categories.
        
        Args:
            categories: Dict where keys are category names and values are sub-dicts
            
        Returns:
            JSON schema for nested structure
        """
        schema = {
            "type": "object",
            "properties": {},
            "additionalProperties": False,
            "required": []
        }
        
        for category_name, subcategories in categories.items():
            if isinstance(subcategories, dict) and len(subcategories) > 0:
                # Nested structure - recursively build
                schema["properties"][category_name] = self._build_nested_keyword_schema(subcategories)
            else:
                # Leaf node - array of keywords
                schema["properties"][category_name] = {
                    "type": "array",
                    "items": {"type": "string"}
                }
            schema["required"].append(category_name)
        
        return schema
    
    def _flatten_keyword_categories(self, categories: Union[List[str], Dict[str, Any]]) -> List[str]:
        """
        Flatten nested keyword categories to a simple list.
        Used for context in the prompt if needed.
        
        Args:
            categories: Either a flat list or nested dict of categories
            
        Returns:
            Flat list of all category names
        """
        if isinstance(categories, list):
            return categories
        
        result = []
        def traverse(d):
            for key, value in d.items():
                result.append(key)
                if isinstance(value, dict) and len(value) > 0:
                    traverse(value)
        
        traverse(categories)
        return result
    
    def _prepare_response_structure(self, request: MetadataGenerationRequest) -> Dict[str, Any]:
        """
        Prepare JSON schema for structured output.
        Different providers have different formats (OpenAI vs Gemini).
        Must be overridden by specific providers.
        """
        schema = {
            "type": "object",
            "properties": {},
            "required": []
        }
        
        if request.generate_title:
            schema["properties"]["title"] = {"type": "string"}
            schema["required"].append("title")
        
        if request.generate_caption:
            schema["properties"]["caption"] = {"type": "string"}
            schema["required"].append("caption")
        
        if request.generate_alt_text:
            schema["properties"]["alt_text"] = {"type": "string"}
            schema["required"].append("alt_text")
        
        if request.generate_keywords:
            if request.keyword_categories:
                # Structured keywords by category (handles both flat and nested)
                if isinstance(request.keyword_categories, dict):
                    # Nested structure
                    keywords_schema = self._build_nested_keyword_schema(request.keyword_categories)
                else:
                    # Flat list
                    keywords_schema = {
                        "type": "object",
                        "properties": {},
                        "additionalProperties": False,
                        "required": []
                    }
                    for category in request.keyword_categories:
                        keywords_schema["properties"][category] = {
                            "type": "array",
                            "items": {"type": "string"}
                        }
                        keywords_schema["required"].append(category)
                schema["properties"]["keywords"] = keywords_schema
            else:
                # Simple keyword array
                schema["properties"]["keywords"] = {
                    "type": "array",
                    "items": {"type": "string"}
                }
            schema["required"].append("keywords")
        
        return schema
    
    def _image_to_base64(self, image_data: bytes) -> str:
        """
        Convert image bytes to base64 string.
        Skips re-encoding if image is already JPEG to preserve quality and save CPU.
        """
        try:
            # Optimization: Check for JPEG magic numbers (FF D8 FF)
            # If it's already JPEG, skip the expensive PIL load/save cycle
            if image_data.startswith(b'\xff\xd8\xff'):
                return base64.b64encode(image_data).decode("utf-8")

            # For non-JPEGs (PNG, WEBP, etc.), convert to JPEG
            image = Image.open(io.BytesIO(image_data)).convert("RGB")
            
            buffer = io.BytesIO()
            # Keep high quality for conversion
            image.save(buffer, format="JPEG", quality=95) 
            image_bytes = buffer.getvalue()
            
            return base64.b64encode(image_bytes).decode("utf-8")
        except Exception as e:
            raise ValueError(f"Failed to process image: {str(e)}")