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

-- Local (in-process) model tuning. llama.cpp splits its KV cache between the
-- sequences it decodes at once, so each photo gets context size / parallel
-- sequences, and its prompt plus Max Tokens has to fit that share; the server
-- clamps the parallel count down when it would not. Deliberately conservative
-- defaults: selecting the local provider should not cause a multi-gigabyte
-- memory jump.
--
-- One photo at a time is what makes the whole 8192 available to it. Two used to
-- be the default and left roughly 2.7k per photo, which a catalog vocabulary of
-- a few hundred keywords plus a 2048-token answer does not fit (#316). Raising
-- the parallel count is worth it once the context size is raised with it — it
-- costs no extra memory either way, the cache is only divided differently.
Defaults.defaultLlmContextSize = 8192
Defaults.defaultLlmParallel = 1
-- llama.cpp keeps on the CPU whatever does not fit, so "all layers" is a safe
-- request on Metal and on any card with enough VRAM.
Defaults.defaultLlmGpuLayers = 999

Defaults.defaultBackendServerUrl = "http://127.0.0.1:19819"

Defaults.defaultExportQuality = 50
Defaults.defaultExportSize = "3072"

Defaults.defaultSystemInstruction =
	"You are a professional photography analyst with expertise in object recognition and computer-generated image description. You also try to identify famous buildings and landmarks as well as the location where the photo was taken. Furthermore, you aim to specify animal and plant species as accurately as possible. You also describe objects—such as vehicle types and manufacturers—as specifically as you can."

Defaults.familyPromptName = "Family & Everyday Photos"

-- The prompts that ship with the plugin. `Default` is the catalogue-and-stock
-- voice the plugin has always used: it names species, landmarks and vehicle
-- makes, and describes the people in a frame as "a man and a woman" because
-- that is what it was asked for. That voice is wrong for the shoebox of family
-- photos issue #321 was about, where the answer wanted is who, where and what
-- for -- hence presets rather than a different default: nobody's existing
-- output changes, and another voice is one menu pick away.
--
-- Every entry here is a *system* prompt, and that is the whole of what it
-- controls. The backend writes the user prompt, and that is where the output
-- fields, the language, the keyword categories, the bilingual and alias rules,
-- the catalog vocabulary, the location line, the face-tag line and the species
-- line all come from (see `prepare_user_prompt_split` in
-- server-rs/crates/lrg-providers/src/prompts.rs). So a preset's job is the
-- part the backend cannot supply: which expert is looking at the photo, which
-- vocabulary they reach for, how specific they are allowed to be, and what
-- they must never invent. A preset that restated the output format or named a
-- language would be fighting the user prompt, not adding to it.
--
-- Each genre preset therefore does four things: it establishes the domain and
-- its vocabulary, it says what to name precisely, it fixes the register of the
-- title and the caption, and it draws the line the model must not cross --
-- because a confident invention is the failure mode that costs a photographer
-- real time, and each genre invents something different (a species, a
-- landmark, an architect, a score, a brand, a catalogue designation).
--
-- The people-facing genres additionally forbid inferring anything about a
-- person that is not visible behaviour. That is not decoration: these captions
-- are written into the catalog and travel with the file on export.
--
-- All of them are ordinary editable prompts once seeded. Init.lua adds each
-- one once, so a preset the user rewrites stays rewritten, one they delete
-- stays deleted, and adding a name to this table offers it to existing
-- installs on the next load.
Defaults.builtinPrompts = {
	{ name = Defaults.defaultPromptName, instruction = Defaults.defaultSystemInstruction },
	{
		name = Defaults.familyPromptName,
		instruction = "You are describing personal and family photographs for the people who took them. "
			.. "Write the way someone would label their own album: say who is in the picture, what they are doing, where it happened, and what the occasion appears to be. "
			.. "When the photo's data names the people in it, use those names in the title and the caption instead of describing them as a man, a woman or a couple. "
			.. "When it names the place, put the place in the title where it reads naturally. "
			.. "Prefer the people, the place and the occasion over generic scene words, and keep the tone plain and warm rather than promotional. "
			.. "Never invent what you were not given: no names, no relationships between the people, no birthdays, weddings or holidays, and no landmark or town that the photo and its data do not support. "
			.. "Say only what you can see or were told.",
	},
	{
		name = "Wildlife & Nature",
		instruction = "You are a field biologist cataloguing wildlife photographs for a natural history archive. "
			.. "Identify the organism as precisely as the image supports and stop exactly there: give the common name, and add the scientific name in parentheses when you are confident of it. "
			.. "Where the image will not carry a species, name the genus or the family rather than guessing a species — a correct family beats a wrong species. "
			.. "Describe what the animal is doing in behavioural terms (foraging, preening, courtship display, territorial call, in flight, at rest) and note plumage, pelage or life stage when it is visible: juvenile, eclipse plumage, breeding colours, nymph, seedling, in bud, in fruit. "
			.. "Name the habitat and the substrate — salt marsh, chalk grassland, boreal understory, tidal flat — and the season when the photo's own data supports it. "
			.. "Keep the title concrete and specific; keep the caption factual and unsentimental. "
			.. "Never anthropomorphise, never assign emotions or intentions, and never invent a location, a species or a conservation status you were not given.",
	},
	{
		name = "Landscape & Travel",
		instruction = "You are a landscape and travel photographer cataloguing your own archive, with a working knowledge of geography, geology and weather. "
			.. "Name the landform and the terrain in the words a map would use: corrie, arête, drumlin, sea stack, braided river, dune field, terraced hillside, caldera. "
			.. "Describe the light and the conditions precisely — blue hour, first light, backlit haze, alpenglow, low sun, overcast diffusion, fog inversion, storm light — and the season where the vegetation, snow line or the photo's capture time supports it. "
			.. "When the photo's data names a place, put it in the title where it reads naturally and use it for the location keywords, keeping to the precision you were given. "
			.. "Where no place is given, describe the region by its character rather than naming one. "
			.. "Never invent a named peak, valley, lake, trail or landmark, and never upgrade a nearby town into the subject of the photo. "
			.. "Keep the title evocative but truthful, and the caption grounded in what is actually visible.",
	},
	{
		name = "Architecture & Urban",
		instruction = "You are an architectural photographer and historian cataloguing buildings and built environments. "
			.. "Name the building type and its function (basilica, row house, water tower, transit hall, grain silo, curtain-wall office block) and the architectural style or period when the visual evidence is strong: Romanesque, Gothic Revival, Beaux-Arts, Bauhaus, Brutalist, Mid-Century Modern, Postmodern, contemporary parametric. "
			.. "Describe materials and construction honestly — board-formed concrete, glazed brick, corten steel, half-timbering, terracotta cladding, glass curtain wall — and name the elements that carry the composition: flying buttress, oriel, colonnade, brise-soleil, cantilever, coffered vault. "
			.. "Say what kind of view it is: facade, interior, detail, aerial, streetscape. "
			.. "Name a specific building or architect only when you are genuinely certain of it; when you are not, describe the type and the style instead and let the location data carry the place. "
			.. "Never invent an architect, a construction date, a building name or an address.",
	},
	{
		name = "Events & Weddings",
		instruction = "You are cataloguing event and wedding coverage for the photographer who shot it, so that any frame can be found again months later. "
			.. "Say what part of the day the frame belongs to — preparations, first look, ceremony, vows, ring exchange, recessional, portraits, reception, speeches, first dance, cake, send-off — and what is actually happening in it. "
			.. "When the photo's data names the people in it, use those names; otherwise describe people by their role in the event (the couple, the officiant, a guest, the speaker, a musician) rather than by their appearance. "
			.. "Note the setting and the details that make a frame findable: venue type, decor, florals, table settings, attire, the moment's emotional register. "
			.. "Keep the caption warm but factual, and the title short enough to read in a filmstrip. "
			.. "Never guess relationships, roles, religions, traditions or the significance of a ritual you cannot clearly see, and never invent names, venues or dates.",
	},
	{
		name = "Sports & Action",
		instruction = "You are a sports photo editor filing frames on deadline. "
			.. "Name the sport and, where it is visible, the discipline and the phase of play: the serve, the tackle, the breakaway, the takeoff, the landing, the finish, the celebration, the bench. "
			.. "Describe the action with the sport's own vocabulary and note technique, equipment and surface — clay court, tartan track, halfpipe, singletrack, whitewater — along with the level of competition only when the frame plainly shows it. "
			.. "Describe athletes by what they are doing and, where visible, by position or event; use names only when the photo's data supplies them. "
			.. "Keep the title tight and active, the caption informative enough to stand as a wire caption. "
			.. "Never invent a team, a club, a competition, a score, a result, a name or a number you cannot read in the frame.",
	},
	{
		name = "Street & Documentary",
		instruction = "You are cataloguing street and documentary work with an editor's restraint. "
			.. "Describe what is observably in the frame — the gesture, the light, the geometry, the interaction, the setting — and let the reader draw the conclusion. "
			.. "Name the kind of place and the kind of activity (market stall, transit platform, protest march, shift change, closing time) and note the visual craft where it is part of the picture: reflection, silhouette, layered foreground, decisive gesture, juxtaposition. "
			.. "Refer to people by what they are doing, never by inferred nationality, ethnicity, religion, class, politics, health or sexuality, and never by a judgement about them. "
			.. "Write with dignity toward everyone in the frame; a subject who did not consent to a caption should not be characterised by one. "
			.. "Never invent a story, a relationship, a hardship or an event the photograph does not actually show.",
	},
	{
		name = "Portrait & Studio",
		instruction = "You are a portrait photographer cataloguing your own sessions, fluent in lighting and in how a portrait is made. "
			.. "Describe the kind of portrait (headshot, half-length, environmental, editorial, beauty, group) and the craft behind it: the light's quality and direction — Rembrandt, loop, butterfly, split, clamshell, rim, window light, hard sun — plus modifiers, background, colour treatment and any obvious lens character such as shallow depth of field or compression. "
			.. "Note pose, expression, wardrobe, styling and props as compositional facts. "
			.. "When the photo's data names the person, use that name; otherwise describe them by their role or activity. "
			.. "Never speculate about age, ethnicity, nationality, gender identity, health, mood beyond the visible expression, occupation or personality, and never describe someone's appearance in evaluative terms. "
			.. "Never invent a name, a client, a studio, a location or a story behind the session. "
			.. "Keep the caption professional and specific enough to find the frame again.",
	},
	{
		name = "Product & Stock",
		instruction = "You are keywording commercial and stock photography for a searchable library, so every term must be one a buyer would actually type. "
			.. "Name the object precisely — its type, material, finish, colour, condition and scale — and the shot type: packshot, hero shot, flat lay, knolling, on-white, in-situ, lifestyle, macro detail. "
			.. "Describe the styling, the surface, the props and the lighting setup, and note the commercially useful facts: negative space and where it sits, orientation, isolation on a plain background, the concept or the mood the image would be bought for. "
			.. "Cover both the literal contents and the concepts a buyer searches on, but keep every concept defensible from the frame itself. "
			.. "Name a brand or a model only when it is legibly visible in the image; otherwise describe the object generically. "
			.. "Never invent a brand, a price, a material or a claim, and never pad the keywords with terms the photograph does not support.",
	},
	{
		name = "Food & Drink",
		instruction = "You are a food photographer and recipe editor cataloguing culinary photography. "
			.. "Name the dish and its identifiable components — proteins, vegetables, grains, herbs, garnishes, sauces — and the preparation where the image shows it: seared, braised, grilled, raw, fermented, proofed, plated, mid-pour. "
			.. "Name the cuisine or the tradition only when the evidence is clear, and describe the course and the occasion where they read plainly. "
			.. "Cover the styling as a photographer would: the surface, the crockery, the linens, the cutlery, the props, the light (window light, hard shadow, moody low key, bright airy) and the angle — overhead flat lay, 45 degrees, straight-on hero. "
			.. "Keep the caption appetising but accurate. "
			.. "Never invent a recipe, an ingredient you cannot see, a restaurant, a chef, or a dietary claim such as vegan, gluten-free or organic.",
	},
	{
		name = "Night & Astro",
		instruction = "You are an astrophotographer and night-sky guide cataloguing low-light and celestial work. "
			.. "Name what is in the sky as precisely as the frame supports: the Milky Way core, a named constellation, a planet, the Moon and its phase, an aurora, noctilucent clouds, a meteor, star trails, an eclipse. "
			.. "Distinguish the technique honestly — single exposure, tracked, stacked, star trail, light-painted, blue hour blend, time blend — and name the foreground and the setting that anchors the frame. "
			.. "Note the conditions where they are visible: sky darkness, airglow, moonlight, light pollution on the horizon, thin cloud, haze. "
			.. "Use the photo's capture time and place to keep the sky plausible, and identify an object only when you are genuinely confident; describe it as a bright star or an unidentified object rather than naming one you cannot verify. "
			.. "Never invent a deep-sky designation, a catalogue number, an exposure time or an event such as a named meteor shower or a comet.",
	},
}
-- The built-in prompts as the `name -> instruction` table `prefs.prompts`
-- holds. Used by "Reset to defaults", which restores every built-in rather
-- than only `Default` -- a reset that quietly dropped the other preset would
-- be a reset that removes a feature.
function Defaults.builtinPromptTable()
	local prompts = {}
	for _, preset in ipairs(Defaults.builtinPrompts) do
		prompts[preset.name] = preset.instruction
	end
	return prompts
end

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
				name = "YuNet (face detection)",
				author = "Shiqi Yu & Wei Wu, via OpenCV Model Zoo",
				url = "https://github.com/opencv/opencv_zoo",
			},
			{
				name = "FaceNet / Inception-ResNet-v1 (VGGFace2)",
				author = "Tim Esler (facenet-pytorch)",
				url = "https://github.com/timesler/facenet-pytorch",
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
