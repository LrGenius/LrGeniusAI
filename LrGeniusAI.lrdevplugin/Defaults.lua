Defaults = {}

Defaults.defaultTopLevelKeywords = {
    "LrGeniusAI",
    "Ollama",
    "LM Studio",
    "ChatGPT",
    "Google Gemini",
}

Defaults.topLevelKeywordSynonym = "LrGeniusAI Top-Level Keyword"

Defaults.defaultGenerateLanguage = "English"

Defaults.generateLanguages = { "English", "German", "French", "Spanish", "Italian" }

Defaults.defaultTemperature = 0.1

Defaults.defaultKeywordCategories = {
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Activities=Activities",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Buildings=Buildings",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Location=Location",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Objects=Objects",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/People=People",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Moods=Moods",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Sceneries=Sceneries",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Texts=Texts",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Companies=Companies",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Weather=Weather",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Plants=Plants",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Animals=Animals",
    LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/keywords/Vehicles=Vehicles",
}

Defaults.targetDataFields = {
    { title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/keywords=Keywords", value = "keyword" },
    { title = LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/ImageTitle=Image title", value = "title" },
    { title = LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/ImageCaption=Image caption", value = "caption" },
    { title = LOC "$$$/lrc-ai-assistant/Defaults/ResponseStructure/ImageAltText=Image Alt Text", value = "altTextAccessibility" },
}

Defaults.exportSizes = {
    "512", "1024", "2048", "3072", "4096"
}

Defaults.pricing = {}
Defaults.pricing["gemini-2.5-pro"] = {}
Defaults.pricing["gemini-2.5-pro"].input = 1.25 / 1000000
Defaults.pricing["gemini-2.5-pro"].output= 10 / 1000000
Defaults.pricing["gemini-2.0-flash"] = {}
Defaults.pricing["gemini-2.0-flash"].input = 0.1 / 1000000
Defaults.pricing["gemini-2.0-flash"].output= 0.4 / 1000000
Defaults.pricing["gemini-2.5-flash"] = {}
Defaults.pricing["gemini-2.5-flash"].input = 0.30 / 1000000
Defaults.pricing["gemini-2.5-flash"].output= 2.5 / 1000000
Defaults.pricing["gemini-2.0-flash-lite"] = {}
Defaults.pricing["gemini-2.0-flash-lite"].input = 0.075 / 1000000
Defaults.pricing["gemini-2.0-flash-lite"].output= 0.3 / 1000000

Defaults.pricing["gpt-4.1"] = {}
Defaults.pricing["gpt-4.1"].input = 2 / 1000000
Defaults.pricing["gpt-4.1"].output= 8 / 1000000
Defaults.pricing["gpt-4.1-mini"] = {}
Defaults.pricing["gpt-4.1-mini"].input = 0.4 / 1000000
Defaults.pricing["gpt-4.1-mini"].output= 1.6 / 1000000
Defaults.pricing["gpt-4.1-nano"] = {}
Defaults.pricing["gpt-4.1-nano"].input = 0.1 / 1000000
Defaults.pricing["gpt-4.1-nano"].output= 0.4 / 1000000

Defaults.pricing["gpt-5"] = {}
Defaults.pricing["gpt-5"].input = 1.25 / 1000000
Defaults.pricing["gpt-5"].output= 10 / 1000000
Defaults.pricing["gpt-5-mini"] = {}
Defaults.pricing["gpt-5-mini"].input = 0.25 / 1000000
Defaults.pricing["gpt-5-mini"].output= 2.0 / 1000000
Defaults.pricing["gpt-5-nano"] = {}
Defaults.pricing["gpt-5-nano"].input = 0.05 / 1000000
Defaults.pricing["gpt-5-nano"].output= 0.4 / 1000000


Defaults.defaultExportQuality = 50
Defaults.defaultExportSize = "3072"

Defaults.defaultSystemInstruction = "You are a professional photography analyst with expertise in object recognition and computer-generated image description. You also try to identify famous buildings and landmarks as well as the location where the photo was taken. Furthermore, you aim to specify animal and plant species as accurately as possible. You also describe objects—such as vehicle types and manufacturers—as specifically as you can."

Defaults.catalogWriteAccessOptions = {
    timeout = 60,  -- seconds
}

Defaults.credits = {
    { name = "JSON.lua by Jeffrey Friedl", author = "Jeffrey Friedl", url = "http://regex.info/blog/lua/json" },
    { name = "timm--ViT-SO400M-16-SigLIP2-384", author = "rwightman", url = "https://huggingface.co/timm/ViT-SO400M-16-SigLIP2-384" },
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
}

Defaults.copyrightString = ""
local f = LrView.osFactory()
for _, credit in ipairs(Defaults.credits) do
    Defaults.copyrightString = Defaults.copyrightString .. string.format("%s (%s)\n", credit.name, credit.url)
end

return Defaults
