-- Unit tests for Secrets: the API keys in the system's password storage
-- (LrPasswords), the move out of the plug-in prefs, and the session cache.
--
-- Run from the repo root with:  busted

local Secrets = require("Secrets")

-- An LrPasswords double. `opts` breaks it in the ways the real one can fail.
local function fakeStorage(opts)
	opts = opts or {}
	local storage = { data = {}, stores = 0, retrieves = 0 }
	storage.store = function(key, value)
		storage.stores = storage.stores + 1
		if opts.storeFails then
			error("keychain is locked")
		end
		storage.data[key] = opts.mangle and (value .. "?") or value
	end
	storage.retrieve = function(key)
		storage.retrieves = storage.retrieves + 1
		if opts.retrieveFails then
			error("access denied")
		end
		return storage.data[key]
	end
	return storage
end

local messages
local saved = {}

before_each(function()
	saved.prefs, saved.LrDialogs, saved.canYield = _G.prefs, _G.LrDialogs, LrTasks.canYield
	Secrets._reset()
	Secrets.storage = fakeStorage()
	_G.prefs = {}
	messages = {}
	_G.LrDialogs = {
		message = function(title, text, style)
			table.insert(messages, { title = title, text = text, style = style })
		end,
	}
	-- Secrets.saveChanged stores from a task; run it inline.
	_G.LrTasks.canYield = function()
		return true
	end
end)

after_each(function()
	_G.prefs, _G.LrDialogs, _G.LrTasks.canYield = saved.prefs, saved.LrDialogs, saved.canYield
end)

describe("Secrets.migrate", function()
	it("moves a key from prefs to the password storage", function()
		prefs.chatgptApiKey = " sk-1 "
		assert.are.same({}, Secrets.migrate())
		assert.are.equal("sk-1", Secrets.storage.data.chatgptApiKey)
		assert.is_nil(prefs.chatgptApiKey)
		assert.are.equal("sk-1", Secrets.get("chatgptApiKey"))
	end)

	it("keeps the key in prefs, and says so, when the storage refuses it", function()
		Secrets.storage = fakeStorage({ storeFails = true })
		prefs.geminiApiKey = "g-1"
		local problems = Secrets.migrate()
		assert.are.equal("g-1", prefs.geminiApiKey)
		assert.are.equal("g-1", Secrets.get("geminiApiKey"))
		assert.are.equal(1, #problems)
		assert.truthy(problems[1]:find("Google Gemini API key could not be moved", 1, true))
		assert.truthy(problems[1]:find("keychain is locked", 1, true))
		assert.are.same(problems, Secrets.problems())
	end)

	it("keeps the key in prefs when it does not read back unchanged", function()
		Secrets.storage = fakeStorage({ mangle = true })
		prefs.anthropicApiKey = "sk-ant"
		local problems = Secrets.migrate()
		assert.are.equal("sk-ant", prefs.anthropicApiKey)
		assert.truthy(problems[1]:find("did not match", 1, true))
	end)

	it("lets a key in prefs win over the stored one", function()
		-- Going back to an older version and entering a new key there writes
		-- it to prefs; it is the newer one.
		Secrets.storage.data.chatgptApiKey = "sk-old"
		prefs.chatgptApiKey = "sk-new"
		Secrets.migrate()
		assert.are.equal("sk-new", Secrets.storage.data.chatgptApiKey)
		assert.are.equal("sk-new", Secrets.get("chatgptApiKey"))
	end)

	it("drops the empty defaults and the retired apiKey pref", function()
		prefs.chatgptApiKey = ""
		prefs.aiServerApiKey = "  "
		prefs.apiKey = ""
		prefs.aiServerUrl = "https://openrouter.ai/api/v1"
		Secrets.migrate()
		assert.is_nil(prefs.chatgptApiKey)
		assert.is_nil(prefs.aiServerApiKey)
		assert.is_nil(prefs.apiKey)
		assert.are.equal("https://openrouter.ai/api/v1", prefs.aiServerUrl)
		assert.are.equal(0, Secrets.storage.stores)
	end)

	it("reports a stored key it cannot read", function()
		Secrets.storage = fakeStorage({ retrieveFails = true })
		local problems = Secrets.migrate()
		assert.are.equal(#Secrets.FIELDS, #problems)
		assert.truthy(problems[1]:find("could not be read", 1, true))
		assert.truthy(problems[1]:find("access denied", 1, true))
	end)
end)

describe("Secrets.get", function()
	it("reads each key from the storage once per session", function()
		Secrets.storage.data.geminiApiKey = "g-1"
		assert.are.equal("g-1", Secrets.get("geminiApiKey"))
		Secrets.storage.data.geminiApiKey = "g-2"
		assert.are.equal("g-1", Secrets.get("geminiApiKey"))
		assert.are.equal(1, Secrets.storage.retrieves)
	end)

	it("caches a key that is not set too", function()
		assert.are.equal("", Secrets.get("anthropicApiKey"))
		assert.are.equal("", Secrets.get("anthropicApiKey"))
		assert.are.equal(1, Secrets.storage.retrieves)
		assert.is_false(Secrets.has("anthropicApiKey"))
	end)

	it("does not cache a failed read, so a later one can succeed", function()
		local broken = fakeStorage({ retrieveFails = true })
		Secrets.storage = broken
		local value, problem = Secrets.get("chatgptApiKey")
		assert.are.equal("", value)
		assert.truthy(problem:find("OpenAI API key could not be read", 1, true))

		Secrets.storage = fakeStorage()
		Secrets.storage.data.chatgptApiKey = "sk-1"
		assert.are.equal("sk-1", Secrets.get("chatgptApiKey"))
	end)

	it("falls back to a key still in prefs", function()
		Secrets.storage = fakeStorage({ retrieveFails = true })
		prefs.aiServerApiKey = "sk-or"
		local value, problem = Secrets.get("aiServerApiKey")
		assert.are.equal("sk-or", value)
		assert.is_nil(problem)
	end)
end)

describe("Secrets.set", function()
	it("stores a key and removes it from prefs", function()
		prefs.chatgptApiKey = "sk-old"
		assert.is_true(Secrets.set("chatgptApiKey", " sk-new "))
		assert.are.equal("sk-new", Secrets.storage.data.chatgptApiKey)
		assert.is_nil(prefs.chatgptApiKey)
	end)

	it("clears a key by storing an empty one", function()
		Secrets.storage.data.geminiApiKey = "g-1"
		assert.is_true(Secrets.set("geminiApiKey", ""))
		assert.are.equal("", Secrets.storage.data.geminiApiKey)
		assert.is_false(Secrets.has("geminiApiKey"))
	end)

	it("keeps a key it cannot store in prefs, and says where it went", function()
		Secrets.storage = fakeStorage({ storeFails = true })
		local ok, problem = Secrets.set("anthropicApiKey", "sk-ant")
		assert.is_false(ok)
		assert.are.equal("sk-ant", prefs.anthropicApiKey)
		assert.are.equal("sk-ant", Secrets.get("anthropicApiKey"))
		assert.truthy(problem:find("Lightroom's preferences instead, as plain text", 1, true))
	end)

	it("says a key it cannot remove will come back", function()
		Secrets.storage = fakeStorage({ storeFails = true })
		local ok, problem = Secrets.set("chatgptApiKey", "")
		assert.is_false(ok)
		assert.is_nil(prefs.chatgptApiKey)
		assert.truthy(problem:find("could not be removed", 1, true))
	end)
end)

describe("Secrets.loadInto / saveChanged", function()
	it("saves only the keys the user changed", function()
		Secrets.storage.data.chatgptApiKey = "sk-1"
		Secrets.storage.data.geminiApiKey = "g-1"
		local props = {}
		local loaded = Secrets.loadInto(props)
		assert.are.equal("sk-1", props.chatgptApiKey)
		assert.are.equal("", props.aiServerApiKey)

		props.geminiApiKey = " g-2 "
		Secrets.saveChanged(props, loaded)
		assert.are.equal(1, Secrets.storage.stores)
		assert.are.equal("g-2", Secrets.storage.data.geminiApiKey)
		assert.are.equal("sk-1", Secrets.storage.data.chatgptApiKey)
		assert.are.same({}, messages)
	end)

	it("does not delete a key that could not be read and was left alone", function()
		Secrets.storage = fakeStorage({ retrieveFails = true })
		local props = {}
		local loaded = Secrets.loadInto(props, { "chatgptApiKey" })
		assert.are.equal("", props.chatgptApiKey)
		Secrets.saveChanged(props, loaded)
		assert.are.equal(0, Secrets.storage.stores)
	end)

	it("loads only the fields asked for", function()
		local props = {}
		local loaded = Secrets.loadInto(props, { "geminiApiKey" })
		assert.are.same({ geminiApiKey = "" }, loaded)
		assert.is_nil(props.chatgptApiKey)
	end)

	it("tells the user about every key it could not store", function()
		local props = {}
		local loaded = Secrets.loadInto(props)
		Secrets.storage = fakeStorage({ storeFails = true })
		props.chatgptApiKey = "sk-1"
		props.anthropicApiKey = "sk-ant"
		Secrets.saveChanged(props, loaded)
		assert.are.equal(1, #messages)
		assert.are.equal("warning", messages[1].style)
		assert.truthy(messages[1].text:find("OpenAI API key", 1, true))
		assert.truthy(messages[1].text:find("Anthropic API key", 1, true))
	end)

	it("uses a changed key at once, before the store finishes", function()
		_G.LrTasks.canYield = function()
			return false
		end
		local savedStart = LrTasks.startAsyncTask
		local pending
		_G.LrTasks.startAsyncTask = function(fn)
			pending = fn
		end
		local props = {}
		local loaded = Secrets.loadInto(props)
		props.chatgptApiKey = "sk-1"
		Secrets.saveChanged(props, loaded)
		assert.are.equal("sk-1", Secrets.get("chatgptApiKey"))
		assert.are.equal(0, Secrets.storage.stores)
		pending()
		assert.are.equal("sk-1", Secrets.storage.data.chatgptApiKey)
		_G.LrTasks.startAsyncTask = savedStart
	end)
end)

describe("Secrets.connectionFields", function()
	it("combines the server address from prefs with the stored keys", function()
		Secrets.storage.data.aiServerApiKey = "sk-or"
		local fields = Secrets.connectionFields({ aiServerUrl = "https://openrouter.ai/api/v1" })
		assert.are.equal("https://openrouter.ai/api/v1", fields.aiServerUrl)
		assert.are.equal("sk-or", fields.aiServerApiKey)
		assert.are.equal("", fields.chatgptApiKey)
		assert.are.same({}, fields.keyProblems)
	end)

	it("carries why a key could not be read", function()
		Secrets.storage = fakeStorage({ retrieveFails = true })
		local fields = Secrets.connectionFields({})
		assert.truthy(fields.keyProblems.geminiApiKey:find("could not be read", 1, true))
	end)
end)

describe("Secrets agent token", function()
	it("is empty until the backend hands one out, then kept and cached", function()
		assert.are.equal("", Secrets.getMcpToken())
		assert.is_true(Secrets.setMcpToken(" " .. string.rep("a", 64) .. " "))
		assert.are.equal(string.rep("a", 64), Secrets.storage.data[Secrets.MCP_TOKEN])
		local reads = Secrets.storage.retrieves
		assert.are.equal(string.rep("a", 64), Secrets.getMcpToken())
		assert.are.equal(reads, Secrets.storage.retrieves, "served from the cache")
	end)

	it("stays out of the provider keys", function()
		for _, field in ipairs(Secrets.FIELDS) do
			assert.are_not.equal(Secrets.MCP_TOKEN, field)
		end
		Secrets.setMcpToken(string.rep("b", 64))
		local fields = Secrets.connectionFields({})
		for _, value in pairs(fields) do
			assert.are_not.equal(string.rep("b", 64), value)
		end
	end)

	it("still works for the session when the storage refuses it", function()
		Secrets.storage = fakeStorage({ storeFails = true })
		local ok, problem = Secrets.setMcpToken(string.rep("c", 64))
		assert.is_false(ok)
		assert.truthy(problem:find("password storage", 1, true))
		assert.are.equal(string.rep("c", 64), Secrets.getMcpToken())
	end)
end)
