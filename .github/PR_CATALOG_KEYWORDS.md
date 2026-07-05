## Description

The backend already supports sending the full catalog keyword vocabulary to the LLM via the `catalog_keywords` field in `MetadataGenerationRequest`, and injects it into the prompt. However, the Lua plugin never populated this field.

## Changes

### Plugin (Lua)
- **TaskAnalyzeAndIndex.lua**: Call `MetadataManager.collectCatalogKeywordNames()` (limit 500) and pass as `options.catalog_keywords`
- **TaskAiEditPhotos.lua**: Same for the AI edit path

### Backend (Python)
- **base.py** (`_normalize_keywords_structure`): Added deduplication (case-insensitive) and catalog name normalization within the response. If the LLM returns "fruehling" and the catalog has "Fruehling", the canonical catalog spelling is used
- All providers (ollama, chatgpt, gemini, lmstudio): Pass `request.catalog_keywords` to the normalization function

## Related Issues
Closes #213
