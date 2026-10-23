-- The backend's access token: which one the plug-in sends, and what happens
-- when the backend refuses it. Real request code, with the file system, the
-- password storage and HTTP replaced by doubles.
require("APISearchIndex")
local Secrets = require("Secrets")
local json = require("JSON")

describe("the backend's access token", function()
	local saved, files, requests, answers

	local TOKEN_FILE = "/Users/test/Library/Application Support/LrGeniusAI/api-token"
	local FILE_TOKEN = string.rep("f", 64)
	local ENTERED_TOKEN = string.rep("e", 64)

	local function headerValue(headers, field)
		for _, header in ipairs(headers or {}) do
			if header.field:lower() == field:lower() then
				return header.value
			end
		end
	end

	local function authorizations(headers)
		local found = {}
		for _, header in ipairs(headers or {}) do
			if header.field:lower() == "authorization" then
				found[#found + 1] = header.value
			end
		end
		return found
	end

	-- The next answer the fake backend gives; the last one repeats.
	local function respond(headers)
		local answer = table.remove(answers, 1) or answers.last
		answers.last = answer
		return answer(headers)
	end

	local function ok()
		return json:encode({ actions = {} }), { status = 200 }
	end

	local function refused()
		return json:encode({ error = "wrong token", error_code = "api_token" }), { status = 403 }
	end

	before_each(function()
		saved = {}
		for _, name in ipairs({ "LrPathUtils", "LrFileUtils", "LrHttp", "JSON", "prefs" }) do
			saved[name] = { value = _G[name] }
		end
		saved.storage = Secrets.storage
		files, requests, answers = {}, {}, { ok }
		_G.LrPathUtils = {
			getStandardFilePath = function(name)
				assert.are.equal("home", name)
				return "/Users/test"
			end,
			child = function(parent, leaf)
				return parent .. "/" .. leaf
			end,
		}
		_G.LrFileUtils = {
			exists = function(path)
				return files[path] and "file" or false
			end,
			readFile = function(path)
				return files[path]
			end,
		}
		_G.LrHttp = {
			get = function(url, headers)
				requests[#requests + 1] = { method = "GET", url = url, headers = headers }
				return respond(headers)
			end,
			post = function(url, body, headers)
				requests[#requests + 1] = { method = "POST", url = url, body = body, headers = headers }
				return respond(headers)
			end,
		}
		_G.JSON = json
		_G.prefs = { backendServerUrl = "http://127.0.0.1:19819", dbStoragePath = "/data" }
		local stored = {}
		Secrets.storage = {
			store = function(key, value)
				stored[key] = value
			end,
			retrieve = function(key)
				return stored[key]
			end,
		}
		Secrets._reset()
		SearchIndexAPI._forgetLocalToken()
	end)

	after_each(function()
		for name, original in pairs(saved) do
			if name ~= "storage" then
				_G[name] = original.value
			end
		end
		Secrets.storage = saved.storage
		Secrets._reset()
		SearchIndexAPI._forgetLocalToken()
	end)

	it("sends the token from the local backend's file", function()
		files[TOKEN_FILE] = FILE_TOKEN .. "\n"
		assert.is_table(SearchIndexAPI.takeUiActions())
		assert.are.equal("Bearer " .. FILE_TOKEN, headerValue(requests[1].headers, "Authorization"))
	end)

	it("sends the entered token to a backend on another computer", function()
		_G.prefs.backendServerUrl = "http://nas.local:19819"
		files[TOKEN_FILE] = FILE_TOKEN
		Secrets.setBackendToken(ENTERED_TOKEN)
		assert.is_table(SearchIndexAPI.takeUiActions())
		assert.are.equal("Bearer " .. ENTERED_TOKEN, headerValue(requests[1].headers, "Authorization"))
	end)

	it("falls back to the entered token when this computer has no file (a container)", function()
		Secrets.setBackendToken(ENTERED_TOKEN)
		assert.is_table(SearchIndexAPI.takeUiActions())
		assert.are.equal("Bearer " .. ENTERED_TOKEN, headerValue(requests[1].headers, "Authorization"))
	end)

	it("sends nothing when there is no token, for an older backend", function()
		assert.is_table(SearchIndexAPI.takeUiActions())
		assert.same({}, authorizations(requests[1].headers))
	end)

	it("leaves a caller's own Authorization header alone", function()
		files[TOKEN_FILE] = FILE_TOKEN
		SearchIndexAPI.openChatSession("agent-token", { model = "m" })
		assert.same({ "Bearer agent-token" }, authorizations(requests[1].headers))
	end)

	it("reads the file again and retries once when the backend refuses a stale token", function()
		files[TOKEN_FILE] = FILE_TOKEN
		assert.is_table(SearchIndexAPI.takeUiActions())

		local renewed = string.rep("n", 64)
		files[TOKEN_FILE] = renewed
		answers = { refused, ok }
		requests = {}
		assert.is_table(SearchIndexAPI.takeUiActions())
		assert.are.equal(2, #requests)
		assert.are.equal("Bearer " .. FILE_TOKEN, headerValue(requests[1].headers, "Authorization"))
		assert.are.equal("Bearer " .. renewed, headerValue(requests[2].headers, "Authorization"))
	end)

	it("tells the user what to do when a remote backend refuses", function()
		_G.prefs.backendServerUrl = "http://nas.local:19819"
		answers = { refused }
		local result, err = SearchIndexAPI.takeUiActions()
		assert.is_nil(result)
		assert.are.equal(1, #requests, "nothing new to retry with")
		assert.truthy(err:find("geniusai-server token", 1, true))
		assert.truthy(err:find("Access token", 1, true))
	end)

	it("hands the People page the token in the fragment only", function()
		files[TOKEN_FILE] = FILE_TOKEN
		local url = SearchIndexAPI.getPeopleUiUrl()
		assert.truthy(url:find("#token=" .. FILE_TOKEN, 1, true))
		assert.is_nil(url:match("^[^#]*" .. FILE_TOKEN), "the token must not be in the part sent to the server")
	end)
end)
