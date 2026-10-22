--[[
    Chat with Your Catalog: opens the agent chat in the browser.

    The chat is the backend's /v1/ui/chat page. Its model works the catalog
    through the agent bridge (AgentBridge.lua), which runs whenever Lightroom
    does — so unlike People, nothing has to stay open here. This task only
    picks the local model, opens a chat session with the agent token and
    hands the browser the page. The page asks before the model changes
    anything; what it may do without asking is set on the page itself.
]]

--- The engines the chat runs on today. Cloud models come later.
local LOCAL_PROVIDERS = { mlx = true, llamacpp = true }

--- The local model to chat with: the one chosen for Analyze & Index when it
--- is local, otherwise the first installed one — MLX first on a Mac, where
--- the release build has no llama.cpp.
--- @return string|nil provider
--- @return string|nil model
local function chooseLocalModel()
	local provider, model = AiProviders.splitModelKey(prefs.modelKey)
	if LOCAL_PROVIDERS[provider] and model then
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

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskChat", function(context)
		LrDialogs.attachErrorDialogToFunctionContext(context)
		if not Util.waitForServerDialog() then
			return
		end

		local provider, model = chooseLocalModel()
		if not provider then
			LrDialogs.message(
				"The chat needs a local AI model",
				"Chatting with your catalog runs on a model on this computer. Download one under "
					.. "Plug-in Manager > LrGeniusAI > Local AI Model, then open the chat again.",
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

		local resp, err = SearchIndexAPI.openChatSession(agentToken, {
			provider = provider,
			model = model,
			-- The same engine settings indexing uses, so opening the chat does
			-- not make the backend reload the model with other ones.
			llm_n_ctx = prefs.llmContextSize and tostring(prefs.llmContextSize) or nil,
			llm_n_parallel = prefs.llmParallel and tostring(prefs.llmParallel) or nil,
			llm_gpu_layers = prefs.llmGpuLayers and tostring(prefs.llmGpuLayers) or nil,
		})
		if err or type(resp) ~= "table" or type(resp.page) ~= "string" then
			ErrorHandler.handleError("Could not open the chat", err or "The backend did not return a chat page.")
			return
		end

		-- Not logged with its address: the fragment carries the chat's token.
		log:info("Opening the chat page (" .. provider .. ", " .. model .. ")")
		LrHttp.openUrlInBrowser(SearchIndexAPI.pageUrl(resp.page))
	end)
end)
