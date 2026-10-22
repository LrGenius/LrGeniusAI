--[[
    Chat with Your Catalog: opens the agent chat in the browser.

    The chat is the backend's /v1/ui/chat page. Its model works the catalog
    through the agent bridge (AgentBridge.lua), which runs whenever Lightroom
    does — so unlike People, nothing has to stay open here. This task only
    picks the model, opens a chat session with the agent token and hands the
    browser the page. The page asks before the model changes anything; what
    it may do without asking is set on the page itself.
]]

--- The engines on this computer.
local LOCAL_PROVIDERS = { mlx = true, llamacpp = true }

--- Every provider the chat runs on: the local engines, AI servers, and the
--- cloud APIs, each through its own tool calling.
local CHAT_PROVIDERS = {
	mlx = true,
	llamacpp = true,
	lmstudio = true,
	ollama = true,
	openai_compatible = true,
	chatgpt = true,
	anthropic = true,
	gemini = true,
}

--- Who reads the conversation for a provider that is not on this computer.
local RECIPIENTS = {
	chatgpt = "OpenAI",
	anthropic = "Anthropic",
	gemini = "Google",
	openai_compatible = "your AI server",
}

--- The model to chat with: the one chosen for Analyze & Index, whichever
--- provider it is on, otherwise the first installed local one — MLX first on
--- a Mac, where the release build has no llama.cpp.
--- @return string|nil provider
--- @return string|nil model
local function chooseModel()
	local provider, model = AiProviders.splitModelKey(prefs.modelKey)
	if CHAT_PROVIDERS[provider] and model then
		return provider, model
	end
	local resp = SearchIndexAPI.getModels({ skipServer = true })
	local models = type(resp) == "table" and type(resp.models) == "table" and resp.models or {}
	local order = MAC_ENV and { "mlx", "llamacpp" } or { "llamacpp", "mlx" }
	for _, candidate in ipairs(order) do
		local list = models[candidate]
		if type(list) == "table" and list[1] ~= nil then
			return candidate, tostring(list[1])
		end
	end
	return nil, nil
end

--- Asks once per provider before a chat sends the catalog off this computer.
--- @return boolean true to go on
local function confirmRecipient(provider)
	local recipient = RECIPIENTS[provider]
	local key = "chatSendsTo_" .. provider
	if not recipient or prefs[key] then
		return true
	end
	local answer = LrDialogs.confirm(
		"This chat sends your catalog to " .. recipient,
		"The chat runs on the model chosen for Analyze & Index. What it reads to do its work — your "
			.. "messages, photo previews, metadata and the names of recognised people — goes to "
			.. recipient
			.. " with every step, under their terms. Pick a local model for Analyze & Index to keep "
			.. "the chat on this computer.",
		"Open the chat",
		"Cancel"
	)
	if answer ~= "ok" then
		return false
	end
	prefs[key] = true
	return true
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskChat", function(context)
		LrDialogs.attachErrorDialogToFunctionContext(context)
		if not Util.waitForServerDialog() then
			return
		end

		local provider, model = chooseModel()
		if not provider then
			LrDialogs.message(
				"The chat needs an AI model",
				"Choose a model for Analyze & Index — a cloud model, an AI server, or a model on this "
					.. "computer (Plug-in Manager > LrGeniusAI > Local AI Model) — then open the chat again.",
				"info"
			)
			return
		end

		local agentToken, tokenErr = Secrets.getMcpToken()
		if tokenErr then
			ErrorHandler.handleError("Could not open the chat", tokenErr)
			return
		end
		if agentToken == "" then
			LrDialogs.message(
				"Turn on AI agents first",
				"The chat works through LrGeniusAI's agent bridge. Turn it on under "
					.. "Plug-in Manager > LrGeniusAI > AI Agents, then open the chat again.",
				"info"
			)
			return
		end

		local body = { provider = provider, model = model }
		if LOCAL_PROVIDERS[provider] then
			-- The same engine settings indexing uses, so opening the chat does
			-- not make the backend reload the model with other ones.
			body.llm_n_ctx = prefs.llmContextSize and tostring(prefs.llmContextSize) or nil
			body.llm_n_parallel = prefs.llmParallel and tostring(prefs.llmParallel) or nil
			body.llm_gpu_layers = prefs.llmGpuLayers and tostring(prefs.llmGpuLayers) or nil
		else
			-- The key and server address the provider needs, as for indexing.
			local connection, problem = AiProviders.connectionOptions(provider, Secrets.connectionFields(prefs))
			if not connection then
				ErrorHandler.handleError("Could not open the chat", problem)
				return
			end
			if not confirmRecipient(provider) then
				return
			end
			body.api_key = connection.api_key
			body.server_url = connection.server_url
		end

		local resp, err = SearchIndexAPI.openChatSession(agentToken, body)
		if err or type(resp) ~= "table" or type(resp.page) ~= "string" then
			ErrorHandler.handleError("Could not open the chat", err or "The backend did not return a chat page.")
			return
		end

		-- Not logged with its address: the fragment carries the chat's token.
		log:info("Opening the chat page (" .. provider .. ", " .. model .. ")")
		LrHttp.openUrlInBrowser(SearchIndexAPI.pageUrl(resp.page))
	end)
end)
