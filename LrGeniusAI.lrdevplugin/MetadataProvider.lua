
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
    },

    schemaVersion = 23,
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
        end,
}