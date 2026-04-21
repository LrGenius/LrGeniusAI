from abc import ABC, abstractmethod
from typing import Dict, List, Optional, Any, Union
from dataclasses import dataclass
import base64
from PIL import Image
import io


# Import prompts from config
from config import METADATA_GENERATION_SYSTEM_PROMPT
from edit_recipe import OPENAI_EDIT_RECIPE_SCHEMA, normalize_edit_recipe


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
    bilingual_keywords: bool = False
    keyword_secondary_language: Optional[str] = None

    # Provider-specific overrides (e.g. Ollama/LM Studio on remote host)
    ollama_base_url: Optional[str] = None
    lmstudio_base_url: Optional[str] = None


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
    warning: Optional[str] = None


@dataclass
class EditGenerationRequest:
    """Request structure for Lightroom edit recipe generation."""

    image_data: bytes
    uuid: str

    provider: str
    model: str
    api_key: Optional[str]

    language: str
    temperature: float
    max_tokens: Optional[int]

    system_prompt: Optional[str]
    user_prompt: Optional[str]

    submit_gps: bool
    submit_keywords: bool
    submit_folder_names: bool

    existing_keywords: Optional[List[str]]
    gps_coordinates: Optional[Dict[str, float]]
    folder_names: Optional[str]
    user_context: Optional[str]
    date_time: Optional[str]
    edit_intent: Optional[str] = None
    style_strength: float = 0.5
    include_masks: bool = True
    adjust_white_balance: bool = True
    adjust_basic_tone: bool = True
    adjust_presence: bool = True
    adjust_color_mix: bool = True
    do_color_grading: bool = True
    use_tone_curve: bool = True
    use_point_curve: bool = True
    adjust_detail: bool = True
    adjust_effects: bool = True
    adjust_lens_corrections: bool = True
    allow_auto_crop: bool = True
    composition_mode: str = "subtle"
    ollama_base_url: Optional[str] = None
    lmstudio_base_url: Optional[str] = None
    training_examples: Optional[List[Dict[str, Any]]] = None


@dataclass
class EditGenerationResponse:
    """Structured Lightroom edit recipe response."""

    uuid: str
    success: bool
    recipe: Optional[Dict[str, Any]] = None
    input_tokens: int = 0
    output_tokens: int = 0
    error: Optional[str] = None
    warning: Optional[str] = None


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
    def generate_metadata(
        self, request: MetadataGenerationRequest
    ) -> MetadataGenerationResponse:
        """
        Generate metadata for a single image.

        Args:
            request: MetadataGenerationRequest containing image and generation options

        Returns:
            MetadataGenerationResponse with generated metadata or error
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
    def generate_edit_recipe(
        self, request: EditGenerationRequest
    ) -> EditGenerationResponse:
        """
        Generate a Lightroom edit recipe for a single image.
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
            base_prompt = (
                "Analyze the uploaded photo and generate the following data:\n"
            )

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

        if request.submit_gps and isinstance(request.gps_coordinates, dict):
            lat = request.gps_coordinates.get("latitude")
            lon = request.gps_coordinates.get("longitude")
            if lat is not None and lon is not None:
                context_additions.append(
                    f"This photo was taken at the following coordinates: {lat}, {lon}"
                )

        if request.submit_keywords and request.existing_keywords:
            # Must be a list; if still a string, split so join() doesn't iterate over characters (issue #45).
            kw_list = request.existing_keywords
            if isinstance(kw_list, str):
                kw_list = [k.strip() for k in kw_list.split(",") if k.strip()]
            if isinstance(kw_list, list):
                keywords_str = ", ".join(
                    str(k).strip() for k in kw_list if str(k).strip()
                )
                if keywords_str:
                    context_additions.append(f"Some keywords are: {keywords_str}")

        if request.user_context and str(request.user_context).strip() != "":
            context_additions.append(f"Context: {request.user_context}")

        if request.submit_folder_names and request.folder_names:
            # Check if folder names contain alphabetic characters (ignore pure numbers or special chars)
            if any(c.isalpha() for c in request.folder_names):
                context_additions.append(f"Folders: {request.folder_names}")

        if request.date_time and str(request.date_time).strip() != "":
            context_additions.append(f"Capture Time: {request.date_time}")

        # Add keyword hierarchy information if provided
        if request.generate_keywords and request.keyword_categories:
            if isinstance(request.keyword_categories, dict):
                # Nested structure - provide instructions on how to use it
                categories_list = self._flatten_keyword_categories(
                    request.keyword_categories
                )
                categories_str = ", ".join(categories_list)
                context_additions.append(
                    f"Please organize keywords into these categories: {categories_str}. Use the hierarchical structure to organize keywords logically."
                )
            else:
                # Flat list
                categories_str = ", ".join(request.keyword_categories)
                context_additions.append(
                    f"Please organize keywords into these categories: {categories_str}"
                )

        if request.generate_keywords and request.bilingual_keywords:
            secondary_language = (
                request.keyword_secondary_language or "English"
            ).strip() or "English"
            if secondary_language.lower() != request.language.lower():
                context_additions.append(
                    "For keywords only, return each keyword as an object with fields "
                    "`name` (in "
                    + request.language
                    + ") and `synonyms` (array in "
                    + secondary_language
                    + "). Include only true language equivalents; avoid duplicates and inflected-only variants."
                )
            else:
                context_additions.append(
                    "For keywords, return each keyword as an object with fields `name` and `synonyms`. "
                    "Use `synonyms` only for meaningful alternate terms and avoid duplicates."
                )

        # Append context if any
        if context_additions:
            base_prompt += "\n\n" + "\n".join(context_additions)

        return base_prompt

    def _prepare_edit_system_prompt(self, request: EditGenerationRequest) -> str:
        if request.system_prompt:
            return request.system_prompt

        return (
            "You are a senior Lightroom Classic retoucher producing high-end, client-ready edits. "
            "Return only a structured Lightroom edit recipe that strictly matches the provided JSON schema. "
            "Never output prose instructions, markdown, or fields not present in the schema. "
            "Prioritize natural color science, tonal separation, and believable micro-contrast unless an explicit stylized intent is given. "
            "Use the minimum number of controls needed for a strong result; avoid noisy over-adjustment. "
            "When local edits are useful, use only supported mask kinds: subject, sky, background."
        )

    def _format_training_example(self, idx: int, example: Dict[str, Any]) -> str:
        """Serialise one training example into a compact prompt-friendly string."""
        dev = example.get("develop_settings", {})
        label = example.get("label") or example.get("filename") or f"Example {idx}"
        summary = example.get("summary")

        # Keep only numeric develop values to avoid cluttering the prompt.
        CANONICAL_KEYS = {
            "Exposure2012",
            "Contrast2012",
            "Highlights2012",
            "Shadows2012",
            "Whites2012",
            "Blacks2012",
            "Temp",
            "Tint",
            "Texture",
            "Clarity2012",
            "Dehaze",
            "Vibrance",
            "Saturation",
            "Sharpness",
            "LuminanceSmoothing",
            "ColorNoiseReduction",
            "PostCropVignetteAmount",
            "GrainAmount",
            "SplitToningShadowHue",
            "SplitToningShadowSaturation",
            "SplitToningHighlightHue",
            "SplitToningHighlightSaturation",
            "SplitToningBalance",
            "ParametricHighlights",
            "ParametricLights",
            "ParametricDarks",
            "ParametricShadows",
        }
        compact = {
            k: round(v, 2) if isinstance(v, float) else v
            for k, v in dev.items()
            if k in CANONICAL_KEYS and isinstance(v, (int, float))
        }
        lines = [f"  [{idx}] {label}"]
        if summary:
            lines.append(f"      Summary: {summary}")
        if compact:
            params = ", ".join(f"{k}={v}" for k, v in sorted(compact.items()))
            lines.append(f"      Settings: {params}")
        else:
            lines.append("      Settings: (no numeric develop settings captured)")
        return "\n".join(lines)

    def _prepare_edit_user_prompt(self, request: EditGenerationRequest) -> str:
        if request.user_prompt:
            base_prompt = request.user_prompt
        else:
            base_prompt = (
                "Analyze the uploaded photo and return a Lightroom edit recipe.\n"
                "* Add a concise summary of the intended look\n"
                "* Put broad corrections in `global`\n"
                "* Put local corrections in `masks` only when they produce clear benefit\n"
                "* Keep the result natural and premium unless the context explicitly asks for stylization\n"
                "* Do not include unchanged controls"
            )

        base_prompt += (
            "\n\nEdit recipe rules:\n"
            "* Return only numeric Lightroom-friendly adjustments\n"
            "* Build edits in this order: white balance and exposure foundation -> tonal shaping -> color refinement -> detail/effects\n"
            "* For white balance use global `temperature` and `tint` (or `white_balance.temperature` / `white_balance.tint`)\n"
            "* Use global controls first; add masks only when global edits cannot solve the problem cleanly\n"
            "* Use masks only for subject, sky, or background\n"
            "* Keep saturation and clarity moderate; avoid brittle or crunchy output\n"
            "* Prefer highlight recovery and shadow shaping before aggressive contrast\n"
            "* If a curve-shaped tone response is needed (e.g. subtle S-curve, matte blacks, gentle roll-off), prefer `tone_curve.point_curve` and/or `tone_curve.extended_point_curve` over faking it with only contrast sliders\n"
            "* When using point curves, provide valid point pairs per channel in ascending x order and keep endpoints anchored near black/white unless a deliberate fade is requested\n"
            "* Use advanced controls (vignette sub-controls, sharpen detail/masking, noise detail, color NR detail/smoothness) only when clearly justified by image content\n"
            "* Use `lens_corrections` and `crop` only when they clearly improve the result\n"
            "* Add warnings when something seems uncertain or unsupported\n"
        )

        if not request.include_masks:
            base_prompt += "* Do not return any masks; keep all edits global\n"
        if not request.adjust_white_balance:
            base_prompt += "* Do not adjust white balance (`temperature`, `tint`, `white_balance`)\n"
        if not request.adjust_basic_tone:
            base_prompt += "* Do not adjust global basic tone controls (`exposure`, `contrast`, `highlights`, `shadows`, `whites`, `blacks`)\n"
        if not request.adjust_presence:
            base_prompt += (
                "* Do not adjust presence controls (`texture`, `clarity`, `dehaze`)\n"
            )
        if not request.adjust_color_mix:
            base_prompt += (
                "* Do not adjust color mix controls (`vibrance`, `saturation`, `hsl`)\n"
            )
        if not request.do_color_grading:
            base_prompt += "* Do not use `color_grading`\n"
        if not request.use_tone_curve:
            base_prompt += (
                "* Do not use `tone_curve` (neither parametric nor point curve)\n"
            )
        elif not request.use_point_curve:
            base_prompt += "* Do not use `tone_curve.point_curve` or `tone_curve.extended_point_curve`; use only parametric tone curve sliders if needed\n"
        if not request.adjust_detail:
            base_prompt += (
                "* Do not adjust detail controls (sharpening/noise reduction)\n"
            )
        if not request.adjust_effects:
            base_prompt += "* Do not adjust effects controls (vignette/grain)\n"
        if not request.adjust_lens_corrections:
            base_prompt += "* Do not use `lens_corrections`\n"
        if not request.allow_auto_crop:
            base_prompt += "* Do not use `crop`\n"
        else:
            composition_mode = str(request.composition_mode or "subtle").lower()
            if composition_mode == "none":
                base_prompt += "* Do not use `crop`\n"
            elif composition_mode == "subtle":
                base_prompt += "* If using `crop`, keep it subtle: preserve overall framing and avoid aggressive trims\n"
            elif composition_mode == "aggressive":
                base_prompt += "* Crop may be assertive when composition clearly improves; keep key subjects and avoid awkward cutoffs\n"

        context_additions: List[str] = []
        if request.edit_intent:
            context_additions.append(f"Requested editing intent: {request.edit_intent}")
        strength = request.style_strength
        if strength is None:
            strength = 0.5
        try:
            strength = float(strength)
        except (TypeError, ValueError):
            strength = 0.5
        if strength < 0.0:
            strength = 0.0
        if strength > 1.0:
            strength = 1.0
        if strength <= 0.25:
            context_additions.append(
                "Style strength: very subtle (minimal slider movement, preserve original character)."
            )
        elif strength <= 0.5:
            context_additions.append(
                "Style strength: subtle to moderate (clean refinement, avoid strong stylization)."
            )
        elif strength <= 0.75:
            context_additions.append(
                "Style strength: moderate to strong (noticeable look while staying plausible)."
            )
        else:
            context_additions.append(
                "Style strength: strong (bold look allowed, but avoid clipping and artifacts)."
            )
        if request.user_context:
            context_additions.append(f"Per-photo instructions: {request.user_context}")
        if request.submit_keywords and request.existing_keywords:
            keywords_str = ", ".join(
                str(k).strip() for k in request.existing_keywords if str(k).strip()
            )
            if keywords_str:
                context_additions.append(f"Existing keywords: {keywords_str}")
        if request.submit_folder_names and request.folder_names:
            context_additions.append(f"Folder context: {request.folder_names}")
        if request.submit_gps and isinstance(request.gps_coordinates, dict):
            lat = request.gps_coordinates.get("latitude")
            lon = request.gps_coordinates.get("longitude")
            if lat is not None and lon is not None:
                context_additions.append(f"Photo coordinates: {lat}, {lon}")
        if request.date_time:
            context_additions.append(f"Capture time: {request.date_time}")
        if request.language:
            context_additions.append(
                f"Write `summary` and `warnings` in {request.language}, but keep field names exactly as specified by the schema."
            )

        if context_additions:
            base_prompt += "\n\n" + "\n".join(context_additions)

        # Inject few-shot training examples from the user's own edits.
        examples = request.training_examples
        if examples and isinstance(examples, list) and len(examples) > 0:
            base_prompt += "\n\n--- YOUR PERSONAL EDIT STYLE (few-shot examples) ---\n"
            base_prompt += (
                "The following examples are from your own Lightroom edits on visually similar photos. "
                "Study the slider values and replicate this editing style for the current photo.\n"
            )
            for i, ex in enumerate(examples, start=1):
                base_prompt += self._format_training_example(i, ex) + "\n"
            base_prompt += "--- END OF STYLE EXAMPLES ---\n"

        return base_prompt

    def _build_nested_keyword_schema(
        self, categories: Dict[str, Any], bilingual: bool = False
    ) -> Dict[str, Any]:
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
            "required": [],
        }

        for category_name, subcategories in categories.items():
            if isinstance(subcategories, dict) and len(subcategories) > 0:
                # Nested structure - recursively build
                schema["properties"][category_name] = self._build_nested_keyword_schema(
                    subcategories, bilingual
                )
            else:
                # Leaf node - array of keywords
                schema["properties"][category_name] = {
                    "type": "array",
                    "items": self._keyword_leaf_item_schema(
                        request_bilingual=bilingual
                    ),
                }
            if category_name not in schema["required"]:
                schema["required"].append(category_name)

        return schema

    def _keyword_leaf_item_schema(self, request_bilingual: bool) -> Dict[str, Any]:
        if not request_bilingual:
            return {"type": "string"}

        return {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "synonyms": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["name", "synonyms"],
            "additionalProperties": False,
        }

    def _flatten_keyword_categories(
        self, categories: Union[List[str], Dict[str, Any]]
    ) -> List[str]:
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

    def _prepare_response_structure(
        self, request: MetadataGenerationRequest
    ) -> Dict[str, Any]:
        """
        Prepare JSON schema for structured output.
        Different providers have different formats (OpenAI vs Gemini).
        Must be overridden by specific providers.
        """
        schema = {"type": "object", "properties": {}, "required": []}

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
                    keywords_schema = self._build_nested_keyword_schema(
                        request.keyword_categories, request.bilingual_keywords
                    )
                else:
                    # Flat list
                    keywords_schema = {
                        "type": "object",
                        "properties": {},
                        "additionalProperties": False,
                        "required": [],
                    }
                    for category in request.keyword_categories:
                        keywords_schema["properties"][category] = {
                            "type": "array",
                            "items": self._keyword_leaf_item_schema(
                                request.bilingual_keywords
                            ),
                        }
                        if category not in keywords_schema["required"]:
                            keywords_schema["required"].append(category)
                schema["properties"]["keywords"] = keywords_schema
            else:
                # Simple keyword array
                schema["properties"]["keywords"] = {
                    "type": "array",
                    "items": self._keyword_leaf_item_schema(request.bilingual_keywords),
                }
            schema["required"].append("keywords")

        return schema

    def _normalize_keyword_leaf(
        self, value: Any
    ) -> Optional[Union[str, Dict[str, Any]]]:
        if isinstance(value, str):
            keyword = value.strip()
            return keyword or None
        if isinstance(value, dict):
            keyword_name = value.get("name")
            if not isinstance(keyword_name, str):
                return None
            keyword_name = keyword_name.strip()
            if not keyword_name:
                return None
            normalized = {"name": keyword_name}
            synonyms = value.get("synonyms")
            if isinstance(synonyms, list):
                cleaned_synonyms: List[str] = []
                seen = set()
                for synonym in synonyms:
                    if not isinstance(synonym, str):
                        continue
                    synonym_text = synonym.strip()
                    if not synonym_text:
                        continue
                    lowered = synonym_text.lower()
                    if lowered == keyword_name.lower() or lowered in seen:
                        continue
                    seen.add(lowered)
                    cleaned_synonyms.append(synonym_text)
                if cleaned_synonyms:
                    normalized["synonyms"] = cleaned_synonyms
            return normalized
        return None

    def _normalize_keywords_structure(self, value: Any) -> Any:
        if isinstance(value, list):
            normalized_list: List[Any] = []
            for item in value:
                normalized_leaf = self._normalize_keyword_leaf(item)
                if normalized_leaf is not None:
                    normalized_list.append(normalized_leaf)
                elif isinstance(item, (dict, list)):
                    nested = self._normalize_keywords_structure(item)
                    if nested not in (None, {}, []):
                        normalized_list.append(nested)
            return normalized_list

        if isinstance(value, dict):
            if "name" in value and isinstance(value.get("name"), str):
                return self._normalize_keyword_leaf(value)

            normalized_dict: Dict[str, Any] = {}
            for key, item in value.items():
                normalized_item = self._normalize_keywords_structure(item)
                if normalized_item in (None, {}, []):
                    continue
                normalized_dict[key] = normalized_item
            return normalized_dict

        normalized_leaf = self._normalize_keyword_leaf(value)
        return normalized_leaf

    def _prepare_edit_response_structure(self) -> Dict[str, Any]:
        return OPENAI_EDIT_RECIPE_SCHEMA

    def _normalize_edit_recipe(self, value: Any) -> Dict[str, Any]:
        return normalize_edit_recipe(value)

    def _image_to_base64(self, image_data: bytes) -> str:
        """
        Convert image bytes to base64 string.
        Skips re-encoding if image is already JPEG to preserve quality and save CPU.
        """
        try:
            # Optimization: Check for JPEG magic numbers (FF D8 FF)
            # If it's already JPEG, skip the expensive PIL load/save cycle
            if image_data.startswith(b"\xff\xd8\xff"):
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
