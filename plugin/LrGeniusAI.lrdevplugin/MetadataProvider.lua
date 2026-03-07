
return {
    metadataFieldsForPhotos = {
        {
            id = 'aiLastRun',
            title = LOC "$$$/lrc-ai-assistant/AIMetadataProvider/aiLastRun=Last AI run",
            dataType = 'string',
            readOnly = true,
            searchable = true,
            browsable = true,
        },
        {
            id = 'aiModel',
            title = LOC "$$$/lrc-ai-assistant/AIMetadataProvider/aiModel=AI model",
            dataType = 'string',
            readOnly = true,
            searchable = true,
            browsable = true,
        },
        {
            id = 'photoContext',
            title = LOC "$$$/lrc-ai-assistant/AIMetadataProvider/photoContext=Photo context",
            dataType = 'string',
            readOnly = false,
            searchable = true,
            browsable = true,
        },
        {
            id = 'keywords',
            title = LOC "$$$/lrc-ai-assistant/AIMetadataProvider/keywords=AI Keywords",
            dataType = 'string',
            readOnly = true,
            searchable = true,
            browsable = true,
        },
        {
            id = 'globalPhotoId',
            title = "Global Photo ID",
            dataType = 'string',
            readOnly = true,
            searchable = false,
            browsable = false,
        },
        {
            id = 'globalPhotoIdFileSize',
            title = "Global Photo ID File Size",
            dataType = 'string',
            readOnly = true,
            searchable = false,
            browsable = false,
        },
        {
            id = 'globalPhotoIdFileModificationDate',
            title = "Global Photo ID File Modification Date",
            dataType = 'string',
            readOnly = true,
            searchable = false,
            browsable = false,
        },
        {
            id = 'globalPhotoIdAlgorithm',
            title = "Global Photo ID Algorithm",
            dataType = 'string',
            readOnly = true,
            searchable = false,
            browsable = false,
        },
    },

    schemaVersion = 25,
    updateFromEarlierSchemaVersion = function (catalog, previousSchemaVersion, progressScope)
            catalog:assertHasPrivateWriteAccess("AIMetadataProvider.updateFromEarlierSchemaVersion")
            if previousSchemaVersion ~= nil and previousSchemaVersion < 23 then
                -- Migration from LrGeniusTagAI
                if LrDialogs.confirm(
                    LOC "$$$/lrc-ai-assistant/MetadataProvider/MigrationDetected=Migration from LrGeniusTagAI detected.",
                    LOC "$$$/lrc-ai-assistant/MetadataProvider/MigrationMessage=It is recommended to run 'Import Metadata from Catalog' from the LrGeniusAI menu to import AI-generated keywords into the new database of LrGeniusAI.",
                    LOC "$$$/lrc-ai-assistant/MetadataProvider/MigrationRunNow=Run now",
                    LOC "$$$/lrc-ai-assistant/MetadataProvider/MigrationSkip=Skip (Can be run later manually)"
                ) == "ok" then
                    require "TaskImportMetadata"
                end
            end

            if previousSchemaVersion ~= nil and previousSchemaVersion < 24 then
                local migrationChoice = LrDialogs.confirm(
                    "Backend ID migration required",
                    "This update introduces file-based photo_id values (breaking change).\n\n" ..
                    "If you already have an indexed backend database from older versions, " ..
                    "run the one-time migration now.",
                    "Run migration now",
                    "Later"
                )

                if migrationChoice == "ok" then
                    LrTasks.startAsyncTask(function()
                        local status, ok, msg
                        if type(LrTasks) == "table" and type(LrTasks.pcall) == "function" then
                            status, ok, msg = LrTasks.pcall(function()
                                return SearchIndexAPI.migratePhotoIdsFromCatalog()
                            end)
                        else
                            ok, msg = SearchIndexAPI.migratePhotoIdsFromCatalog()
                            status = true
                        end

                        if not status then
                            log:error("Photo-ID migration crashed during schema upgrade.")
                            LrDialogs.message("Photo-ID Migration failed", tostring(ok), "critical")
                        elseif ok then
                            LrDialogs.message("Photo-ID Migration", msg or "Migration completed.")
                        else
                            LrDialogs.message("Photo-ID Migration failed", msg or "Unknown error", "critical")
                        end
                    end)
                else
                    LrDialogs.message(
                        "Migration reminder",
                        "Please run 'Migrate existing DB IDs to photo_id' later from:\n" ..
                        "Plug-in Manager -> LrGeniusAI -> Backend Server.",
                        "info"
                    )
                end
            end

            if previousSchemaVersion ~= nil and previousSchemaVersion < 25 then
                local migrationChoice = LrDialogs.confirm(
                    "Backend ID migration recommended",
                    "The photo_id algorithm was updated to remain stable when metadata is written to files (for example DNG updates).\n\n" ..
                    "Please run the backend ID migration once so existing indexed data matches the new stable IDs.",
                    "Run migration now",
                    "Later"
                )

                if migrationChoice == "ok" then
                    LrTasks.startAsyncTask(function()
                        local status, ok, msg
                        if type(LrTasks) == "table" and type(LrTasks.pcall) == "function" then
                            status, ok, msg = LrTasks.pcall(function()
                                return SearchIndexAPI.migratePhotoIdsFromCatalog()
                            end)
                        else
                            ok, msg = SearchIndexAPI.migratePhotoIdsFromCatalog()
                            status = true
                        end

                        if not status then
                            log:error("Photo-ID migration crashed during schema upgrade to 25.")
                            LrDialogs.message("Photo-ID Migration failed", tostring(ok), "critical")
                        elseif ok then
                            LrDialogs.message("Photo-ID Migration", msg or "Migration completed.")
                        else
                            LrDialogs.message("Photo-ID Migration failed", msg or "Unknown error", "critical")
                        end
                    end)
                end
            end
        end,
}