"""
Service layer for metadata generation across different LLM providers.
Handles provider selection, initialization, and orchestration.
Uses lazy loading - providers are only initialized when needed.
"""
from typing import Dict, List, Optional, Any
from llm_provider_base import (
    LLMProviderBase, 
    EditGenerationRequest,
    EditGenerationResponse,
    MetadataGenerationRequest, 
    MetadataGenerationResponse,
)
from llm_provider_ollama import OllamaProvider
from llm_provider_lmstudio import LMStudioProvider
from llm_provider_chatgpt import ChatGPTProvider
from llm_provider_gemini import GeminiProvider
from edit_recipe import filter_edit_recipe_by_controls
from config import logger, DEFAULT_METADATA_PROVIDER, DEFAULT_METADATA_LANGUAGE
import service_training as training_service
from PIL import Image
import io
import torch
import torch.nn.functional as F
from config import TORCH_DEVICE

class AnalysisService:
    """
    Central service for managing metadata generation across multiple LLM providers.
    Handles provider initialization, selection, and fallback logic.
    Uses lazy loading - providers are created but models loaded only on first use.
    """
    
    def __init__(self, lazy_load=True):
        """
        Initialize the metadata service with all available providers.
        
        Args:
            lazy_load: If True, only check availability. Models are loaded on first use.
        """
        self.providers: Dict[str, LLMProviderBase] = {}
        self.provider_status: Dict[str, str] = {}
        self.provider_errors: Dict[str, str] = {}
        self.lazy_load = lazy_load
        self._initialize_providers()
    
    def _initialize_providers(self):
        """
        Initialize all configured providers with lazy loading.
        Providers are created but heavy models are only loaded on first use.
        
        Note: Ollama and LM Studio availability is NOT checked here - they will be
        dynamically checked when listing models, allowing them to be started after server startup.
        """
        logger.info("Checking available LLM providers (lazy loading enabled)...")
        
        # Ollama (local) - Always register, availability checked dynamically
        try:
            ollama = OllamaProvider({})
            self.providers['ollama'] = ollama
            if ollama.is_available():
                self.provider_status['ollama'] = 'available'
                logger.info("[OK] Ollama provider available")
            else:
                self.provider_status['ollama'] = 'registered'
                logger.info("[INFO] Ollama provider registered (server not running, can be started later)")
        except Exception as e:
            self.provider_status['ollama'] = 'failed'
            self.provider_errors['ollama'] = str(e)
            logger.error(f"[FAIL] Failed to initialize Ollama provider: {e}")
        
        # LM Studio (local) - Always register, availability checked dynamically
        try:
            lmstudio = LMStudioProvider({})
            self.providers['lmstudio'] = lmstudio
            if lmstudio.is_available():
                self.provider_status['lmstudio'] = 'available'
                logger.info("[OK] LM Studio provider initialized")
            else:
                self.provider_status['lmstudio'] = 'registered'
                logger.info("[INFO] LM Studio provider registered (server not running, can be started later)")
        except Exception as e:
            self.provider_status['lmstudio'] = 'failed'
            self.provider_errors['lmstudio'] = str(e)
            logger.error(f"[FAIL] Failed to initialize LM Studio provider: {e}")
        
        # ChatGPT (cloud) - Always add to providers, API key can be provided later
        try:
            chatgpt = ChatGPTProvider({})
            self.providers['chatgpt'] = chatgpt
            if chatgpt.is_available():
                self.provider_status['chatgpt'] = 'available'
                logger.info("[OK] ChatGPT provider initialized")
            else:
                self.provider_status['chatgpt'] = 'registered'
                logger.info("[INFO] ChatGPT provider registered (API key not configured, can be provided later)")
        except Exception as e:
            self.provider_status['chatgpt'] = 'failed'
            self.provider_errors['chatgpt'] = str(e)
            logger.error(f"[FAIL] Failed to initialize ChatGPT provider: {e}")
        
        # Gemini (cloud) - Always add to providers, API key can be provided later
        try:
            gemini = GeminiProvider({})
            self.providers['gemini'] = gemini
            if gemini.is_available():
                self.provider_status['gemini'] = 'available'
                logger.info("[OK] Gemini provider initialized")
            else:
                self.provider_status['gemini'] = 'registered'
                logger.info("[INFO] Gemini provider registered (API key not configured, can be provided later)")
        except Exception as e:
            self.provider_status['gemini'] = 'failed'
            self.provider_errors['gemini'] = str(e)
            logger.error(f"[FAIL] Failed to initialize Gemini provider: {e}")
        
        if not self.providers:
            logger.error("[WARN] No LLM providers available! Metadata generation will not work.")
        else:
            logger.info(f"Metadata service ready with {len(self.providers)} provider(s): {', '.join(self.providers.keys())}")
    
    def get_available_providers(self) -> List[str]:
        """Get list of available provider names"""
        return list(self.providers.keys())

    def analyze_batch(self, image_triplets: list[tuple[bytes, str, str]], options: dict, image_model, image_processor,
                     uuids_needing_embeddings=None, uuids_needing_metadata=None):
        """
        Analyzes a batch of images, generating embeddings and metadata.
        Only generates data for UUIDs in the corresponding needing_* lists.
        """
        uuids = [triplet[1] for triplet in image_triplets]
        image_data = [triplet[0] for triplet in image_triplets]
        images = [Image.open(io.BytesIO(data)).convert("RGB") for data in image_data]

        # If no specific UUIDs lists provided, generate for all (backward compatibility)
        if uuids_needing_embeddings is None:
            uuids_needing_embeddings = uuids if options.get('compute_embeddings', True) else []
        if uuids_needing_metadata is None:
            uuids_needing_metadata = uuids if options.get('compute_metadata', False) else []

        embeddings = None
        if len(uuids_needing_embeddings) > 0:
            logger.debug(f"Generating embeddings for {len(uuids_needing_embeddings)} images...")
            embeddings = []
            for i, uuid in enumerate(uuids):
                if uuid in uuids_needing_embeddings:
                    emb = self._generate_image_embeddings([images[i]], image_model, image_processor)
                    embeddings.append(emb[0] if emb else None)
                else:
                    embeddings.append(None)  # Placeholder for images not needing embeddings

        metadata_results = None
        if len(uuids_needing_metadata) > 0:
            logger.info(f"Generating metadata for {len(uuids_needing_metadata)} images out of {len(uuids)} total")
            logger.info(f"UUIDs needing metadata: {uuids_needing_metadata}")
            # Filter to only process images that need metadata
            filtered_triplets = [(image_data[i], uuids[i], '') for i, uuid in enumerate(uuids) if uuid in uuids_needing_metadata]
            logger.info(f"Filtered to {len(filtered_triplets)} triplets for metadata generation")
            partial_results = self._generate_metadata_batch([t[1] for t in filtered_triplets], 
                                                           [t[0] for t in filtered_triplets], 
                                                           options)
            # Reconstruct full results array with None for images that didn't need metadata
            metadata_results = []
            partial_idx = 0
            for uuid in uuids:
                if uuid in uuids_needing_metadata:
                    metadata_results.append(partial_results[partial_idx] if partial_results else None)
                    partial_idx += 1
                else:
                    metadata_results.append(None)

        # Datetime/capture_time extraction is handled entirely by the client
        # (Lightroom plugin) via explicit fields in the request and stored in
        # service_index.process_image_task.
        return embeddings, metadata_results

    def _generate_image_embeddings(self, images: List[Image.Image], image_model, image_processor) -> List[Optional[List[float]]]:
        """
        Generates embeddings for all images in the batch.
        Errors are handled per image.
        """
        if not image_model:
            logger.error("Vision model not initialized.")
            return [None] * len(images)
        
        embeddings = []

        for i, image in enumerate(images):
            try:
                # The image_processor is now the open_clip transform.
                # It returns a tensor for a single image, so we add a batch dimension.
                image_tensor = image_processor(image).unsqueeze(0).to(TORCH_DEVICE)

                with torch.no_grad():
                    image_features = image_model.encode_image(image_tensor)
                    normalized_embeddings = F.normalize(image_features, p=2, dim=1)
                    embeddings.append(normalized_embeddings.cpu().numpy()[0])
            
            except Exception as e:
                logger.error(f"Failed to generate image embedding for image at index {i}: {e}", exc_info=True)
                embeddings.append(None)
        
        return embeddings

    def _generate_metadata_batch(self, uuids: List[str], image_data: List[bytes], options: dict) -> List[Optional[MetadataGenerationResponse]]:
        """
        Generates metadata for all images in the batch.
        """
        results = []
        for i, uuid in enumerate(uuids):
            response = self.generate_metadata_single(uuid, image_data[i], options)
            results.append(response)
        return results

    def generate_metadata_single(
        self,
        uuid: str,
        image_data: bytes,
        options: dict
    ) -> MetadataGenerationResponse:
        """
        Generate metadata for a single image.
        """
        provider = options.get('provider') or DEFAULT_METADATA_PROVIDER
        
        if provider not in self.providers:
            if not self.providers:
                return MetadataGenerationResponse(uuid=uuid, success=False, error="No LLM providers available")
            provider = list(self.providers.keys())[0]
            warning_msg = f"Requested provider not available, using fallback: {provider}"
            logger.warning(warning_msg)
        
        selected_provider = self.providers[provider]
        logger.info(f"Generating metadata for {uuid} using {provider}")
        
        request = MetadataGenerationRequest(
            image_data=image_data,
            uuid=uuid,
            provider=provider,
            model=options['model'],
            api_key=options.get('api_key'),
            generate_keywords=options['generate_keywords'],
            generate_caption=options['generate_caption'],
            generate_title=options['generate_title'],
            generate_alt_text=options['generate_alt_text'],
            language=options['language'],
            temperature=options['temperature'],
            max_tokens=options.get('max_tokens'),
            user_prompt=options.get('user_prompt'),
            submit_gps=options['submit_gps'],
            submit_keywords=options['submit_keywords'],
            submit_folder_names=options['submit_folder_names'],
            existing_keywords=options.get('existing_keywords'),
            gps_coordinates=options.get('gps_coordinates'),
            folder_names=options.get('folder_names'),
            user_context=options.get('user_context'),
            keyword_categories=options.get('keyword_categories'),
            bilingual_keywords=options.get('bilingual_keywords', False),
            keyword_secondary_language=options.get('keyword_secondary_language'),
            system_prompt=options.get('prompt'),
            date_time=options.get('date_time'),
            ollama_base_url=options.get('ollama_base_url'),
            lmstudio_base_url=options.get('lmstudio_base_url'),
        )
        
        # Diagnostic logging for prompt context
        ctx_summary = []
        if request.existing_keywords: ctx_summary.append(f"{len(request.existing_keywords)} keywords")
        if request.folder_names: ctx_summary.append(f"{len(request.folder_names)} folders")
        if request.user_context: ctx_summary.append(f"context ({len(str(request.user_context))} chars)")
        if request.gps_coordinates: ctx_summary.append("GPS")
        if ctx_summary:
            logger.info(f"Context for {uuid}: {', '.join(ctx_summary)}")
        else:
            logger.debug(f"No additional context for {uuid}")

        try:
            response = selected_provider.generate_metadata(request)
            if not response.success:
                logger.error(f"[FAIL] Failed to generate metadata for {uuid}: {response.error}")
            if 'warning_msg' in locals():
                response.warning = warning_msg
            return response
        except Exception as e:
            logger.error(f"Unexpected error during metadata generation for {uuid}: {e}", exc_info=True)
            return MetadataGenerationResponse(uuid=uuid, success=False, error=str(e))

    def generate_edit_recipe_single(
        self,
        uuid: str,
        image_data: bytes,
        options: dict
    ) -> EditGenerationResponse:
        provider = options.get('provider') or DEFAULT_METADATA_PROVIDER

        if provider not in self.providers:
            if not self.providers:
                return EditGenerationResponse(uuid=uuid, success=False, error="No LLM providers available")
            provider = list(self.providers.keys())[0]
            warning_msg = f"Requested provider '{provider}' not available, using fallback: {provider}"
            logger.warning(warning_msg)

        selected_provider = self.providers[provider]
        logger.info(f"Generating edit recipe for {uuid} using {provider}")

        request = EditGenerationRequest(
            image_data=image_data,
            uuid=uuid,
            provider=provider,
            model=options['model'],
            api_key=options.get('api_key'),
            language=options.get('language', DEFAULT_METADATA_LANGUAGE),
            temperature=options.get('temperature', 0.2),
            max_tokens=options.get('max_tokens'),
            user_prompt=options.get('user_prompt'),
            submit_gps=options.get('submit_gps', False),
            submit_keywords=options.get('submit_keywords', False),
            submit_folder_names=options.get('submit_folder_names', False),
            existing_keywords=options.get('existing_keywords'),
            gps_coordinates=options.get('gps_coordinates'),
            folder_names=options.get('folder_names'),
            user_context=options.get('user_context'),
            system_prompt=options.get('prompt'),
            date_time=options.get('date_time'),
            edit_intent=options.get('edit_intent'),
            style_strength=options.get('style_strength', 0.5),
            include_masks=options.get('include_masks', True),
            adjust_white_balance=options.get('adjust_white_balance', True),
            adjust_basic_tone=options.get('adjust_basic_tone', True),
            adjust_presence=options.get('adjust_presence', True),
            adjust_color_mix=options.get('adjust_color_mix', True),
            do_color_grading=options.get('do_color_grading', True),
            use_tone_curve=options.get('use_tone_curve', True),
            use_point_curve=options.get('use_point_curve', True),
            adjust_detail=options.get('adjust_detail', True),
            adjust_effects=options.get('adjust_effects', True),
            adjust_lens_corrections=options.get('adjust_lens_corrections', True),
            allow_auto_crop=options.get('allow_auto_crop', True),
            composition_mode=options.get('composition_mode', 'subtle'),
            ollama_base_url=options.get('ollama_base_url'),
            lmstudio_base_url=options.get('lmstudio_base_url'),
        )

        # Query similar training examples for few-shot style injection.
        training_examples = None
        use_training = options.get('use_training_style', True)
        if use_training:
            try:
                # Re-use the CLIP embedding of the current photo from the main collection
                # (best-effort: skip if not available).
                import service_chroma as chroma_service
                existing = chroma_service.get_image(uuid)
                embedding = None
                if existing and existing.get("ids") and existing.get("embeddings"):
                    raw_emb = existing["embeddings"][0]
                    if raw_emb is not None:
                        import numpy as np
                        emb_arr = np.asarray(raw_emb, dtype=np.float32)
                        if emb_arr.size > 0 and not np.allclose(emb_arr, 0.0):
                            embedding = emb_arr.tolist()
                if embedding is not None:
                    training_examples = training_service.query_similar_training_examples(
                        embedding, n_results=3
                    )
                    if training_examples:
                        logger.info(
                            "Injecting %d training example(s) for uuid=%s",
                            len(training_examples),
                            uuid,
                        )
            except Exception as exc:
                training_warning = f"Could not retrieve training examples for uuid={uuid}: {exc}"
                logger.warning(training_warning)

        request.training_examples = training_examples or []

        try:
            response = selected_provider.generate_edit_recipe(request)
            if response.success and isinstance(response.recipe, dict):
                response.recipe = filter_edit_recipe_by_controls(
                    response.recipe,
                    {
                        "include_masks": request.include_masks,
                        "adjust_white_balance": request.adjust_white_balance,
                        "adjust_basic_tone": request.adjust_basic_tone,
                        "adjust_presence": request.adjust_presence,
                        "adjust_color_mix": request.adjust_color_mix,
                        "do_color_grading": request.do_color_grading,
                        "use_tone_curve": request.use_tone_curve,
                        "use_point_curve": request.use_point_curve,
                        "adjust_detail": request.adjust_detail,
                        "adjust_effects": request.adjust_effects,
                        "adjust_lens_corrections": request.adjust_lens_corrections,
                        "allow_auto_crop": request.allow_auto_crop,
                        "composition_mode": request.composition_mode,
                    },
                )
            if not response.success:
                logger.error(f"[FAIL] Failed to generate edit recipe for {uuid}: {response.error}")
            
            # Aggregate warnings
            warnings = []
            if 'warning_msg' in locals():
                warnings.append(warning_msg)
            if 'training_warning' in locals():
                warnings.append(training_warning)
            if warnings:
                response.warning = " | ".join(warnings)
                
            return response
        except Exception as e:
            logger.error(f"Unexpected error during edit generation for {uuid}: {e}", exc_info=True)
            return EditGenerationResponse(uuid=uuid, success=False, error=str(e))

    def get_available_models(
        self,
        openai_apikey: Optional[str] = None,
        gemini_apikey: Optional[str] = None,
        ollama_base_url: Optional[str] = None,
        lmstudio_base_url: Optional[str] = None,
    ) -> Dict[str, List[str]]:
        """
        Return all available multimodal (vision-capable) models from all providers.
        """
        result: Dict[str, List[str]] = {}
        for provider_name, provider_instance in self.providers.items():
            try:
                if provider_name == 'chatgpt' and openai_apikey:
                    provider_instance.api_key = openai_apikey
                if provider_name == 'gemini' and gemini_apikey:
                    provider_instance.api_key = gemini_apikey
                
                if provider_name == 'ollama' and ollama_base_url:
                    provider_instance = OllamaProvider({'base_url': ollama_base_url})
                if provider_name == 'lmstudio' and lmstudio_base_url:
                    # Reuse existing provider instance but point it to a different host
                    provider_instance.host = lmstudio_base_url

                if provider_name in ['ollama', 'lmstudio'] and not provider_instance.is_available():
                    result[provider_name] = []
                    continue

                models = provider_instance.list_available_models()
                result[provider_name] = models
            except Exception as e:
                logger.error(f"Error listing models for provider {provider_name}: {e}", exc_info=True)
                result[provider_name] = []
        return result

    def get_health_status(self) -> Dict[str, Any]:
        """Return health status of LLM providers."""
        return {
            "llm_providers": self.provider_status,
            "llm_errors": self.provider_errors
        }

# Global service instance
_analysis_service: Optional[AnalysisService] = None

def get_analysis_service() -> AnalysisService:
    """
    Get or create the global analysis service instance.
    """
    global _analysis_service
    if _analysis_service is None:
        _analysis_service = AnalysisService()
    return _analysis_service