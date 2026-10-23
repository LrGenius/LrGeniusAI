local function shutdownApp(doneFunc, progressFunc)
	-- Instead of shutting down the backend, we now request it to unload models and collections from memory
	-- to free up resources. This is sent to both local and remote backends to ensure efficiency.
	LrTasks.startAsyncTask(function()
		-- First, while HTTP still works: an agent waiting on Lightroom learns
		-- that it went away instead of waiting out its timeout.
		LrTasks.pcall(AgentBridge.shutdown)
		LrTasks.pcall(function()
			SearchIndexAPI.unloadResources()
		end)
		doneFunc()
	end)
end

return {
	LrShutdownFunction = shutdownApp,
}
