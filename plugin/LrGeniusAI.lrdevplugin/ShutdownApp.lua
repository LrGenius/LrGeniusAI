local function shutdownApp(doneFunc, progressFunc)
    -- Only shut down the backend when it is running on localhost (we started it).
    if SearchIndexAPI.isBackendOnLocalhost() then
        LrTasks.startAsyncTask(function()
            pcall(function()
                SearchIndexAPI.shutdownServer({
                    graceSeconds = 10,
                    forceWaitSeconds = 10,
                    pollIntervalSeconds = 0.5,
                    shutdownRequestTimeoutSeconds = 5,
                })
            end)
            doneFunc()
        end)
    else
        doneFunc()
    end
end

return {
    LrShutdownFunction = shutdownApp,
}
