Defaults = {}

Defaults.defaultTopLevelKeyword = "LrGeniusAI"

Defaults.defaultPromptName = "Default"
Defaults.defaultEditPromptName = "Default"

Defaults.defaultTopLevelKeywords = {
	"LrGeniusAI",
	"Ollama",
	"LM Studio",
	"ChatGPT",
	"Google Gemini",
}

Defaults.topLevelKeywordSynonym = "LrGeniusAI Top-Level Keyword"

-- Root of the taxonomy keyword branch BioCLIP writes under.
--
-- Deliberately *not* the LrGeniusAI root the LLM keywords live under. Two
-- reasons: `MetadataManager.buildAliasIndex` scopes itself to that subtree, and
-- letting a Linnean hierarchy into it would give the LLM's de-clutter pass
-- thousands of scientific names to try to merge things onto; and a taxonomy
-- branch is something a user may well want to keep, export or delete
-- independently of AI-generated descriptive keywords.
Defaults.defaultSpeciesKeyword = "Species"

-- Aggregated probability a taxonomic rank must reach before it is written.
--
-- Lower than it looks like it should be: rank aggregation splits probability
-- across near-identical congeners, so a *correct* genus call often peaks
-- around 0.4-0.6. Raising this does not buy accuracy, it just pushes answers
-- up to coarser ranks.
Defaults.speciesMinConfidence = 0.35

-- Only run BioCLIP on photos where the organism gate fires. See the
-- `species_prefilter` option in the backend's `index_upload.rs` for why this
-- defaults on.
Defaults.speciesPrefilter = true

-- Write the taxonomy as a keyword hierarchy in addition to the plugin
-- metadata fields. Off by default: the fields are always written and are
-- enough to filter on, while keywords change the catalog's keyword tree,
-- which is a bigger commitment and exports with the file.
Defaults.speciesKeywords = false

-- Language for the iNaturalist and Wikipedia links written into the Metadata
-- panel. "auto" follows Lightroom's own interface language; anything else is a
-- Wikipedia subdomain. Only affects which article a link opens — the
-- identification itself is language-independent.
Defaults.speciesLinkLang = "auto"

-- Offered in the Analyze & Index dialog. Not the same list as
-- `Defaults.generateLanguages`: that one names languages for an LLM to write
-- in, these are Wikipedia subdomains, and the value has to be the subdomain.
Defaults.speciesLinkLanguages = {
	{ title = "Automatic (interface language)", value = "auto" },
	{ title = "English", value = "en" },
	{ title = "Deutsch", value = "de" },
	{ title = "Français", value = "fr" },
	{ title = "Español", value = "es" },
	{ title = "Italiano", value = "it" },
	{ title = "Nederlands", value = "nl" },
	{ title = "Português", value = "pt" },
	{ title = "Svenska", value = "sv" },
	{ title = "Polski", value = "pl" },
	{ title = "Русский", value = "ru" },
	{ title = "日本語", value = "ja" },
	{ title = "中文", value = "zh" },
}

Defaults.defaultGenerateLanguage = "English"

Defaults.generateLanguages = { "English", "German", "French", "Spanish", "Italian", "Norwegian" }
Defaults.defaultBilingualKeywords = false
Defaults.defaultKeywordSecondaryLanguage = "English"
Defaults.defaultKeywordAliases = false

-- How many catalog keywords are sent to the LLM as existing vocabulary. Enough
-- to cover a working photographer's live vocabulary without turning the
-- run-constant part of the prompt into the bulk of the context on small local
-- models.
Defaults.catalogKeywordLimit = 500

Defaults.defaultTemperature = 0.1
Defaults.defaultMaxTokens = 2048

Defaults.defaultKeywordCategories = {
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Activities=Activities"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Buildings=Buildings"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Location=Location"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Objects=Objects"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/People=People"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Moods=Moods"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Sceneries=Sceneries"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Texts=Texts"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Companies=Companies"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Weather=Weather"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Plants=Plants"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Animals=Animals"),
	LOC("$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Vehicles=Vehicles"),
}

Defaults.exportSizes = {
	"512",
	"1024",
	"2048",
	"3072",
	"4096",
}

Defaults.defaultOllamaBaseUrl = "http://localhost:11434"
Defaults.defaultLmStudioBaseUrl = "localhost:1234"

-- Local (in-process) model tuning. The whole group of photos in a request has
-- to fit the context window alongside the shared prompt prefix, so context size
-- and parallel sequences trade against each other; the server clamps parallel
-- sequences down if they will not fit. Deliberately conservative defaults:
-- selecting the local provider should not cause a multi-gigabyte memory jump.
Defaults.defaultLlmContextSize = 8192
Defaults.defaultLlmParallel = 2
-- llama.cpp keeps on the CPU whatever does not fit, so "all layers" is a safe
-- request on Metal and on any card with enough VRAM.
Defaults.defaultLlmGpuLayers = 999

Defaults.defaultBackendServerUrl = "http://127.0.0.1:19819"

Defaults.defaultExportQuality = 50
Defaults.defaultExportSize = "3072"

Defaults.defaultSystemInstruction =
	"You are a professional photography analyst with expertise in object recognition and computer-generated image description. You also try to identify famous buildings and landmarks as well as the location where the photo was taken. Furthermore, you aim to specify animal and plant species as accurately as possible. You also describe objects—such as vehicle types and manufacturers—as specifically as you can."
Defaults.defaultEditSystemInstruction =
	"You are a senior Lightroom Classic retoucher. Return only a structured Lightroom edit recipe that matches the schema exactly. No prose, no markdown, no unsupported controls. Build edits in this order: white balance and exposure foundation, tonal shaping, color refinement, detail/effects. Use masks only when materially beneficial and only for subject, sky, or background. Prefer subtle, natural, premium output unless explicitly asked for a stylized look. When a curve-shaped response is needed, prefer explicit tone_curve point curves over simulating everything with contrast alone."
Defaults.defaultEditIntent = "Natural professional Lightroom edit"
Defaults.editIntentCustomValue = "custom"
Defaults.defaultEditIntentPresetValue = "natural_pro"
Defaults.editIntentPresets = {
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/NaturalPro=General - Natural Professional"),
		value = "natural_pro",
		instruction = "Natural professional Lightroom edit with balanced contrast, realistic color, and clean detail.",
	},
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/MoodyDramatic=General - Moody Dramatic"),
		value = "moody_dramatic",
		instruction = "Moody dramatic treatment with deeper shadows, restrained saturation, and cinematic tonal separation while preserving realism.",
	},
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/CinematicLandscape=Landscape - Cinematic"),
		value = "cinematic_landscape",
		instruction = "Cinematic landscape look with controlled dynamic range, subtle color contrast, and tasteful depth without overprocessing.",
	},
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/VibrantNaturalLandscape=Landscape - Vibrant Natural"),
		value = "landscape_vibrant_natural",
		instruction = "Vibrant but natural landscape look with clear tonal separation, protected highlights, and controlled saturation.",
	},
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/SkinSafePortrait=Portrait - Skin Safe"),
		value = "portrait_skin_safe",
		instruction = "Portrait-focused edit with skin-tone safety, gentle contrast, natural texture, and flattering highlights.",
	},
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/EditorialPortrait=Portrait - Editorial"),
		value = "portrait_editorial",
		instruction = "Editorial portrait style with clean skin tones, polished midtone contrast, soft highlight roll-off, and restrained color shifts.",
	},
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/SoftAiryWedding=Wedding - Soft Airy"),
		value = "wedding_soft_airy",
		instruction = "Soft airy wedding style with bright mids, warm-neutral white balance, gentle contrast, and elegant highlight rendering.",
	},
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/RichFilmicWedding=Wedding - Rich Filmic"),
		value = "wedding_rich_filmic",
		instruction = "Rich filmic wedding style with subtle warm skin tones, gentle black-point lift, and cinematic but natural color depth.",
	},
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/BrightNeutralRealEstate=Real Estate - Bright Neutral"),
		value = "real_estate_bright_neutral",
		instruction = "Real-estate edit with bright neutral interiors, straight tonal balance, clean whites, and minimal stylization.",
	},
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/CleanCommercial=Commercial - Clean Product"),
		value = "clean_commercial",
		instruction = "Clean commercial look: neutral white balance, crisp detail, controlled contrast, and true-to-product colors.",
	},
	{
		title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/PunchyDocumentaryStreet=Street - Punchy Documentary"),
		value = "street_punchy_doc",
		instruction = "Punchy documentary street look with decisive contrast, neutral color fidelity, and clear subject separation.",
	},
	{ title = LOC("$$$/LrGeniusAI/Defaults/EditIntent/Custom=Custom"), value = "custom", instruction = "" },
}
Defaults.defaultEditStyleStrength = 0.5
Defaults.defaultCompositionMode = "subtle"
Defaults.compositionModes = {
	{ title = LOC("$$$/LrGeniusAI/Defaults/CompositionMode/None=No crop"), value = "none" },
	{ title = LOC("$$$/LrGeniusAI/Defaults/CompositionMode/Subtle=Subtle crop"), value = "subtle" },
	{ title = LOC("$$$/LrGeniusAI/Defaults/CompositionMode/Aggressive=Aggressive crop"), value = "aggressive" },
}

Defaults.catalogWriteAccessOptions = {
	timeout = 60, -- seconds
}

-- Everything the plug-in and its backend are built on, grouped the way the
-- pieces actually fit together. The Python/Flask/Chroma stack this used to
-- list is gone: the backend is a single Rust binary since the 2026 rewrite.
Defaults.credits = {
	{
		section = "Lightroom plug-in",
		items = {
			{ name = "JSON.lua", author = "Jeffrey Friedl", url = "http://regex.info/blog/lua/json" },
		},
	},
	{
		section = "Backend — core",
		items = {
			{ name = "Rust", author = "Rust project", url = "https://www.rust-lang.org/" },
			{ name = "Tokio", author = "Tokio project", url = "https://tokio.rs/" },
			{ name = "axum", author = "Tokio project", url = "https://crates.io/crates/axum" },
			{ name = "tower", author = "Tower project", url = "https://crates.io/crates/tower" },
			{ name = "Serde / serde_json", author = "Serde project", url = "https://serde.rs/" },
			{ name = "clap", author = "clap-rs", url = "https://crates.io/crates/clap" },
			{ name = "log", author = "Rust project", url = "https://crates.io/crates/log" },
			{ name = "chrono", author = "chronotope", url = "https://crates.io/crates/chrono" },
			{ name = "regex", author = "Rust project", url = "https://crates.io/crates/regex" },
			{ name = "uuid", author = "uuid-rs", url = "https://crates.io/crates/uuid" },
			{ name = "reqwest", author = "Sean McArthur", url = "https://crates.io/crates/reqwest" },
			{ name = "rayon", author = "rayon-rs", url = "https://crates.io/crates/rayon" },
			{ name = "futures-rs", author = "Rust project", url = "https://crates.io/crates/futures" },
			{ name = "async-trait", author = "David Tolnay", url = "https://crates.io/crates/async-trait" },
			{ name = "thiserror", author = "David Tolnay", url = "https://crates.io/crates/thiserror" },
			{ name = "base64", author = "Marshall Pierce", url = "https://crates.io/crates/base64" },
			{ name = "sha2", author = "RustCrypto", url = "https://crates.io/crates/sha2" },
			{ name = "zip", author = "zip-rs", url = "https://crates.io/crates/zip" },
		},
	},
	{
		section = "Backend — storage",
		items = {
			{ name = "LanceDB", author = "LanceDB", url = "https://lancedb.com/" },
			{ name = "Apache Arrow", author = "Apache Software Foundation", url = "https://arrow.apache.org/" },
			{ name = "rusqlite", author = "rusqlite contributors", url = "https://crates.io/crates/rusqlite" },
			-- Only used to read a leftover ChromaDB directory during migration.
			{ name = "serde-pickle", author = "Georg Brandl", url = "https://crates.io/crates/serde-pickle" },
		},
	},
	{
		section = "Backend — imaging",
		items = {
			{ name = "image", author = "image-rs", url = "https://crates.io/crates/image" },
			{ name = "fast_image_resize", author = "Cykooz", url = "https://crates.io/crates/fast_image_resize" },
			{ name = "rawler", author = "DNGLab project", url = "https://crates.io/crates/rawler" },
			{ name = "kamadak-exif", author = "KAMADA Ken'ichi", url = "https://crates.io/crates/kamadak-exif" },
		},
	},
	{
		section = "Backend — machine learning",
		items = {
			{ name = "ONNX Runtime", author = "Microsoft", url = "https://onnxruntime.ai/" },
			{ name = "ort", author = "pyke.io", url = "https://ort.pyke.io/" },
			{ name = "Tokenizers", author = "Hugging Face", url = "https://github.com/huggingface/tokenizers" },
			{ name = "ndarray", author = "rust-ndarray", url = "https://crates.io/crates/ndarray" },
		},
	},
	{
		section = "Local AI model backends",
		items = {
			{
				name = "llama.cpp / ggml",
				author = "Georgi Gerganov & contributors",
				url = "https://github.com/ggml-org/llama.cpp",
			},
			{ name = "llama-cpp-2", author = "Utility AI", url = "https://crates.io/crates/llama-cpp-2" },
			{ name = "llguidance / toktrie", author = "guidance-ai", url = "https://crates.io/crates/llguidance" },
			{ name = "MLX / mlx-swift-lm", author = "Apple", url = "https://github.com/ml-explore/mlx-swift-lm" },
			{
				name = "swift-transformers",
				author = "Hugging Face",
				url = "https://github.com/huggingface/swift-transformers",
			},
		},
	},
	{
		section = "Optional AI providers",
		items = {
			{ name = "OpenAI API", author = "OpenAI", url = "https://platform.openai.com/" },
			{ name = "Google Gemini API", author = "Google", url = "https://ai.google.dev/" },
			{ name = "Ollama", author = "Ollama", url = "https://ollama.com/" },
			{ name = "LM Studio", author = "LM Studio", url = "https://lmstudio.ai/" },
			{ name = "gcp_auth", author = "gcp_auth contributors", url = "https://crates.io/crates/gcp_auth" },
		},
	},
	{
		section = "Models",
		items = {
			{
				name = "SigLIP2 (ViT-SO400M-16-SigLIP2-384)",
				author = "Google, via timm/rwightman",
				url = "https://huggingface.co/timm/ViT-SO400M-16-SigLIP2-384",
			},
			{
				name = "InsightFace buffalo_l (SCRFD + ArcFace)",
				author = "DeepInsight",
				url = "https://github.com/deepinsight/insightface",
			},
			{ name = "Gemma", author = "Google DeepMind", url = "https://ai.google.dev/gemma" },
			{ name = "Qwen-VL", author = "Alibaba Qwen team", url = "https://github.com/QwenLM" },
			{ name = "Ministral", author = "Mistral AI", url = "https://mistral.ai/" },
			{ name = "SmolVLM", author = "Hugging Face", url = "https://huggingface.co/HuggingFaceTB" },
		},
	},
}

-- Rendered once into the flat block of text the Credits section shows.
Defaults.copyrightString = ""
for _, group in ipairs(Defaults.credits) do
	Defaults.copyrightString = Defaults.copyrightString .. group.section .. "\n"
	for _, credit in ipairs(group.items) do
		Defaults.copyrightString = Defaults.copyrightString
			.. string.format("    %s — %s (%s)\n", credit.name, credit.author, credit.url)
	end
	Defaults.copyrightString = Defaults.copyrightString .. "\n"
end

-- An LrView static_text does not grow to fit a multi-line string, it clips to
-- height_in_lines, so the Credits view has to be told how tall the text is.
Defaults.copyrightLineCount = select(2, Defaults.copyrightString:gsub("\n", "\n"))

return Defaults
