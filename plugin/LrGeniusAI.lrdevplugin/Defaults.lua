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

Defaults.defaultGenerateLanguage = "English"

Defaults.generateLanguages = { "English", "German", "French", "Spanish", "Italian", "Norwegian" }
Defaults.defaultBilingualKeywords = false
Defaults.defaultKeywordSecondaryLanguage = "English"

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

Defaults.credits = {
	{ name = "JSON.lua by Jeffrey Friedl", author = "Jeffrey Friedl", url = "http://regex.info/blog/lua/json" },
	{
		name = "timm--ViT-SO400M-16-SigLIP2-384",
		author = "rwightman",
		url = "https://huggingface.co/timm/ViT-SO400M-16-SigLIP2-384",
	},
	{ name = "Flask", author = "Pallets", url = "https://flask.palletsprojects.com/" },
	{ name = "Waitress", author = "Pylons Project", url = "https://github.com/Pylons/waitress" },
	{ name = "ChromaDB", author = "Chroma", url = "https://www.trychroma.com/" },
	{ name = "OpenCLIP", author = "OpenAI & Contributors", url = "https://github.com/mlfoundations/open_clip" },
	{ name = "PyTorch", author = "Meta & Contributors", url = "https://pytorch.org/" },
	{ name = "Pillow", author = "Alex Clark & Contributors", url = "https://python-pillow.org/" },
	{ name = "NumPy", author = "NumPy Developers", url = "https://numpy.org/" },
	{ name = "Pandas", author = "Pandas Development Team", url = "https://pandas.pydata.org/" },
	{ name = "Transformers", author = "Hugging Face", url = "https://huggingface.co/transformers/" },
	{ name = "Google GenAI SDK", author = "Google", url = "https://ai.google.dev/" },
	{ name = "OpenAI SDK", author = "OpenAI", url = "https://github.com/openai/openai-python" },
	{ name = "Ollama SDK", author = "Ollama", url = "https://github.com/ollama/ollama-python" },
	{ name = "LM Studio SDK", author = "LM Studio", url = "https://lmstudio.ai/" },
	{ name = "InsightFace", author = "DeepInsight", url = "https://github.com/deepinsight/insightface" },
	{ name = "ONNX Runtime", author = "Microsoft", url = "https://onnxruntime.ai/" },
	{ name = "Scikit-learn", author = "scikit-learn developers", url = "https://scikit-learn.org/" },
	{ name = "Psutil", author = "Giampaolo Rodola", url = "https://github.com/giampaolo/psutil" },
	{ name = "Requests", author = "Kenneth Reitz & Contributors", url = "https://requests.readthedocs.io/" },
	{ name = "Vertex AI SDK (Google Cloud)", author = "Google", url = "https://cloud.google.com/vertex-ai" },
	{ name = "Torchvision", author = "PyTorch Team", url = "https://pytorch.org/vision/" },
	{ name = "Tokenizers", author = "Hugging Face", url = "https://github.com/huggingface/tokenizers" },
}

Defaults.copyrightString = ""
for _, credit in ipairs(Defaults.credits) do
	Defaults.copyrightString = Defaults.copyrightString .. string.format("%s (%s)\n", credit.name, credit.url)
end

return Defaults
