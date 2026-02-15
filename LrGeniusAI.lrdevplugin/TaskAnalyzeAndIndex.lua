-- TaskAnalyzeAndIndex.lua
-- Unified task for analyzing photos with AI (metadata + quality scores) and indexing them.
-- Combines the old TaskAnalyzeImage and TaskManageIndex into one streamlined workflow.


---
-- Shows the main configuration dialog for analyze and index task.
-- @param ctx The LrFunctionContext for the dialog.
-- @return table with configuration options or nil if canceled.
--
local function showAnalyzeAndIndexDialog(ctx)
    local f = LrView.osFactory()
    local bind = LrView.bind
    local share = LrView.share

    local props = LrBinding.makePropertyTable(ctx)
    
    -- Scope settings
    props.scope = prefs.indexScope or "selected"
    
    -- Check if CLIP model is ready on server
    props.clipReady = SearchIndexAPI.isClipReady() and prefs.useClip

    -- Tasks to perform
    props.enableEmbeddings = (prefs.enableEmbeddings ~= false) and props.clipReady -- default true
    props.enableMetadata = prefs.enableMetadata ~= false -- default true
    props.enableFaces = prefs.enableFaces or false
    props.enableImportBeforeIndex = prefs.enableImportBeforeIndex or false
    props.enableQuality = false 
    props.regenerateMetadata = prefs.regenerateMetadata or false
    
    -- Metadata generation options
    props.temperature = prefs.temperature or 0.1
    props.promptTitles = {}
    for title, prompt in pairs(prefs.prompts) do
        table.insert(props.promptTitles, { title = title, value = title })
    end

    props.prompt = prefs.prompt
    props.prompts = prefs.prompts

    props.selectedPrompt = prefs.prompts[prefs.prompt]

    props:addObserver('prompt', function(properties, key, newValue)
        properties.selectedPrompt = properties.prompts[newValue]
    end)

    props:addObserver('selectedPrompt', function(properties, key, newValue)
        properties.prompts[properties.prompt] = newValue
    end)

    props.generateKeywords = prefs.generateKeywords ~= false
    props.generateCaption = prefs.generateCaption ~= false
    props.generateTitle = prefs.generateTitle ~= false
    props.generateAltText = prefs.generateAltText or false
    props.useKeywordHierarchy = prefs.useKeywordHierarchy or false
    props.useCatalogKeywordStructure = prefs.useCatalogKeywordStructure or false
    props.useTopLevelKeyword = prefs.useTopLevelKeyword or false
    props.topLevelKeyword = prefs.topLevelKeyword or "LrGeniusAI"
    
    -- AI Model selection (unified across providers)
    props.modelKey = prefs.modelKey -- format: "provider::model"
    props.language = prefs.generateLanguage or "English"
    props.temperature = prefs.temperature or 0.1
    props.replaceSS = prefs.replaceSS or false

    -- Build model list from server (local providers first)
    local modelItems = {}

    -- Fetch all models with API keys if configured
    -- Server will check all providers and filter to multimodal only
    local openaiKey = (prefs and not Util.nilOrEmpty(prefs.chatgptApiKey)) and prefs.chatgptApiKey or nil
    local geminiKey = (prefs and not Util.nilOrEmpty(prefs.geminiApiKey)) and prefs.geminiApiKey or nil
    
    local modelsResp = SearchIndexAPI.getModels(openaiKey, geminiKey)
    if modelsResp and modelsResp.models then
        for provider, list in pairs(modelsResp.models) do
            for _, model in ipairs(list) do
                local title = provider .. ": " .. model
                local value = provider .. "::" .. model
                table.insert(modelItems, { title = title, value = value })
            end 
        end
    end
    
    table.sort(modelItems, function(a,b) return a.title < b.title end)
    if (not modelItems or #modelItems == 0) then
        -- Fallback option if nothing matched filters
        table.insert(modelItems, { title = 'qwen: (default)', value = 'qwen::' })
    end
    if not props.modelKey or props.modelKey == '' then
        props.modelKey = modelItems[1].value
    end
    
    -- Context options
    props.submitGPS = prefs.submitGPS or false
    props.submitKeywords = prefs.submitKeywords or false
    props.submitFolderName = prefs.submitFolderName or false
    props.showPhotoContextDialog = prefs.showPhotoContextDialog or false
    props.submitDateTime = prefs.submitDateTime or false
    
    -- SaveDataToCatalog
    props.saveDataToCatalog = prefs.saveDataToCatalog ~= false -- default true

    -- Validation
    props.enableValidation = prefs.enableValidation or false

    props.promptTitleMenu = f:popup_menu {
        items = bind 'promptTitles',
        value = bind 'prompt',
    }

    local contents = f:column {
        bind_to_object = props,
        spacing = f:control_spacing(),
        
        -- Scope Selection
        f:group_box {
            title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/Scope=Scope",
            fill_horizontal = 1,
            f:row {
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/Scope=Scope",
                    width = share 'labelWidth',
                },
                f:popup_menu {
                    value = bind 'scope',
                    width = 300,
                    items = {
                        { title = LOC "$$$/LrGeniusAI/common/ScopeSelected=Selected photos only", value = 'selected' },
                        { title = LOC "$$$/LrGeniusAI/common/ScopeView=Current view", value = 'view' },
                        { title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/ScopeAll=All photos in catalog", value = 'all' },
                        { title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/ScopeMissing=New or unprocessed photos", value = 'missing' },
                    },
                },
            },
        },

        -- AI Model Settings (unified)
        f:group_box {
            title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/AISettings=AI Settings",
            fill_horizontal = 1,
            f:row {
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/aiModel=AI Model:",
                    width = share 'labelWidth',
                },
                f:popup_menu {
                    value = bind 'modelKey',
                    items = modelItems,
                    width = 300,
                },
            },
            f:row {
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/Temperature=Temperature:",
                    width = share 'labelWidth',
                },
                f:slider {
                    value = bind 'temperature',
                    min = 0.0,
                    max = 0.5,
                    integral = false,
                    width = 300,
                },
                f:static_text {
                    title = bind 'temperature',
                    width = 40,
                },
            },
            f:row {
                f:static_text {
                    width = share 'labelWidth',
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/editPrompts=Edit prompts",
                },
                props.promptTitleMenu,
                f:push_button {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/add=Add",
                    action = function(button)
                        local newName = PromptConfigProvider.addPrompt(props)
                    end,
                },
                f:push_button {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/delete=Delete",
                    action = function(button)
                        PromptConfigProvider.deletePrompt(props)
                    end,
                },
            },
            f:row {
                f:static_text {
                    width = share 'labelWidth',
                    title = LOC "$$$/LrGeniusAI/PromptConfig/PromptField=Prompt",
                },
                f:edit_field {
                    value = bind 'selectedPrompt',
                    width_in_chars = 40,
                    height_in_lines = 10,
                    -- enabled = false,
                },
            },
            f:row {
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/generateLanguage=Language:",
                    width = share 'labelWidth',
                },
                f:combo_box {
                    value = bind 'language',
                    items = Defaults.generateLanguages,
                },
                f:checkbox {
                    value = bind 'replaceSS',
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/replaceSS=Replace ß with ss",
                },
            },
        },
        
        -- Tasks to Perform
        f:group_box {
            title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/Tasks=Tasks to Perform",
            fill_horizontal = 1,
            f:row {
                f:checkbox {
                    value = bind 'enableEmbeddings',
                    title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/EnableEmbeddings=Create search embeddings",
                    enabled = props.clipReady,
                },
            },
            f:row {
                f:checkbox {
                    value = bind 'enableMetadata',
                    title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/EnableMetadata=Generate AI metadata",
                },
            },
            f:row {
                f:checkbox {
                    value = bind 'enableFaces',
                    title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/EnableFaces=Create face embeddings",
                },
            },
            f:row {
                f:checkbox {
                    value = bind 'enableImportBeforeIndex',
                    title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/EnableImportBeforeIndex=Import metadata from catalog before indexing",
                },
            },
            f:row {
                f:checkbox {
                    value = bind 'regenerateMetadata',
                    title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/RegenerateMetadata=Regenerate all data (overwrite existing)",
                },
            },
        },
        
        -- Metadata Options (only shown if metadata is enabled)
        f:group_box {
            title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/MetadataOptions=Metadata Options",
            fill_horizontal = 1,
            visible = bind 'enableMetadata',
            f:row {
                f:checkbox {
                    value = bind 'generateKeywords',
                    width = share 'checkboxWidth',
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/keywords=Keywords",
                },
                f:checkbox {
                    value = bind 'generateCaption',
                    width = share 'checkboxWidth',
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/caption=Caption",
                },
                f:checkbox {
                    value = bind 'generateTitle',
                    width = share 'checkboxWidth',
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/title=Title",
                },
                f:checkbox {
                    value = bind 'generateAltText',
                    width = share 'checkboxWidth',
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/alttext=Alt Text",
                },
            },
            f:row {
                f:static_text {
                    width = share 'labelWidth',
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/useKeywordHierarchy=Use keyword hierarchy",
                },
                f:checkbox {
                    value = bind 'useKeywordHierarchy',
                    width = share 'checkboxWidth',
                },
                f:push_button {
                    width = share 'labelWidth',
                    enabled = bind 'useKeywordHierarchy',
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/editKeywordHierarchy=Edit keyword categories",
                    action = function (button)
                        KeywordConfigProvider.showKeywordCategoryDialog()
                    end,
                },
            },
            f:row {
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/UI/UseCatalogKeywordStructure=Use keyword structure from Lightroom catalog"
                },
                f:checkbox {
                    value = bind 'useCatalogKeywordStructure',
                    width = share 'checkboxWidth',
                }
            },
            f:row {
                f:static_text {
                    width = share 'labelWidth',
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/useTopLevelKeyword=Use top-level keyword",
                },
                f:checkbox {
                    value = bind 'useTopLevelKeyword',
                    width = share 'checkboxWidth',
                },
                f:edit_field {
                    value = bind 'topLevelKeyword',
                    width_in_chars = 20,
                    enabled = bind 'useTopLevelKeyword',
                },
            }
        },
        
        -- Context Options
        f:group_box {
            title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/ContextOptions=Context Options",
            fill_horizontal = 1,
            visible = bind 'enableMetadata',
            f:row {
                f:checkbox {
                    value = bind 'submitGPS',
                    width = share 'checkboxWidth',
                },
                f:static_text {
                    title = "GPS Coordinates",
                },
                f:checkbox {
                    value = bind 'submitKeywords',
                    width = share 'checkboxWidth',
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/submitKeywords=Existing Keywords",
                },
                f:checkbox {
                    value = bind 'submitDateTime',
                    width = share 'checkboxWidth',
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/submitDateTime=Capture Date/Time",
                },
            },
            f:row {
                f:checkbox {
                    value = bind 'submitFolderName',
                    width = share 'checkboxWidth',
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/folderNames=Folder Names",
                },
                f:checkbox {
                    value = bind 'showPhotoContextDialog',
                    width = share 'checkboxWidth',
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/showPhotoContextDialog=Photo Context Dialog",
                },
            },
        },
        
        -- Validation
        f:row {
            f:checkbox {
                value = bind 'saveDataToCatalog',
            },
            f:static_text {
                title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/SaveDataToCatalog=Save generated data to catalog",
            },
            f:checkbox {
                enabled = bind 'saveDataToCatalog',
                value = bind 'enableValidation',
            },
            f:static_text {
                title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/validation=Review before saving",
            },
        },
    }

    local result = LrDialogs.presentModalDialog {
        title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/WindowTitle=Analyze and Index Photos",
        contents = contents,
        actionVerb = LOC "$$$/LrGeniusAI/common/Start=Start",
        cancelVerb = LOC "$$$/LrGeniusAI/common/Cancel=Cancel",
        resizable = true,
    }

    if result == 'ok' then
        -- Save preferences
        prefs.indexScope = props.scope
        prefs.enableEmbeddings = props.enableEmbeddings
        prefs.enableMetadata = props.enableMetadata
        prefs.enableFaces = props.enableFaces
        prefs.enableQuality = props.enableQuality
        prefs.enableImportBeforeIndex = props.enableImportBeforeIndex
        prefs.regenerateMetadata = props.regenerateMetadata
        prefs.generateKeywords = props.generateKeywords
        prefs.generateCaption = props.generateCaption
        prefs.generateTitle = props.generateTitle
        prefs.generateAltText = props.generateAltText
        -- Persist selected model key and provider for backwards compatibility
        prefs.modelKey = props.modelKey
        if props.modelKey then
            local sep = string.find(props.modelKey, "::", 1, true)
            if sep then
                local prov = string.sub(props.modelKey, 1, sep-1)
                prefs.ai = prov
            end
        end
        prefs.generateLanguage = props.language
        prefs.temperature = props.temperature
        prefs.submitGPS = props.submitGPS
        prefs.submitKeywords = props.submitKeywords
        prefs.submitFolderName = props.submitFolderName
        prefs.submitDateTime = props.submitDateTime
        prefs.showPhotoContextDialog = props.showPhotoContextDialog
        prefs.enableValidation = props.enableValidation
        prefs.saveDataToCatalog = props.saveDataToCatalog
        prefs.replaceSS = props.replaceSS
        prefs.prompt = props.prompt
        prefs.prompts = props.prompts
        prefs.useKeywordHierarchy = props.useKeywordHierarchy
        prefs.useCatalogKeywordStructure = props.useCatalogKeywordStructure
        prefs.useTopLevelKeyword = props.useTopLevelKeyword
        prefs.topLevelKeyword = props.topLevelKeyword

        -- Keep track of used top-level keywords
        if props.useTopLevelKeyword and not Util.table_contains(prefs.knownTopLevelKeywords, props.topLevelKeyword) then
            table.insert(prefs.knownTopLevelKeywords, props.topLevelKeyword)
        end

        return props
    end
    
    return nil
end


local function showPhotoContextDialog(photo)
    local f = LrView.osFactory()
    local bind = LrView.bind
    local share = LrView.share

    local props = {}
    props.skipFromHere = SkipPhotoContextDialog
    local photoContextFromCatalog = photo:getPropertyForPlugin(_PLUGIN, 'photoContext')
    if photoContextFromCatalog ~= nil then
        PhotoContextData = photoContextFromCatalog
    end
    props.photoContextData = PhotoContextData
    props.skipFromHere = false

    local dialogView = f:column {
        bind_to_object = props,
        f:row {
            f:static_text {
                title = photo:getFormattedMetadata('fileName'),
            },
        },
        f:row {
            f:spacer {
                height = 10,
            },
        },
        f:row {
            alignment = "center",
            f:catalog_photo {
                photo = photo,
                width = 300,
            },
        },
        f:row {
            f:spacer {
                height = 10,
            },
        },
        f:row {
            f:static_text {
                title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/PhotoContextDialogData=Photo Context",
            },
        },
        f:row {
            f:spacer {
                height = 10,
            },
        },
        f:row {
            f:edit_field {
                value = bind 'photoContextData',
                width_in_chars = 40,
                height_in_lines = 10,
            },
        },
        f:row {
            f:spacer {
                height = 10,
            },
        },
        f:row {
            f:checkbox {
                value = bind 'skipFromHere'
            },
            f:static_text {
                title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/SkipPreflightFromHere=Use for all following pictures.",
            },
        },
    }

    local result = LrDialogs.presentModalDialog({
        title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/PhotoContextDialogData=Photo Context",
        contents = dialogView,
    })

    SkipPhotoContextDialog = props.skipFromHere

    return result, props.photoContextData, props.skipFromHere
end

LrTasks.startAsyncTask(function()
    LrFunctionContext.callWithContext("AnalyzeAndIndexTask", function(context)
        -- Check server connection
        if not Util.waitForServerDialog() then return end

        -- Show dialog
        local props = showAnalyzeAndIndexDialog(context)
        if not props then return end

        -- Validate that at least one task is selected
        if not props.enableEmbeddings and not props.enableMetadata and not props.enableQuality and not props.enableFaces then
            LrDialogs.showError(LOC "$$$/LrGeniusAI/AnalyzeAndIndex/NoTasksSelected=Please select at least one task to perform.")
            return
        end

        -- Build tasks array
        local tasks = {}
        if props.enableEmbeddings then table.insert(tasks, "embeddings") end
        if props.enableMetadata then table.insert(tasks, "metadata") end
        if props.enableQuality then table.insert(tasks, "quality") end
        if props.enableFaces then table.insert(tasks, "faces") end

        -- Parse provider and model from unified modelKey (format: provider::model)
        local providerFromKey, modelFromKey = nil, nil
        if props.modelKey then
            local sep = string.find(props.modelKey, "::", 1, true)
            if sep then
                providerFromKey = string.sub(props.modelKey, 1, sep-1)
                modelFromKey = string.sub(props.modelKey, sep+2)
                if modelFromKey == "" then modelFromKey = nil end
            else
                providerFromKey = props.modelKey -- fallback
            end
        end

        -- Build options for the API
        local options = {
            tasks = tasks,
            provider = providerFromKey,
            model = modelFromKey,
            language = props.language,
            temperature = props.temperature,
            generate_keywords = props.generateKeywords,
            generate_caption = props.generateCaption,
            generate_title = props.generateTitle,
            generate_alt_text = props.generateAltText,
            submit_gps = props.submitGPS,
            submit_keywords = props.submitKeywords,
            submit_folder_names = props.submitFolderName,
            submit_user_context = props.showPhotoContextDialog,
            submit_date_time = props.submitDateTime,
            enableMetadata = props.enableMetadata,
            enableQuality = props.enableQuality,
            enableFaces = props.enableFaces,
            replace_ss = props.replaceSS,
            regenerate_metadata = props.regenerateMetadata,
            prompt = props.selectedPrompt,
        }
        -- Add API key for cloud providers if configured
        if providerFromKey == 'chatgpt' and prefs then
            log:trace("Added ChatGPT API key to options")
            if prefs.chatgptApiKey == nil or prefs.chatgptApiKey == '' then
                LrDialogs.showError(LOC "$$$/LrGeniusAI/AnalyzeAndIndex/MissingChatGPTAPIKey=ChatGPT API key is not configured. Please set it in the plugin preferences.")
                return
            end
            options.api_key = prefs.chatgptApiKey
        elseif providerFromKey == 'gemini' and prefs then
            if prefs.geminiApiKey == nil or prefs.geminiApiKey == '' then
                LrDialogs.showError(LOC "$$$/LrGeniusAI/AnalyzeAndIndex/MissingGeminiAPIKey=Gemini API key is not configured. Please set it in the plugin preferences.")
                return
            end
            log:trace("Added Gemini API key to options")
            options.api_key = prefs.geminiApiKey
        end

        if prefs.useKeywordHierarchy then
            if prefs.useCatalogKeywordStructure then
                options.keyword_categories = MetadataManager.getCatalogKeywordHierarchy()
            else
                options.keyword_categories = KeywordConfigProvider.getKeywordCategories()
            end
        end

        -- Create progress scope
        local progressScope = LrProgressScope({
            title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/ProgressTitle=Processing photos...",
            functionContext = context,
        })

        local status, processed, failed, processedPhotos

        -- Get photos to process
        -- For scope 'missing', pass task options so backend checks which photos need the selected tasks
        local taskOptionsForScope = (props.scope == "missing") and {
            enableEmbeddings = props.enableEmbeddings,
            enableMetadata = props.enableMetadata,
            enableQuality = props.enableQuality,
            enableFaces = props.enableFaces,
            regenerateMetadata = props.regenerateMetadata
        } or nil
        local photosToProcess, errorStatus = PhotoSelector.getPhotosInScope(props.scope, taskOptionsForScope)

        if photosToProcess == nil or type(photosToProcess) ~= 'table' or #photosToProcess == 0 then
            if errorStatus == "Invalid view" then
                LrDialogs.message(
                    LOC "$$$/LrGeniusAI/common/InvalidViewTitle=Invalid View",
                    LOC "$$$/LrGeniusAI/common/InvalidViewMessage=The 'Current view' scope only works when a folder or collection is selected."
                )
            else
                log:trace("No photos found to process in scope: " .. props.scope .. " errorStatus: " .. (errorStatus or "nil"))
                LrDialogs.message(
                    LOC "$$$/LrGeniusAI/common/NoPhotosTitle=No Photos Found",
                    LOC "$$$/LrGeniusAI/common/NoPhotosMessage=No photos found in the selected scope."
                )
            end
            return
        end

        -- If photo context dialog is enabled, show it for each photo
        if props.showPhotoContextDialog and props.enableMetadata then
            -- Show photo context dialog to gather additional context
            local skipFromHere = false
            local contextData = ""
            for _, photo in ipairs(photosToProcess) do
                local result = ""
                if not skipFromHere then
                    result, contextData, skipFromHere = showPhotoContextDialog(photo)
                    if result == "ok" then
                    elseif result == "cancel" then
                        log:trace("User canceled photo context dialog for photo: " .. (photo:getFormattedMetadata('fileName') or "unknown"))
                        progressScope:done()
                        return
                    end
                end
                LrApplication.activeCatalog():withPrivateWriteAccessDo(function()
                    photo:setPropertyForPlugin(_PLUGIN, 'photoContext', options.user_context)
                end)
            end
        end

        if props.enableImportBeforeIndex then
            local importProgressScope = LrProgressScope({
                title = LOC "$$$/LrGeniusAI/AnalyzeAndIndex/ImportingMetadata=Importing existing metadata from catalog...",
                functionContext = context,
                parent = progressScope,
            })
            log:trace("Importing existing metadata from catalog before indexing...")
            SearchIndexAPI.importMetadataFromCatalog(photosToProcess, importProgressScope)
        end

        log:trace("Starting AnalyzeAndIndexTask with " .. #photosToProcess .. " photos")
        
        -- Update progress title with photo count
        local processingProgressScope = LrProgressScope({
            title = LOC("$$$/LrGeniusAI/AnalyzeAndIndex/ProcessingPhotos=Processing ^1 photos with ^2...", #photosToProcess, modelFromKey or "AI"),
            functionContext = context,
            parent = progressScope,
        })
        
        status, processed, failed, processedPhotos = SearchIndexAPI.analyzeAndIndexSelectedPhotos(photosToProcess, processingProgressScope, options)

        if status ~= "allfailed" and (props.enableMetadata or props.enableQuality) and props.saveDataToCatalog then
            log:trace("Saving metadata for processed photos...")
            local savedCount = 0
            local skippedCount = 0
            
            local skipFromHere = false

            for _, photo in ipairs(processedPhotos) do
                -- Process responses if validation is enabled or just save metadata
                local response = SearchIndexAPI.getPhotoData(photo:getRawMetadata('uuid'))

                log:trace("Got generated data for photo: " .. (photo:getFormattedMetadata('fileName') or "unknown"))
                log:trace("Response: " .. (Util.dumpTable(response) or "nil"))

                if props.enableValidation and props.enableMetadata and response and response.metadata then
                    -- Show validation dialog
                    local result, validatedData = nil, nil
                    if not skipFromHere then
                        result, validatedData = MetadataManager.showValidationDialog(context, photo, response, {
                            applyKeywords = props.generateKeywords,
                            applyTitle = props.generateTitle,
                            applyCaption = props.generateCaption,
                            applyAltText = props.generateAltText,
                            applyQuality = props.enableQuality,
                        })

                        if validatedData ~= nil and validatedData.skipFromHere then
                            log:trace("Skipping validation from here for subsequent photos.")
                            skipFromHere = true
                        end
                    else
                        skippedCount = skippedCount + 1
                    end

                    if result == "ok" and validatedData then
                        -- Apply validated metadata
                        MetadataManager.applyMetadata(photo, response, validatedData, {
                            applyKeywords = props.generateKeywords,
                            applyTitle = props.generateTitle,
                            applyCaption = props.generateCaption,
                            applyAltText = props.generateAltText,
                            applyQuality = props.enableQuality,
                            useTopLevelKeyword = props.useTopLevelKeyword,
                            topLevelKeyword = props.topLevelKeyword,
                        })

                        -- Overwrite with validated data
                        log:trace("Reimported validated metadata for photo: " .. (photo:getFormattedMetadata('fileName') or "unknown"))
                        SearchIndexAPI.importMetadataFromCatalog({ photo }, progressScope)

                        savedCount = savedCount + 1
                    elseif result == "other" then
                        skippedCount = skippedCount + 1
                    elseif result == "cancel" then
                        break
                    end

                elseif props.enableMetadata and response and response.metadata then
                    -- Directly save generated metadata without validation
                    MetadataManager.applyMetadata(photo, response, nil, {
                        applyKeywords = props.generateKeywords,
                        applyTitle = props.generateTitle,
                        applyCaption = props.generateCaption,
                        applyAltText = props.generateAltText,
                        applyQuality = props.enableQuality,
                        useTopLevelKeyword = props.useTopLevelKeyword,
                        topLevelKeyword = props.topLevelKeyword,
                    })
                    savedCount = savedCount + 1
                end
            end
        end
        
        progressScope:done()

        -- Show completion message based on status
        if status == "canceled" then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/common/TaskCanceled/Title=Task Canceled",
                LOC "$$$/LrGeniusAI/common/TaskCanceled/Message=The task was canceled by the user."
            )
        elseif status == "allfailed" then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/common/TaskFailed/Title=Task Failed",
                LOC("$$$/LrGeniusAI/AnalyzeAndIndex/AllFailedMessage=All ^1 photos failed to process.", processed)
            )
        elseif status == "somefailed" then
            local successCount = processed - failed
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/common/TaskCompleted/Title=Task Completed with Errors",
                LOC("$$$/LrGeniusAI/AnalyzeAndIndex/SomeFailedMessage=^1 of ^2 photos processed successfully. ^3 failed.", successCount, processed, failed)
            )
        else -- success
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/common/TaskCompleted/Title=Task Completed",
                LOC("$$$/LrGeniusAI/AnalyzeAndIndex/SuccessMessage=Successfully processed ^1 photos.", processed)
            )
        end
        
        log:trace("AnalyzeAndIndexTask completed: Status=" .. status .. ", Processed=" .. processed .. ", Failed=" .. failed)
    end)
end)
