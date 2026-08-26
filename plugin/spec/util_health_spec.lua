-- Unit tests for the provider side of the system-check preflight.
--
-- `Util.checkPluginHealth` and the Plug-in Manager's System Health panel both
-- ask "is any LLM provider configured?". They used to answer it with two
-- hand-written conditions, and drifted: the panel counted the backend's
-- built-in engine, the preflight did not. Users whose only provider was a
-- downloaded local model were told by the panel that everything looked good
-- and by every task that they had no provider at all (issue #313).
--
-- Both now call SearchIndexAPI.hasAnyLlmProvider, which is pure and tested here.
--
-- Run from the repo root with:  busted

local SearchIndexAPI = require("APISearchIndex")
local Util = require("Util")

--- A health table as SearchIndexAPI.getDetailedHealth() returns it, with the
--- named providers switched on.
local function health(available)
	local h = {
		backend = true,
		clip = true,
		gemini = false,
		chatgpt = false,
		ollama = false,
		lmstudio = false,
		localEngine = false,
	}
	for _, name in ipairs(available or {}) do
		h[name] = true
	end
	return h
end

describe("SearchIndexAPI.hasAnyLlmProvider", function()
	it("counts the backend's built-in engine as a provider", function()
		-- The regression behind issue #313: a Windows user with only a
		-- downloaded llama.cpp model, no API keys, no Ollama, no LM Studio.
		assert.is_true(SearchIndexAPI.hasAnyLlmProvider(health({ "localEngine" })))
	end)

	it("counts each cloud or local-app provider on its own", function()
		assert.is_true(SearchIndexAPI.hasAnyLlmProvider(health({ "gemini" })))
		assert.is_true(SearchIndexAPI.hasAnyLlmProvider(health({ "chatgpt" })))
		assert.is_true(SearchIndexAPI.hasAnyLlmProvider(health({ "ollama" })))
		assert.is_true(SearchIndexAPI.hasAnyLlmProvider(health({ "lmstudio" })))
	end)

	it("reports no provider only when every one of them is off", function()
		assert.is_false(SearchIndexAPI.hasAnyLlmProvider(health()))
	end)

	it("ignores non-provider health fields", function()
		-- A reachable backend with a loaded CLIP model is not an LLM provider:
		-- it generates embeddings, not keywords or descriptions.
		assert.is_false(SearchIndexAPI.hasAnyLlmProvider({ backend = true, clip = true }))
	end)

	it("treats a missing or malformed health table as no provider", function()
		assert.is_false(SearchIndexAPI.hasAnyLlmProvider(nil))
		assert.is_false(SearchIndexAPI.hasAnyLlmProvider("healthy"))
	end)
end)

describe("Util.errorText", function()
	it("passes a real message through unchanged", function()
		assert.are.equal("Model failed to load", Util.errorText("Model failed to load", "fallback"))
	end)

	it("falls back when the value is an empty or blank string", function()
		-- The bug this exists for: `err or "fallback"` never fires for "",
		-- because the empty string is truthy in Lua. The user got a dialog with
		-- a blank line where the reason should have been.
		assert.are.equal("fallback", Util.errorText("", "fallback"))
		assert.are.equal("fallback", Util.errorText("   ", "fallback"))
		assert.are.equal("fallback", Util.errorText("\t\n", "fallback"))
	end)

	it("falls back when there is no value at all", function()
		assert.are.equal("fallback", Util.errorText(nil, "fallback"))
	end)

	it("falls back rather than rendering a table as 'table: 0x...'", function()
		assert.are.equal("fallback", Util.errorText({ error = "nested" }, "fallback"))
	end)

	it("keeps a numeric error value, which still tells the user something", function()
		assert.are.equal("500", Util.errorText(500, "fallback"))
	end)

	it("has a usable fallback of its own when the caller gives none", function()
		local text = Util.errorText(nil)
		assert.is_true(type(text) == "string" and #text > 0)
	end)
end)
