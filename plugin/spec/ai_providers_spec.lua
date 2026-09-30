-- Unit tests for AiProviders: the model picker, per-provider connection
-- fields, the Other AI server status line, and the one-time move of the old
-- Ollama / LM Studio address settings.
--
-- Run from the repo root with:  busted

local AiProviders = require("AiProviders")

local DEFAULTS = {
	defaultOllamaBaseUrl = "http://localhost:11434",
	defaultLmStudioBaseUrl = "localhost:1234",
}

local function titles(items)
	local out = {}
	for _, item in ipairs(items) do
		table.insert(out, item.title)
	end
	return out
end

local function values(items)
	local out = {}
	for _, item in ipairs(items) do
		table.insert(out, item.value)
	end
	return out
end

describe("AiProviders.splitModelKey", function()
	it("splits provider and model on the first separator", function()
		local provider, model = AiProviders.splitModelKey("openai_compatible::google/gemma-3-27b-it:free")
		assert.are.equal("openai_compatible", provider)
		assert.are.equal("google/gemma-3-27b-it:free", model)
	end)

	it("reads an empty model as none", function()
		local provider, model = AiProviders.splitModelKey("mlx::")
		assert.are.equal("mlx", provider)
		assert.is_nil(model)
		assert.is_nil((AiProviders.splitModelKey("")))
		assert.is_nil((AiProviders.splitModelKey(nil)))
	end)
end)

describe("AiProviders.modelItems", function()
	local resp = {
		models = {
			gemini = { "gemini-2.5-flash" },
			chatgpt = { "gpt-5-mini" },
			anthropic = { "claude-opus-5-5" },
			openai_compatible = { "z-model", "a-model" },
			mlx = { "gemma-4-e4b-it-4bit" },
			ollama = {},
			lmstudio = { "qwen2.5-vl-7b" },
		},
		servers = { openai_compatible = { label = "OpenRouter" } },
	}

	it("orders this computer first, then local apps, the own server, then the cloud", function()
		assert.are.same({
			"On this Mac · gemma-4-e4b-it-4bit",
			"LM Studio · qwen2.5-vl-7b",
			"OpenRouter · a-model",
			"OpenRouter · z-model",
			"OpenAI · gpt-5-mini",
			"Google Gemini · gemini-2.5-flash",
			"Anthropic · claude-opus-5-5",
		}, titles(AiProviders.modelItems(resp)))
	end)

	it("keeps provider::model as the value", function()
		local items = AiProviders.modelItems(resp)
		assert.are.equal("mlx::gemma-4-e4b-it-4bit", items[1].value)
	end)

	it("says so when nothing is available, instead of inventing a provider", function()
		local items = AiProviders.modelItems({ models = {} })
		assert.are.same({ AiProviders.NO_MODEL_TITLE }, titles(items))
		assert.are.same({ "" }, values(items))
		local none = AiProviders.modelItems(nil, nil, { emptyTitle = "None (similarity only)" })
		assert.are.same({ "None (similarity only)" }, titles(none))
	end)

	it("keeps a saved choice that is not offered right now, marked as such", function()
		local items = AiProviders.modelItems(resp, "ollama::llava:13b")
		assert.are.equal("ollama::llava:13b", items[1].value)
		assert.are.equal("Ollama · llava:13b (not available now)", items[1].title)
		assert.are.equal(8, #items)
	end)

	it("does not duplicate a saved choice that is offered", function()
		assert.are.equal(7, #AiProviders.modelItems(resp, "chatgpt::gpt-5-mini"))
	end)

	it("lists providers it does not know yet instead of hiding them", function()
		local items = AiProviders.modelItems({ models = { newthing = { "m" } } })
		assert.are.same({ "newthing · m" }, titles(items))
	end)
end)

describe("AiProviders.unavailableReason", function()
	local resp = { models = { ollama = { "llava:13b" } } }

	it("is nil for a model that is offered", function()
		assert.is_nil(AiProviders.unavailableReason(resp, "ollama::llava:13b"))
	end)

	it("explains an empty choice", function()
		assert.truthy(AiProviders.unavailableReason(resp, ""):find("No AI model is set up", 1, true))
	end)

	it("tells the user what to do for each kind of provider", function()
		assert.truthy(AiProviders.unavailableReason(resp, "lmstudio::qwen"):find("Start LM Studio", 1, true))
		assert.truthy(AiProviders.unavailableReason(resp, "openai_compatible::x"):find("Other AI server", 1, true))
	end)

	it("does not refuse what it cannot judge", function()
		-- No list at all: the backend did not answer; the run will say why.
		assert.is_nil(AiProviders.unavailableReason(nil, "mlx::gemma-4-e4b-it-4bit"))
		-- A cloud list is empty on any hiccup; the key is checked instead.
		assert.is_nil(AiProviders.unavailableReason(resp, "chatgpt::gpt-5-mini"))
		assert.is_nil(AiProviders.unavailableReason(resp, "gemini::gemini-2.5-flash"))
		assert.is_nil(AiProviders.unavailableReason(resp, "anthropic::claude-opus-5-5"))
	end)

	it("passes on why the Other AI server could not be asked", function()
		local reason = AiProviders.unavailableReason({
			models = { openai_compatible = {} },
			warnings = { "Other AI server: OpenRouter rejected the API key." },
		}, "openai_compatible::google/gemini-2.5-flash")
		assert.truthy(reason:find("rejected the API key", 1, true))
	end)
end)

describe("AiProviders.resolveSavedKey", function()
	local resp = {
		models = { mlx = { "gemma-3-12b-it-qat-4bit" } },
		aliases = { mlx = { ["4f1c9e"] = "gemma-3-12b-it-qat-4bit" } },
	}

	it("maps an old Hugging Face cache hash onto the model's name", function()
		local key = AiProviders.resolveSavedKey(resp, "mlx::4f1c9e")
		assert.are.equal("mlx::gemma-3-12b-it-qat-4bit", key)
		assert.is_nil(AiProviders.unavailableReason(resp, key))
		assert.are.equal(1, #AiProviders.modelItems(resp, key))
	end)

	it("leaves every other choice alone", function()
		assert.are.equal("mlx::other", AiProviders.resolveSavedKey(resp, "mlx::other"))
		assert.are.equal("ollama::x", AiProviders.resolveSavedKey(resp, "ollama::x"))
		assert.is_nil(AiProviders.resolveSavedKey(resp, nil))
		assert.are.equal("mlx::4f1c9e", AiProviders.resolveSavedKey(nil, "mlx::4f1c9e"))
	end)
end)

describe("AiProviders.connectionOptions", function()
	it("sends the key of the provider in use, trimmed", function()
		local opts = AiProviders.connectionOptions("chatgpt", { chatgptApiKey = " sk-1 ", geminiApiKey = "g" })
		assert.are.same({ api_key = "sk-1" }, opts)
		assert.are.same({ api_key = "g" }, AiProviders.connectionOptions("gemini", { geminiApiKey = "g" }))
		assert.are.same(
			{ api_key = "sk-ant" },
			AiProviders.connectionOptions("anthropic", { anthropicApiKey = " sk-ant ", chatgptApiKey = "sk-1" })
		)
	end)

	it("refuses a cloud provider without a key, with a reason", function()
		local opts, err = AiProviders.connectionOptions("gemini", { geminiApiKey = "  " })
		assert.is_nil(opts)
		assert.truthy(err:find("Gemini API key is not set", 1, true))
		opts, err = AiProviders.connectionOptions("anthropic", {})
		assert.is_nil(opts)
		assert.truthy(err:find("Anthropic API key is not set", 1, true))
	end)

	it("sends the Other AI server address, and its key only when there is one", function()
		assert.are.same(
			{ server_url = "https://openrouter.ai/api/v1", api_key = "sk-or" },
			AiProviders.connectionOptions(
				"openai_compatible",
				{ aiServerUrl = " https://openrouter.ai/api/v1 ", aiServerApiKey = "sk-or" }
			)
		)
		assert.are.same(
			{ server_url = "192.168.1.5:8080" },
			AiProviders.connectionOptions(
				"openai_compatible",
				{ aiServerUrl = "192.168.1.5:8080", aiServerApiKey = "" }
			)
		)
		local opts, err = AiProviders.connectionOptions("openai_compatible", { aiServerUrl = "" })
		assert.is_nil(opts)
		assert.truthy(err:find("Other AI server", 1, true))
	end)

	it("needs nothing for the apps and engines on this computer", function()
		for _, provider in ipairs({ "ollama", "lmstudio", "mlx", "llamacpp" }) do
			assert.are.same({}, AiProviders.connectionOptions(provider, { chatgptApiKey = "sk" }))
		end
	end)
end)

describe("AiProviders.describeServerStatus", function()
	it("shows the hint while no address is set", function()
		local text, state = AiProviders.describeServerStatus("", nil)
		assert.are.equal(AiProviders.SERVER_HINT, text)
		assert.are.equal("hint", state)
	end)

	it("counts the models that can read photos", function()
		local text, state = AiProviders.describeServerStatus("openrouter.ai/api/v1", {
			models = { openai_compatible = { "a", "b" } },
			servers = { openai_compatible = { label = "OpenRouter" } },
			warnings = {},
		})
		assert.are.equal("OpenRouter: 2 models that can read photos", text)
		assert.are.equal("ok", state)
	end)

	it("shows the server's own error", function()
		local text, state = AiProviders.describeServerStatus("x", {
			models = { openai_compatible = {} },
			warnings = { "Other AI server: OpenRouter rejected the API key." },
		})
		assert.truthy(text:find("rejected the API key", 1, true))
		assert.are.equal("error", state)
	end)

	it("flags a server that offers nothing usable", function()
		local text, state = AiProviders.describeServerStatus("x", { models = { openai_compatible = {} } })
		assert.truthy(text:find("no model that can read photos", 1, true))
		assert.are.equal("error", state)
	end)
end)

describe("AiProviders.migrateLegacyPrefs", function()
	it("leaves default addresses alone, and only runs once", function()
		local p = { ollamaBaseUrl = "http://localhost:11434/", lmstudioBaseUrl = "http://127.0.0.1:1234/v1" }
		assert.is_nil(AiProviders.migrateLegacyPrefs(p, DEFAULTS))
		assert.are.equal(2, p.providerPrefsVersion)
		assert.is_nil(p.aiServerUrl)
		p.providerPrefsVersion = 2
		p.lmstudioBaseUrl = "192.168.1.5:1234"
		assert.is_nil(AiProviders.migrateLegacyPrefs(p, DEFAULTS))
	end)

	it("moves a changed LM Studio address and the choices that used it", function()
		local p = {
			lmstudioBaseUrl = "192.168.1.5:1234",
			modelKey = "lmstudio::qwen2.5-vl-7b",
			deduplicateModelKey = "chatgpt::gpt-5-mini",
		}
		local notice = AiProviders.migrateLegacyPrefs(p, DEFAULTS)
		assert.are.equal("192.168.1.5:1234", p.aiServerUrl)
		assert.are.equal("openai_compatible::qwen2.5-vl-7b", p.modelKey)
		assert.are.equal("chatgpt::gpt-5-mini", p.deduplicateModelKey)
		assert.truthy(notice:find("LM Studio address", 1, true))
		-- Kept, so going back to an older plug-in still works.
		assert.are.equal("192.168.1.5:1234", p.lmstudioBaseUrl)
	end)

	it("moves a changed Ollama address", function()
		local p = { ollamaBaseUrl = "http://nas.local:11434", modelKey = "ollama::llava:13b" }
		AiProviders.migrateLegacyPrefs(p, DEFAULTS)
		assert.are.equal("http://nas.local:11434", p.aiServerUrl)
		assert.are.equal("openai_compatible::llava:13b", p.modelKey)
	end)

	it("with both changed, follows the app in use and says what was not kept", function()
		local p = {
			ollamaBaseUrl = "http://nas.local:11434",
			lmstudioBaseUrl = "192.168.1.5:1234",
			modelKey = "ollama::llava:13b",
		}
		local notice = AiProviders.migrateLegacyPrefs(p, DEFAULTS)
		assert.are.equal("http://nas.local:11434", p.aiServerUrl)
		assert.truthy(notice:find("192.168.1.5:1234", 1, true))
		assert.truthy(notice:find("could not be kept", 1, true))
	end)

	-- The case that crashed plug-in start-up: a changed Ollama address, a
	-- cloud model in use, and no keyword-dedup choice. An unset choice left a
	-- nil hole that hid the fallbacks from ipairs.
	it("moves the address when the model in use is another provider's", function()
		local p = { ollamaBaseUrl = "http://192.168.1.10:11434", modelKey = "chatgpt::gpt-4o" }
		local notice = AiProviders.migrateLegacyPrefs(p, DEFAULTS)
		assert.are.equal("http://192.168.1.10:11434", p.aiServerUrl)
		assert.are.equal("chatgpt::gpt-4o", p.modelKey)
		assert.is_nil(p.deduplicateModelKey)
		assert.truthy(notice:find("Ollama address", 1, true))
	end)

	it("moves the address when no model was ever chosen", function()
		for _, modelKey in ipairs({ false, "" }) do
			local p = { lmstudioBaseUrl = "192.168.1.5:1234", modelKey = modelKey or nil }
			AiProviders.migrateLegacyPrefs(p, DEFAULTS)
			assert.are.equal("192.168.1.5:1234", p.aiServerUrl)
		end
	end)

	it("clears the old qwen:: placeholder choice", function()
		local p = { modelKey = "qwen::", deduplicateModelKey = "qwen::" }
		assert.is_nil(AiProviders.migrateLegacyPrefs(p, DEFAULTS))
		assert.are.equal("", p.modelKey)
		assert.are.equal("", p.deduplicateModelKey)
	end)

	it("never overwrites an Other AI server that is already set", function()
		local p = { lmstudioBaseUrl = "192.168.1.5:1234", aiServerUrl = "https://openrouter.ai/api/v1" }
		assert.is_nil(AiProviders.migrateLegacyPrefs(p, DEFAULTS))
		assert.are.equal("https://openrouter.ai/api/v1", p.aiServerUrl)
	end)
end)
