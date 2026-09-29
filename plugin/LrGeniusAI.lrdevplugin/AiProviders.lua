---
-- The AI providers as the plug-in presents them: the items of the model
-- picker, the connection fields a request for a provider needs, and the
-- one-time move of the old Ollama / LM Studio address settings.
--
-- Two task dialogs used to build the picker and pick the API key with their
-- own copies of the same code, and both had to learn about every provider
-- separately. Everything provider-specific now lives here. It is pure Lua —
-- no Lightroom SDK calls — so all of it is covered by busted specs.
--
AiProviders = {}

-- Order of the model picker: this computer first, then apps running on it,
-- then the user's own server, then the paid cloud services.
local ORDER = { "mlx", "llamacpp", "ollama", "lmstudio", "openai_compatible", "chatgpt", "gemini" }

local LABELS = {
	mlx = "On this Mac",
	ollama = "Ollama",
	lmstudio = "LM Studio",
	openai_compatible = "Other AI server",
	chatgpt = "OpenAI",
	gemini = "Google Gemini",
}

--- Shown as the only picker item when no provider offers a model.
AiProviders.NO_MODEL_TITLE = "No AI model set up yet — see Plug-in Manager"

--- Shown under the "Other AI server" field while it is empty.
AiProviders.SERVER_HINT = "Optional: OpenRouter, a llama.cpp server, LiteLLM, or LM Studio / Ollama on another "
	.. "computer. Leave the API key empty if the server needs none."

local function blank(value)
	return value == nil or (type(value) == "string" and value:match("^%s*$") ~= nil)
end

local function trim(value)
	return (tostring(value):gsub("^%s+", ""):gsub("%s+$", ""))
end

---
-- Splits a stored model choice ("provider::model").
--
-- @param key string|nil
-- @return string|nil provider
-- @return string|nil model, nil when the key names no model
--
function AiProviders.splitModelKey(key)
	if blank(key) then
		return nil, nil
	end
	local sep = string.find(key, "::", 1, true)
	if not sep then
		return key, nil
	end
	local provider = string.sub(key, 1, sep - 1)
	local model = string.sub(key, sep + 2)
	if model == "" then
		model = nil
	end
	return provider, model
end

---
-- The name a provider is shown under.
--
-- @param provider string Wire name, e.g. "openai_compatible".
-- @param resp table|nil The /v1/llm/providers/models response, for the user's
--        server label ("OpenRouter", or its host and port).
--
function AiProviders.label(provider, resp)
	if provider == "llamacpp" then
		return MAC_ENV and "On this Mac" or "On this PC"
	end
	if provider == "openai_compatible" then
		local server = type(resp) == "table" and type(resp.servers) == "table" and resp.servers.openai_compatible
		if type(server) == "table" and not blank(server.label) then
			return server.label
		end
	end
	return LABELS[provider] or tostring(provider)
end

-- The providers of a /models response in picker order; any the plug-in does
-- not know yet come last, alphabetically, rather than being hidden.
local function providersInOrder(models)
	local ordered, known = {}, {}
	for _, provider in ipairs(ORDER) do
		known[provider] = true
		if type(models[provider]) == "table" then
			table.insert(ordered, provider)
		end
	end
	local extra = {}
	for provider, list in pairs(models) do
		if not known[provider] and type(list) == "table" then
			table.insert(extra, provider)
		end
	end
	table.sort(extra)
	for _, provider in ipairs(extra) do
		table.insert(ordered, provider)
	end
	return ordered
end

---
-- Builds the model picker's items from a /v1/llm/providers/models response.
--
-- Titles read "<provider label> · <model>"; values are "provider::model".
-- With nothing to offer, the single item says so (value ""), so a dialog never
-- falls back to a provider that does not exist. A saved choice that is not
-- offered right now — Ollama not running, a key removed — stays in the list,
-- marked "(not available now)" and selectable, so that opening the dialog
-- never quietly switches a free local run to a paid cloud model.
--
-- @param resp table|nil The response; nil when the backend did not answer.
-- @param savedKey string|nil The stored choice.
-- @param options table|nil { emptyTitle = string } overrides NO_MODEL_TITLE.
-- @return table items { { title, value }, ... }
--
function AiProviders.modelItems(resp, savedKey, options)
	options = options or {}
	local items = {}
	local models = type(resp) == "table" and type(resp.models) == "table" and resp.models or {}
	for _, provider in ipairs(providersInOrder(models)) do
		local list = {}
		for _, model in ipairs(models[provider]) do
			table.insert(list, tostring(model))
		end
		table.sort(list, function(a, b)
			return a:lower() < b:lower()
		end)
		local label = AiProviders.label(provider, resp)
		for _, model in ipairs(list) do
			table.insert(items, { title = label .. " · " .. model, value = provider .. "::" .. model })
		end
	end

	if #items == 0 then
		table.insert(items, { title = options.emptyTitle or AiProviders.NO_MODEL_TITLE, value = "" })
	end

	if not blank(savedKey) then
		for _, item in ipairs(items) do
			if item.value == savedKey then
				return items
			end
		end
		local provider, model = AiProviders.splitModelKey(savedKey)
		table.insert(items, 1, {
			title = AiProviders.label(provider, resp) .. " · " .. (model or "?") .. " (not available now)",
			value = savedKey,
		})
	end
	return items
end

---
-- Maps a saved choice onto the name its model is offered under now.
--
-- MLX models from the Hugging Face cache used to be listed under their
-- snapshot hash; the backend reports those old names in `aliases`. Without
-- this, a model that is installed and works would read "(not available now)".
--
-- @param resp table|nil The /v1/llm/providers/models response.
-- @param key string|nil The saved "provider::model".
-- @return string|nil The key to use.
--
function AiProviders.resolveSavedKey(resp, key)
	local provider, model = AiProviders.splitModelKey(key)
	if not provider or not model or type(resp) ~= "table" or type(resp.aliases) ~= "table" then
		return key
	end
	local aliases = resp.aliases[provider]
	local current = type(aliases) == "table" and aliases[model]
	if type(current) == "string" and current ~= "" then
		return provider .. "::" .. current
	end
	return key
end

---
-- The choice a picker should start on: the saved one, else the first item.
--
function AiProviders.initialKey(items, savedKey)
	if not blank(savedKey) then
		return savedKey
	end
	return items[1] and items[1].value or ""
end

---
-- Why a chosen model cannot be used right now, or nil when it can.
--
-- Only as sure as the model list it is given. With no list at all (the
-- backend did not answer in time), nothing is refused here: the run itself
-- reports what is wrong. OpenAI and Gemini are not gated on their list
-- either — a brief hiccup there empties it, and refusing a paid run the user
-- set up on that basis would be wrong; their key is still required.
--
-- @param resp table|nil The /models response the picker was built from.
-- @param key string|nil The chosen "provider::model".
-- @return string|nil A message for the user.
--
function AiProviders.unavailableReason(resp, key)
	if blank(key) then
		return "No AI model is set up yet. Download a local model, start Ollama or LM Studio, or add a key or "
			.. "server under Plug-in Manager → Optional AI providers."
	end
	local provider, model = AiProviders.splitModelKey(key)
	if type(resp) ~= "table" or type(resp.models) ~= "table" then
		return nil
	end
	if provider == "chatgpt" or provider == "gemini" then
		return nil
	end
	for _, offered in ipairs(resp.models[provider] or {}) do
		if offered == model then
			return nil
		end
	end
	local name = AiProviders.label(provider, resp) .. " · " .. (model or "?")
	local fix
	if provider == "ollama" or provider == "lmstudio" then
		fix = "Start " .. AiProviders.label(provider) .. " and try again, or pick another model."
	elseif provider == "openai_compatible" then
		fix = "Check the Other AI server under Plug-in Manager → Optional AI providers, or pick another model."
		-- The backend says why the server could not be asked, when it knows.
		for _, warning in ipairs(resp.warnings or {}) do
			if tostring(warning):find("^Other AI server") then
				fix = tostring(warning)
				break
			end
		end
	else
		fix = "Download it again in Plug-in Manager, or pick another model."
	end
	return name .. " is not available right now. " .. fix
end

---
-- The connection fields a request for `provider` has to carry.
--
-- Ollama and LM Studio need none: they are always reached at their default
-- address on this computer. One on another machine is the Other AI server.
--
-- @param provider string
-- @param p table The plug-in prefs.
-- @return table|nil { api_key?, server_url? }
-- @return string|nil Why the provider cannot be used, when the table is nil.
--
function AiProviders.connectionOptions(provider, p)
	p = p or {}
	local opts = {}
	if provider == "chatgpt" then
		if blank(p.chatgptApiKey) then
			return nil, "The OpenAI API key is not set. Enter it under Plug-in Manager → Optional AI providers."
		end
		opts.api_key = trim(p.chatgptApiKey)
	elseif provider == "gemini" then
		if blank(p.geminiApiKey) then
			return nil,
				"The Google Gemini API key is not set. Enter it under Plug-in Manager → Optional AI providers."
		end
		opts.api_key = trim(p.geminiApiKey)
	elseif provider == "openai_compatible" then
		if blank(p.aiServerUrl) then
			return nil,
				"No AI server address is set. Enter it under Plug-in Manager → Optional AI providers → Other AI server."
		end
		opts.server_url = trim(p.aiServerUrl)
		if not blank(p.aiServerApiKey) then
			opts.api_key = trim(p.aiServerApiKey)
		end
	end
	return opts
end

---
-- The line shown under the Other AI server fields.
--
-- @param serverUrl string|nil The address as entered.
-- @param resp table|nil /models response for that address; nil if none.
-- @param err string|nil Why there is no response.
-- @return string text
-- @return string state "hint" | "ok" | "error"
--
function AiProviders.describeServerStatus(serverUrl, resp, err)
	if blank(serverUrl) then
		return AiProviders.SERVER_HINT, "hint"
	end
	if type(resp) ~= "table" then
		return "Could not check the server: " .. (err or "the backend is not running."), "error"
	end
	for _, warning in ipairs(resp.warnings or {}) do
		if tostring(warning):find("^Other AI server") then
			return tostring(warning), "error"
		end
	end
	local models = type(resp.models) == "table" and resp.models.openai_compatible or {}
	local count = type(models) == "table" and #models or 0
	local label = AiProviders.label("openai_compatible", resp)
	if count == 0 then
		return label .. " answered, but offers no model that can read photos.", "error"
	end
	return label .. ": " .. count .. (count == 1 and " model" or " models") .. " that can read photos", "ok"
end

-- An address counts as the app's default when it names the same host and
-- port, however it is spelled: with or without scheme, trailing slash or /v1,
-- and localhost or 127.0.0.1.
local function sameAddress(a, b)
	local function norm(value)
		local s = trim(value):lower()
		s = s:gsub("^https?://", "")
		s = s:gsub("/+$", "")
		s = s:gsub("/v1$", "")
		s = s:gsub("^127%.0%.0%.1", "localhost")
		return s
	end
	return norm(a) == norm(b)
end

local function isCustomAddress(value, default)
	return not blank(value) and not sameAddress(value, default or "")
end

---
-- Moves a changed Ollama or LM Studio address to the Other AI server, once.
--
-- Both apps used to have an address field. They are found automatically at
-- their default address now, so an address the user had changed can only
-- mean the app runs somewhere else — which is what the Other AI server is
-- for. The stored model choices that pointed at that app follow it. The old
-- prefs are left in place, so going back to an older plug-in still works.
--
-- @param p table The plug-in prefs (modified).
-- @param defaults table Holds defaultOllamaBaseUrl and defaultLmStudioBaseUrl.
-- @return string|nil A notice for the user, when something was moved.
--
function AiProviders.migrateLegacyPrefs(p, defaults)
	if (tonumber(p.providerPrefsVersion) or 1) >= 2 then
		return nil
	end
	p.providerPrefsVersion = 2

	-- The old pickers stored "qwen::" when nothing was available — a provider
	-- the backend never had. For keyword dedup it meant "similarity only",
	-- which is what an empty choice means now.
	for _, field in ipairs({ "modelKey", "deduplicateModelKey" }) do
		if AiProviders.splitModelKey(p[field]) == "qwen" then
			p[field] = ""
		end
	end

	local custom = {}
	if isCustomAddress(p.ollamaBaseUrl, defaults.defaultOllamaBaseUrl) then
		custom.ollama = trim(p.ollamaBaseUrl)
	end
	if isCustomAddress(p.lmstudioBaseUrl, defaults.defaultLmStudioBaseUrl) then
		custom.lmstudio = trim(p.lmstudioBaseUrl)
	end
	if not custom.ollama and not custom.lmstudio then
		return nil
	end
	if not blank(p.aiServerUrl) then
		-- Already set up, e.g. after going back to an older version and forward again.
		return nil
	end

	-- The app the user actually runs with wins; otherwise LM Studio. Built
	-- without nil holes: `ipairs` stops at the first nil, and an unset model
	-- choice must not hide the fallbacks behind it.
	local chosen
	local candidates = {}
	for _, key in ipairs({ "modelKey", "deduplicateModelKey" }) do
		local provider = AiProviders.splitModelKey(p[key])
		if provider then
			table.insert(candidates, provider)
		end
	end
	table.insert(candidates, "lmstudio")
	table.insert(candidates, "ollama")
	for _, candidate in ipairs(candidates) do
		if custom[candidate] then
			chosen = candidate
			break
		end
	end

	p.aiServerUrl = custom[chosen]
	for _, field in ipairs({ "modelKey", "deduplicateModelKey" }) do
		local provider, model = AiProviders.splitModelKey(p[field])
		if provider == chosen then
			p[field] = "openai_compatible::" .. (model or "")
		end
	end

	local name = LABELS[chosen]
	local notice = "Your "
		.. name
		.. " address ("
		.. custom[chosen]
		.. ") is now the Other AI server under Optional AI providers."
	local other = chosen == "ollama" and "lmstudio" or "ollama"
	if custom[other] then
		notice = notice
			.. " Your "
			.. LABELS[other]
			.. " address ("
			.. custom[other]
			.. ") could not be kept as well — enter it there instead if that is the one you use."
	end
	return notice
end

return AiProviders
