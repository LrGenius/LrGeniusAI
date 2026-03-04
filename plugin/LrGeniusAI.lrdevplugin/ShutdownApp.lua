local function shutdownApp(doneFunc, progressFunc)
    -- Only shut down the backend when it is running on localhost (we started it).
    if SearchIndexAPI.isBackendOnLocalhost() then
        LrTasks.startAsyncTask(function ()
            SearchIndexAPI.shutdownServer()
        end)
    end
    doneFunc()
end

return {
    LrShutdownFunction = shutdownApp,
}
