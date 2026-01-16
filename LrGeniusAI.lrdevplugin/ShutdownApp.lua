local function shutdownApp(doneFunc, progressFunc)
    LrTasks.startAsyncTask(function ()
        SearchIndexAPI.shutdownServer()
    end)
    doneFunc()
end

return {
    LrShutdownFunction = shutdownApp,
}
