-- TaskImportMetadata.lua
-- Task to import existing metadata from the Lightroom catalog to the backend.

local function showImportDialog(ctx)
    local f = LrView.osFactory()
    local bind = LrView.bind
    local share = LrView.share

    local props = LrBinding.makePropertyTable(ctx)
    props.scope = prefs.importScope or "all"

    local contents = f:column {
        bind_to_object = props,
        spacing = f:control_spacing(),
        f:group_box {
            title = LOC "$$$/LrGeniusAI/ImportMetadata/Scope=Scope",
            fill_horizontal = 1,
            f:row {
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/ImportMetadata/Scope=Scope",
                    width = share 'labelWidth',
                },
                f:popup_menu {
                    value = bind 'scope',
                    width = 300,
                    items = {
                        { title = LOC "$$$/LrGeniusAI/common/ScopeSelected=Selected photos only", value = 'selected' },
                        { title = LOC "$$$/LrGeniusAI/common/ScopeView=Current view", value = 'view' },
                        { title = LOC "$$$/LrGeniusAI/ImportMetadata/ScopeAll=All photos in catalog", value = 'all' },
                    },
                },
            },
        },
    }

    local result = LrDialogs.presentModalDialog {
        title = LOC "$$$/LrGeniusAI/ImportMetadata/WindowTitle=Import Metadata from Catalog",
        contents = contents,
        actionVerb = LOC "$$$/LrGeniusAI/common/Start=Start",
        cancelVerb = LOC "$$$/LrGeniusAI/common/Cancel=Cancel",
        resizable = false,
    }

    if result == 'ok' then
        prefs.importScope = props.scope
        return props
    end

    return nil
end

LrTasks.startAsyncTask(function()
    LrFunctionContext.callWithContext("ImportMetadataTask", function(context)
        -- Check server connection
        if not Util.waitForServerDialog() then return end

        local props = showImportDialog(context)
        if not props then return end

        local photosToProcess, errorStatus = PhotoSelector.getPhotosInScope(props.scope)

        if photosToProcess == nil or type(photosToProcess) ~= 'table' or #photosToProcess == 0 then
            if errorStatus == "Invalid view" then
                LrDialogs.message(
                    LOC "$$$/LrGeniusAI/common/InvalidViewTitle=Invalid View",
                    LOC "$$$/LrGeniusAI/common/InvalidViewMessage=The 'Current view' scope only works when a folder or collection is selected."
                )
            else
                LrDialogs.message(
                    LOC "$$$/LrGeniusAI/common/NoPhotosTitle=No Photos Found",
                    LOC "$$$/LrGeniusAI/common/NoPhotosMessage=No photos found in the selected scope."
                )
            end
            return
        end

        local progressScope = LrProgressScope({
            title = LOC "$$$/LrGeniusAI/ImportMetadata/ProgressTitle=Importing metadata...",
            functionContext = context,
        })

        local status, processed, failed = SearchIndexAPI.importMetadataFromCatalog(photosToProcess, progressScope)

        progressScope:done()

        if status == "canceled" then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/common/TaskCanceled/Title=Task Canceled",
                LOC "$$$/LrGeniusAI/common/TaskCanceled/Message=The task was canceled by the user."
            )
        elseif status == "allfailed" then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/common/TaskFailed/Title=Task Failed",
                LOC("$$$/LrGeniusAI/ImportMetadata/AllFailedMessage=All ^1 photos failed to import.", processed)
            )
        elseif status == "somefailed" then
            local successCount = processed - failed
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/common/TaskCompleted/Title=Task Completed with Errors",
                LOC("$$$/LrGeniusAI/ImportMetadata/SomeFailedMessage=^1 of ^2 photos imported successfully. ^3 failed.", successCount, processed, failed)
            )
        else -- success
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/common/TaskCompleted/Title=Task Completed",
                LOC("$$$/LrGeniusAI/ImportMetadata/SuccessMessage=Successfully imported metadata for ^1 photos.", processed)
            )
        end

        log:trace("ImportMetadataTask completed: Status=" .. status .. ", Processed=" .. processed .. ", Failed=" .. failed)
    end)
end)