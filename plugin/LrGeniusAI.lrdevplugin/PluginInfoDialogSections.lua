PluginInfoDialogSections = {}

function PluginInfoDialogSections.startDialog(propertyTable)
    propertyTable.useClip = prefs.useClip

    propertyTable.clipReady = false
    propertyTable.keepChecksRunning = true
    LrTasks.startAsyncTask(function(context)
        propertyTable.clipReady = SearchIndexAPI.isClipReady()
        while propertyTable.keepChecksRunning do
            LrTasks.sleep(5)
            propertyTable.clipReady = SearchIndexAPI.isClipReady()
        end
    end
    )
    propertyTable.logging = prefs.logging
    propertyTable.geminiApiKey = prefs.geminiApiKey
    propertyTable.chatgptApiKey = prefs.chatgptApiKey
    propertyTable.vertexProjectId = prefs.vertexProjectId
    propertyTable.vertexLocation = prefs.vertexLocation or "us-central1"

    propertyTable.exportSize = prefs.exportSize
    propertyTable.exportQuality = prefs.exportQuality
    propertyTable.usePreviewThumbnails = (prefs.usePreviewThumbnails ~= false)

    propertyTable.promptTitles = {}
    for title, prompt in pairs(prefs.prompts) do
        table.insert(propertyTable.promptTitles, { title = title, value = title })
    end

    propertyTable.prompt = prefs.prompt
    propertyTable.prompts = prefs.prompts

    propertyTable.selectedPrompt = prefs.prompts[prefs.prompt]

    propertyTable:addObserver('prompt', function(properties, key, newValue)
        properties.selectedPrompt = properties.prompts[newValue]
    end)

    propertyTable:addObserver('selectedPrompt', function(properties, key, newValue)
        properties.prompts[properties.prompt] = newValue
    end)

    propertyTable.periodicalUpdateCheck = prefs.periodicalUpdateCheck
    propertyTable.backendServerUrl = prefs.backendServerUrl or Defaults.defaultBackendServerUrl
    propertyTable.ollamaBaseUrl = prefs.ollamaBaseUrl or Defaults.defaultOllamaBaseUrl
    propertyTable.lmstudioBaseUrl = prefs.lmstudioBaseUrl or Defaults.defaultLmStudioBaseUrl

    -- Training/Style Profile stats (loaded asynchronously).
    propertyTable.trainingCount = 0
    propertyTable.styleStats = nil
    propertyTable.styleReadiness = "cold_start"
    propertyTable.styleReadyText = LOC "$$$/LrGeniusAI/Training/Status/ColdStart=Cold Start (0 examples)"
    propertyTable.styleReadyColor = { 0.7, 0.7, 0.7 }

    local function updateStats()
        LrTasks.startAsyncTask(function()
            local stats, err = SearchIndexAPI.getTrainingStats()
            if stats then
                propertyTable.styleStats = stats
                propertyTable.trainingCount = stats.count or 0

                local readiness = stats.readiness or "cold_start"
                propertyTable.styleReadiness = readiness

                if readiness == "active" then
                    propertyTable.styleReadyText = LOC "$$$/LrGeniusAI/Training/Status/Active=ACTIVE - High precision matching"
                    propertyTable.styleReadyColor = { 0.2, 0.8, 0.2 }
                elseif readiness == "limited" then
                    propertyTable.styleReadyText = LOC "$$$/LrGeniusAI/Training/Status/Limited=LIMITED - Good matching"
                    propertyTable.styleReadyColor = { 0.8, 0.8, 0.2 }
                elseif readiness == "warming_up" then
                    propertyTable.styleReadyText = LOC(
                        "$$$/LrGeniusAI/Training/Status/WarmingUp=WARMING UP (^1/10 examples)", tostring(stats.count))
                    propertyTable.styleReadyColor = { 0.8, 0.4, 0.1 }
                else
                    propertyTable.styleReadyText = LOC "$$$/LrGeniusAI/Training/Status/ColdStart=COLD START (Add examples to begin)"
                    propertyTable.styleReadyColor = { 0.7, 0.7, 0.7 }
                end
            end
        end)
    end

    updateStats()
    propertyTable.refreshStyleStats = updateStats

    -- System Health monitoring
    propertyTable.healthStatus = "healthy"
    propertyTable.healthIssues = ""
    propertyTable.healthColor = { 0, 0.8, 0 }

    local function updateHealth()
        LrTasks.startAsyncTask(function()
            local health = SearchIndexAPI.getDetailedHealth()
            local status = "healthy"
            local issues = {}
            local color = { 0, 0.8, 0 }

            if not health.backend then
                status = "critical"
                table.insert(issues, LOC "$$$/LrGeniusAI/Health/BackendFailed=Backend server is not reachable.")
                color = { 0.8, 0, 0 }
            end
            if not health.clip and prefs.useClip then
                if status ~= "critical" then
                    status = "warning"
                    color = { 0.8, 0.8, 0 }
                end
                table.insert(issues, LOC "$$$/LrGeniusAI/Health/ClipMissing=CLIP model for semantic search is missing.")
            end
            if not health.gemini and not health.chatgpt and not health.ollama and not health.lmstudio then
                if status ~= "critical" then
                    status = "warning"
                    color = { 0.8, 0.8, 0 }
                end
                table.insert(issues,
                    LOC "$$$/LrGeniusAI/Health/ApiKeysMissing=No AI providers configured for AI generation.")
            end

            propertyTable.healthStatus = status
            propertyTable.healthIssues = table.concat(issues, "; ")
            propertyTable.healthColor = color
        end)
    end

    updateHealth()
    LrTasks.startAsyncTask(function()
        while propertyTable.keepChecksRunning do
            LrTasks.sleep(10)
            updateHealth()
        end
    end)
end

function PluginInfoDialogSections.sectionsForBottomOfDialog(f, propertyTable)
    local bind = LrView.bind
    local share = LrView.share

    return {
        {
            bind_to_object = propertyTable,
            title = LOC "$$$/LrGeniusAI/PluginInfo/Logging=Logging",

            f:group_box {
                title = LOC "$$$/LrGeniusAI/PluginInfo/Logging=Logging",
                width = 600,

                f:row {
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/ShowLogfile=Show logfile",
                        action = function(button)
                            LrShell.revealInShell(Util.getLogfilePath())
                        end,
                        width = share 'bottomButtons',
                    },
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/CopyLogToDesktop=Copy logfiles to Desktop",
                        action = function(button)
                            LrTasks.startAsyncTask(function()
                                Util.copyLogfilesToDesktop()
                            end)
                        end,
                        width = share 'bottomButtons',
                    },
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/UpdateCheck=Check for updates",
                        action = function(button)
                            LrTasks.startAsyncTask(function()
                                UpdateCheck.checkForNewVersion()
                            end)
                        end,
                        width = share 'bottomButtons',
                    },
                },
                f:row {
                    f:checkbox {
                        value = bind 'periodicalUpdateCheck',
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/periodUpdateCheck=Periodically check for Updates",
                    },
                },
            },
        },
        {
            title = LOC "$$$/LrGeniusAI/PluginInfo/Credits=CREDITS",
            f:group_box {
                width = 600,
                title = LOC "$$$/LrGeniusAI/PluginInfo/Credits=CREDITS",
                f:row {
                    f:static_text {
                        title = Defaults.copyrightString,
                        width_in_chars = 140,
                        height_in_lines = 20,
                    },
                },
            },
        },
    }
end

function PluginInfoDialogSections.sectionsForTopOfDialog(f, propertyTable)
    local bind = LrView.bind
    local share = LrView.share

    local groupBoxWidth = 600

    propertyTable.models = {}

    propertyTable.promptTitleMenu = f:popup_menu {
        items = bind 'promptTitles',
        value = bind 'prompt',
    }

    return {

        {
            bind_to_object = propertyTable,

            title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/header=LrGeniusAI configuration",

            f:group_box {
                width = groupBoxWidth,
                title = LOC "$$$/LrGeniusAI/Health/SummaryTitle=System Health",

                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Health/SummaryTitle=System Health",
                        font = "<system/bold>",
                        alignment = 'right',
                    },
                    f:static_text {
                        title = bind {
                            key = 'healthStatus',
                            transform = function(v)
                                if v == "healthy" then return LOC "$$$/LrGeniusAI/Health/StatusHealthy=Everything looks good!" end
                                if v == "warning" then return LOC "$$$/LrGeniusAI/Health/StatusWarning=Some features might not work correctly." end
                                return LOC "$$$/LrGeniusAI/Health/StatusCritical=Critical issues detected. Plugin cannot function."
                            end
                        },
                        text_color = bind 'healthColor',
                    },
                },
                f:row {
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/Health/RunWizard=Run Setup Wizard",
                        action = function() OnboardingWizard.show(true) end,
                    },
                },
            },
            f:row {
                visible = bind {
                    key = 'healthIssues',
                    transform = function(v) return v ~= "" end
                },
                f:spacer { width = share 'labelWidth' },
                f:static_text {
                    fill_horizontal = 1,
                    title = bind 'healthIssues',
                    text_color = bind 'healthColor',
                    size = "small",
                    wrap = true,
                },
            },
            f:row {
                f:push_button {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/Docs=Read documentation online",
                    action = function(button)
                        LrHttp.openUrlInBrowser("https://github.com/LrGenius/LrGeniusAI/wiki")
                    end,
                },
            },
            f:group_box {
                width = groupBoxWidth,
                title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/ApiKeys=API keys",
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/GoogleApiKey=Google API key",
                        alignment = 'right',
                        width = share 'apiKeyLabelWidth',
                    },
                    f:edit_field {
                        value = bind 'geminiApiKey',
                        fill_horizontal = 1,
                    },
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/GetAPIkey=Get API key",
                        action = function(button)
                            LrHttp.openUrlInBrowser("https://aistudio.google.com/app/apikey")
                        end,
                        width = share 'apiKeyButtonWidth',
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/ChatGPTApiKey=ChatGPT API key",
                        alignment = 'right',
                        width = share 'apiKeyLabelWidth',
                    },
                    f:edit_field {
                        value = bind 'chatgptApiKey',
                        fill_horizontal = 1,
                    },
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/GetAPIkey=Get API key",
                        action = function(button)
                            LrHttp.openUrlInBrowser("https://platform.openai.com/api-keys")
                        end,
                        width = share 'apiKeyButtonWidth',
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/VertexProjectId=Vertex AI Project ID",
                        alignment = 'right',
                        width = share 'apiKeyLabelWidth',
                    },
                    f:edit_field {
                        value = bind 'vertexProjectId',
                        fill_horizontal = 1,
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/VertexLocation=Vertex AI Location",
                        alignment = 'right',
                        width = share 'apiKeyLabelWidth',
                    },
                    f:edit_field {
                        value = bind 'vertexLocation',
                        width_in_chars = 20,
                    },
                },
            },
            f:group_box {
                width = groupBoxWidth,
                title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/BackendServer=Backend Server",
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/BackendServerUrl=Backend server URL (IP/hostname)",
                        alignment = 'right',
                        width = share 'labelWidth',
                    },
                    f:edit_field {
                        value = bind 'backendServerUrl',
                        fill_horizontal = 1,
                    },
                },
                f:row {
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/GeneratePhotoIds=Generate hash-based photo IDs (catalog only)",
                        width = share 'longBackendButtonWidth',
                        action = function(button)
                            LrTasks.startAsyncTask(function()
                                local ok, msg = SearchIndexAPI.generateGlobalPhotoIdsForCatalog()
                                if ok then
                                    LrDialogs.message(
                                        LOC "$$$/LrGeniusAI/PluginInfo/PhotoIdGenTitle=Photo-ID Generation",
                                        msg or LOC "$$$/LrGeniusAI/common/GenerationCompleted=Generation completed.")
                                else
                                    LrDialogs.message(
                                        LOC "$$$/LrGeniusAI/PluginInfo/PhotoIdGenFailed=Photo-ID Generation failed",
                                        msg or LOC "$$$/LrGeniusAI/common/UnknownError=Unknown error", "critical")
                                end
                            end)
                        end,
                    },
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/MigratePhotoIds=Migrate existing DB IDs to photo_id",
                        width = share 'longBackendButtonWidth',
                        action = function(button)
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
                                    log:error("Photo-ID migration crashed.")
                                    LrDialogs.message(
                                        LOC "$$$/LrGeniusAI/PluginInfo/PhotoIdMigrateFailed=Photo-ID Migration failed",
                                        tostring(ok), "critical")
                                elseif ok then
                                    LrDialogs.message(
                                        LOC "$$$/LrGeniusAI/PluginInfo/PhotoIdMigrateTitle=Photo-ID Migration",
                                        msg or LOC "$$$/LrGeniusAI/common/MigrationCompleted=Migration completed.")
                                else
                                    LrDialogs.message(
                                        LOC "$$$/LrGeniusAI/PluginInfo/PhotoIdMigrateFailed=Photo-ID Migration failed",
                                        msg or LOC "$$$/LrGeniusAI/common/UnknownError=Unknown error", "critical")
                                end
                            end)
                        end,
                    },
                },
                f:row {
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/ClaimPhotos=Claim photos for this catalog",
                        width = share 'backendButtonWidth',
                        action = function(button)
                            LrTasks.startAsyncTask(function()
                                local progressScope = LrProgressScope({
                                    title = LOC "$$$/LrGeniusAI/SearchIndexAPI/claimingPhotos=Claiming photos for this catalog...",
                                    functionContext = nil,
                                })
                                local ok, err, result = SearchIndexAPI.claimPhotosForCatalog(progressScope)
                                progressScope:done()
                                if ok then
                                    local msg = result and
                                        (LOC "$$$/LrGeniusAI/PluginInfo/ClaimedPrefix=Claimed: " .. tostring(result.claimed) .. (result.errors and result.errors > 0 and (LOC "$$$/LrGeniusAI/PluginInfo/ClaimedErrors=; errors: " .. tostring(result.errors)) or "")) or
                                        LOC "$$$/LrGeniusAI/common/Done=Done."
                                    LrDialogs.message(LOC "$$$/LrGeniusAI/PluginInfo/ClaimPhotosTitle=Claim photos", msg)
                                else
                                    LrDialogs.message(
                                        LOC "$$$/LrGeniusAI/PluginInfo/ClaimPhotosFailed=Claim photos failed",
                                        tostring(err or LOC "$$$/LrGeniusAI/common/UnknownError=Unknown error"),
                                        "critical")
                                end
                            end)
                        end,
                    },
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/ShowDbStats=Show DB stats",
                        width = share 'backendButtonWidth',
                        action = function(button)
                            LrTasks.startAsyncTask(function()
                                local stats, err = SearchIndexAPI.getStats()
                                if stats then
                                    LrDialogs.message(LOC "$$$/LrGeniusAI/PluginInfo/DbStatsTitle=Database statistics",
                                        SearchIndexAPI.formatStats(stats), "info")
                                else
                                    LrDialogs.message(
                                        LOC "$$$/LrGeniusAI/PluginInfo/DbStatsFailed=Database statistics failed",
                                        tostring(err or LOC "$$$/LrGeniusAI/common/UnknownError=Unknown error"),
                                        "critical")
                                end
                            end)
                        end,
                    },
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/DownloadDbBackup=Download DB backup",
                        width = share 'backendButtonWidth',
                        action = function(button)
                            LrTasks.startAsyncTask(function()
                                local result, path = SearchIndexAPI.downloadDatabaseBackup()
                                if result then
                                    LrShell.revealInShell(path)
                                    LrDialogs.message(
                                        LOC "$$$/LrGeniusAI/PluginInfo/DbBackupDownloaded=Database backup downloaded.",
                                        path)
                                else
                                    LrDialogs.message(
                                        LOC "$$$/LrGeniusAI/PluginInfo/DbBackupFailed=Database backup failed",
                                        tostring(result or LOC "$$$/LrGeniusAI/common/UnknownError=Unknown error"),
                                        "critical")
                                end
                            end)
                        end,
                    },
                },
                f:row {
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/CheckVersions=Check Plugin/Backend versions",
                        width = share 'backendButtonWidth',
                        action = function(button)
                            LrTasks.startAsyncTask(function()
                                local result, err = SearchIndexAPI.checkVersionCompatibility()
                                if result then
                                    local backendTag = tostring(result.backend_release_tag or
                                        ("v" .. tostring(result.backend_version or LOC "$$$/LrGeniusAI/common/Unknown=unknown")))
                                    local pluginTag = tostring(result.plugin_release_tag or
                                        ("v" .. tostring(result.plugin_version or LOC "$$$/LrGeniusAI/common/Unknown=unknown")))
                                    local buildInfo = LOC "$$$/LrGeniusAI/PluginInfo/PluginBuild=Plugin build: " ..
                                        tostring(result.plugin_build or "n/a") ..
                                        "\n" ..
                                        LOC "$$$/LrGeniusAI/PluginInfo/BackendBuild=Backend build: " ..
                                        tostring(result.backend_build or "n/a")

                                    if result.compatible then
                                        LrDialogs.message(
                                            LOC "$$$/LrGeniusAI/PluginInfo/VersionCheckPassed=Version check passed",
                                            LOC "$$$/LrGeniusAI/PluginInfo/VersionMatch=Plugin and backend versions match.\n" ..
                                            LOC "$$$/LrGeniusAI/PluginInfo/PluginPrefix=Plugin: " .. pluginTag .. "\n" ..
                                            LOC "$$$/LrGeniusAI/PluginInfo/BackendPrefix=Backend: " ..
                                            backendTag .. "\n\n" ..
                                            buildInfo
                                        )
                                    else
                                        LrDialogs.message(
                                            LOC "$$$/LrGeniusAI/PluginInfo/VersionMismatch=Version mismatch",
                                            LOC "$$$/LrGeniusAI/PluginInfo/VersionDiffer=Plugin and backend versions differ.\n" ..
                                            LOC "$$$/LrGeniusAI/PluginInfo/PluginPrefix=Plugin: " .. pluginTag .. "\n" ..
                                            LOC "$$$/LrGeniusAI/PluginInfo/BackendPrefix=Backend: " ..
                                            backendTag .. "\n" ..
                                            LOC "$$$/LrGeniusAI/PluginInfo/ReasonPrefix=Reason: " ..
                                            tostring(result.reason or LOC "$$$/LrGeniusAI/common/Unknown=unknown") ..
                                            "\n\n" ..
                                            buildInfo,
                                            "warning"
                                        )
                                    end
                                else
                                    LrDialogs.message(
                                        LOC "$$$/LrGeniusAI/PluginInfo/VersionCheckFailed=Version check failed",
                                        tostring(err or LOC "$$$/LrGeniusAI/common/UnknownError=Unknown error"),
                                        "critical")
                                end
                            end)
                        end,
                    },

                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/RestartBackend=Restart Backend",
                        width = share 'backendButtonWidth',
                        action = function(button)
                            LrTasks.startAsyncTask(function()
                                local progressScope = LrProgressScope({
                                    title = LOC "$$$/LrGeniusAI/PluginInfo/Restarting=Restarting...",
                                    functionContext = nil,
                                })
                                local ok, err = SearchIndexAPI.restartBackend()
                                progressScope:done()
                                if ok then
                                    LrDialogs.message(LOC "$$$/LrGeniusAI/PluginInfo/RestartBackend=Restart Backend",
                                        LOC "$$$/LrGeniusAI/PluginInfo/RestartSuccess=Backend restarted successfully.")
                                else
                                    LrDialogs.message(LOC "$$$/LrGeniusAI/PluginInfo/RestartBackend=Restart Backend",
                                        LOC("$$$/LrGeniusAI/PluginInfo/RestartFailed=Failed to restart backend: ^1",
                                            tostring(err)), "critical")
                                end
                            end)
                        end,
                    },
                },
            },
            f:group_box {
                width = groupBoxWidth,
                title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/ollamaSettings=Ollama Settings",
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/OllamaBaseUrl=Ollama Base URL",
                        width = share 'setupLabelWidth',
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:edit_field {
                        value = bind 'ollamaBaseUrl',
                        width_in_chars = 40,
                    },
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/OllamaSetup=Setup Ollama",
                        action = function(button)
                            LrHttp.openUrlInBrowser("https://github.com/LrGenius/LrGeniusAI/wiki/Help-Ollama-Setup")
                        end,
                        width = share 'setupButtonWidth',
                    },
                },
            },
            f:group_box {
                width = groupBoxWidth,
                title = LOC "$$$/LrGeniusAI/PluginInfo/LmStudioSettings=LM Studio Settings",
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/LmStudioUrl=LM Studio Base URL (host:port)",
                        width = share 'setupLabelWidth',
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:edit_field {
                        value = bind 'lmstudioBaseUrl',
                        width_in_chars = 40,
                    },
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/SetupLmStudio=Setup LM Studio",
                        action = function(button)
                            LrHttp.openUrlInBrowser("https://github.com/LrGenius/LrGeniusAI/wiki/Help-LM-Studio-Setup")
                        end,
                        width = share 'setupButtonWidth',
                    },
                },
            },
            f:group_box {
                width = groupBoxWidth,
                title = LOC "$$$/LrGeniusAI/UI/Prompts=Prompts",
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        alignment = 'right',
                        width = share 'promptLabelWidth',
                        title = LOC "$$$/LrGeniusAI/UI/PromptTitle=Title",
                    },
                    propertyTable.promptTitleMenu,
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/add=Add",
                        action = function(button)
                            local newName = PromptConfigProvider.addPrompt(propertyTable)
                        end,
                    },
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/delete=Delete",
                        action = function(button)
                            PromptConfigProvider.deletePrompt(propertyTable)
                        end,
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        alignment = 'right',
                        width = share 'promptLabelWidth',
                        title = LOC "$$$/LrGeniusAI/PromptConfig/PromptField=Prompt",
                    },
                    f:scrolled_view {
                        horizontal_scroller = false,
                        vertical_scroller = true,
                        fill_horizontal = 1,
                        height_in_lines = 15,
                        width = 500,
                        f:edit_field {
                            value = bind 'selectedPrompt',
                            fill_horizontal = 1,
                            height_in_lines = 30,
                            wraps = true,
                            width = 480,
                        },
                    },
                },
            },
            f:group_box {
                width = groupBoxWidth,
                title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/exportSettings=Export settings",
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/exportSize=Export size in pixel (long edge)",
                        alignment = 'right',
                        width = share 'labelWidth',
                    },
                    f:popup_menu {
                        value = bind 'exportSize',
                        items = Defaults.exportSizes,
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/exportQuality=Export JPEG quality in percent",
                        alignment = 'right',
                        width = share 'labelWidth',
                    },
                    f:slider {
                        value = bind 'exportQuality',
                        min = 1,
                        max = 100,
                        integral = true,
                        immediate = true,
                        fill_horizontal = 1,
                    },
                    f:static_text {
                        title = bind 'exportQuality',
                        width_in_chars = 5,
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:spacer { width = share 'labelWidth' },
                    f:checkbox {
                        value = bind 'usePreviewThumbnails',
                        title = LOC "$$$/LrGeniusAI/PluginInfo/UsePreviewThumbnails=Use Lightroom previews for faster indexing",
                    },
                },
            },
            f:group_box {
                width = groupBoxWidth,
                f:checkbox {
                    value = bind 'useClip',
                    title = LOC "$$$/LrGeniusAI/PluginInfo/UseOpenClip=Use OpenCLIP AI model for advanced search",
                },
                f:group_box {
                    fill_horizontal = 1,
                    title = LOC "$$$/LrGeniusAI/PluginInfo/AdvancedSearchTitle=Advanced search",
                    f:row {
                        fill_horizontal = 1,
                        f:checkbox {
                            value = bind 'clipReady',
                            enabled = false,
                            title = LOC "$$$/LrGeniusAI/PluginInfo/OpenClipReady=OpenCLIP AI model is ready",
                        },
                        f:push_button {
                            title = LOC "$$$/LrGeniusAI/PluginInfo/DownloadNow=Download now",
                            action = function(button)
                                LrTasks.startAsyncTask(function()
                                    SearchIndexAPI.startClipDownload()
                                end)
                            end,
                            enabled = bind 'useClip',
                        }
                    },
                }
            },
            f:group_box {
                width = groupBoxWidth,
                title = LOC "$$$/LrGeniusAI/Training/SectionTitle=My Style Profile",
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Training/EngineStatus=Style Engine Status:",
                        alignment = 'right',
                        width = share 'labelWidth',
                    },
                    f:static_text {
                        title = bind 'styleReadyText',
                        text_color = bind 'styleReadyColor',
                        font = "<system/bold>",
                        fill_horizontal = 1,
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Training/SavedExamples=Saved training examples:",
                        alignment = 'right',
                        width = share 'labelWidth',
                    },
                    f:static_text {
                        title = bind { key = 'trainingCount', transform = function(v) return tostring(v or 0) end },
                        fill_horizontal = 1,
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Training/TopScenes=Top Scene Types:",
                        alignment = 'right',
                        width = share 'labelWidth',
                    },
                    f:static_text {
                        fill_horizontal = 1,
                        title = bind { key = 'styleStats', transform = function(s)
                            if not s or not s.scene_distribution then return "..." end
                            local sorted = {}
                            for k, v in pairs(s.scene_distribution) do
                                table.insert(sorted, { name = k, count = v })
                            end
                            table.sort(sorted, function(a, b) return a.count > b.count end)
                            local top = {}
                            for i = 1, math.min(3, #sorted) do
                                local name = sorted[i].name:gsub("^scene_", ""):gsub("_", " ")
                                table.insert(top, name:sub(1, 1):upper() .. name:sub(2))
                            end
                            return #top > 0 and table.concat(top, ", ") or "None yet"
                        end },
                        font = "<system/italic>",
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Training/LearnedCameras=Learned Cameras:",
                        alignment = 'right',
                        width = share 'labelWidth',
                    },
                    f:static_text {
                        fill_horizontal = 1,
                        title = bind { key = 'styleStats', transform = function(s)
                            if not s or not s.camera_distribution then return "..." end
                            local sorted = {}
                            for k, v in pairs(s.camera_distribution) do
                                table.insert(sorted, { name = k, count = v })
                            end
                            table.sort(sorted, function(a, b) return a.count > b.count end)
                            local top = {}
                            for i = 1, math.min(5, #sorted) do
                                table.insert(top, string.format("%s (%d)", sorted[i].name, sorted[i].count))
                            end
                            return #top > 0 and table.concat(top, "\n") or "None yet"
                        end },
                        font = "<system/italic>",
                        height_in_lines = 5,
                        wrap = true,
                    },
                },
                f:row {
                    fill_horizontal = 1,
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Training/StyleDNA=Style DNA (Average):",
                        alignment = 'right',
                        width = share 'labelWidth',
                    },
                    f:static_text {
                        fill_horizontal = 1,
                        title = bind { key = 'styleStats', transform = function(s)
                            if not s or not s.exposure then return "..." end
                            local e = s.exposure
                            local parts = {}
                            if e.mean_luminance then
                                table.insert(parts,
                                    string.format("Lum: %.0f%%", e.mean_luminance * 100))
                            end
                            if e.mean_contrast then
                                table.insert(parts,
                                    string.format("Con: %.0f%%", e.mean_contrast * 100))
                            end
                            if e.mean_colorfulness then
                                table.insert(parts,
                                    string.format("Color: %.0f%%", e.mean_colorfulness * 100))
                            end
                            return #parts > 0 and table.concat(parts, " | ") or "..."
                        end },
                    },
                },
                f:row {
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/common/Refresh=Refresh",
                        action = function(button)
                            propertyTable.refreshStyleStats()
                        end,
                    },
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/Training/ClearAll=Clear all training examples",
                        action = function(button)
                            local confirm = LrDialogs.confirm(
                                LOC "$$$/LrGeniusAI/Training/ClearConfirmTitle=Clear Training Examples",
                                LOC "$$$/LrGeniusAI/Training/ClearConfirmMsg=This will permanently delete all saved training examples. The Style Engine will be reset to Cold Start. Continue?",
                                LOC "$$$/LrGeniusAI/Training/ClearConfirmOk=Delete All",
                                LOC "$$$/LrGeniusAI/Training/ClearConfirmCancel=Cancel"
                            )
                            if confirm == "ok" then
                                LrTasks.startAsyncTask(function()
                                    local ok, err = SearchIndexAPI.clearAllTrainingExamples()
                                    if ok then
                                        propertyTable.refreshStyleStats()
                                        LrDialogs.message(
                                            LOC "$$$/LrGeniusAI/Training/ClearedTitle=Training Data Cleared",
                                            LOC "$$$/LrGeniusAI/Training/ClearedMsg=All training examples have been removed.",
                                            "info"
                                        )
                                    else
                                        ErrorHandler.handleError(
                                            LOC "$$$/LrGeniusAI/Training/ClearFailedTitle=Clear Failed",
                                            tostring(err or "Unknown error"))
                                    end
                                end)
                            end
                        end,
                    },
                },
            },
        },
    }
end

function PluginInfoDialogSections.endDialog(propertyTable)
    prefs.geminiApiKey = propertyTable.geminiApiKey
    prefs.chatgptApiKey = propertyTable.chatgptApiKey
    prefs.vertexProjectId = (propertyTable.vertexProjectId and propertyTable.vertexProjectId:gsub("^%s*(.-)%s*$", "%1")) or
        ""
    prefs.vertexLocation = (propertyTable.vertexLocation and propertyTable.vertexLocation:gsub("^%s*(.-)%s*$", "%1")) or
        "us-central1"
    prefs.exportSize = propertyTable.exportSize
    prefs.exportQuality = propertyTable.exportQuality
    prefs.usePreviewThumbnails = (propertyTable.usePreviewThumbnails ~= false)

    prefs.prompt = propertyTable.prompt
    prefs.prompts = propertyTable.prompts

    prefs.logging = propertyTable.logging
    if propertyTable.logging then
        log:enable('logfile')
    else
        log:disable()
    end

    prefs.periodicalUpdateCheck = propertyTable.periodicalUpdateCheck

    prefs.useClip = propertyTable.useClip

    if propertyTable.backendServerUrl and propertyTable.backendServerUrl:gsub("^%s*(.-)%s*$", "%1") ~= "" then
        prefs.backendServerUrl = propertyTable.backendServerUrl:gsub("^%s*(.-)%s*$", "%1")
    else
        prefs.backendServerUrl = Defaults.defaultBackendServerUrl
    end

    if propertyTable.ollamaBaseUrl and propertyTable.ollamaBaseUrl:gsub("^%s*(.-)%s*$", "%1") ~= "" then
        prefs.ollamaBaseUrl = propertyTable.ollamaBaseUrl:gsub("^%s*(.-)%s*$", "%1")
    else
        prefs.ollamaBaseUrl = Defaults.defaultOllamaBaseUrl
    end

    if propertyTable.lmstudioBaseUrl and propertyTable.lmstudioBaseUrl:gsub("^%s*(.-)%s*$", "%1") ~= "" then
        prefs.lmstudioBaseUrl = propertyTable.lmstudioBaseUrl:gsub("^%s*(.-)%s*$", "%1")
    else
        prefs.lmstudioBaseUrl = Defaults.defaultLmStudioBaseUrl
    end

    propertyTable.keepChecksRunning = false -- Stop the async task checking for CLIP readiness
end
