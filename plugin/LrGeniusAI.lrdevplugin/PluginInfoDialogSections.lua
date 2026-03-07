PluginInfoDialogSections = {}

function PluginInfoDialogSections.startDialog(propertyTable)

    propertyTable.useClip = prefs.useClip

    propertyTable.clipReady = false
    propertyTable.keepChecksRunning = true
    LrTasks.startAsyncTask(function (context)
            propertyTable.clipReady = SearchIndexAPI.isClipReady()
            while propertyTable.keepChecksRunning  do
                LrTasks.sleep(1)
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
end

function PluginInfoDialogSections.sectionsForBottomOfDialog(f, propertyTable)
    local bind = LrView.bind
    local share = LrView.share

    return {
        {
            bind_to_object = propertyTable,
            title = "Logging",

            f:row {
                f:static_text {
                    title = Util.getLogfilePath(),
                },
            },
            f:row {
                f:checkbox {
                    value = bind 'logging',
                    enabled = false,
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/enableDebugLogging=Enable logging",
                    alignment = 'right',
                },
                f:push_button {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/ShowLogfile=Show logfile",
                    action = function (button)
                        LrShell.revealInShell(Util.getLogfilePath())
                    end,
                },
                f:push_button {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/CopyLogToDesktop=Copy logfiles to Desktop",
                    action = function (button)
                        Util.copyLogfilesToDesktop()
                    end,
                },
            },
            f:row {
                f:checkbox {
                    value = bind 'periodicalUpdateCheck',
                },
                f:static_text {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/periodUpdateCheck=Periodically check for Updates",
                    alignment = 'right',
                },
            },
            f:row {
                f:push_button {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/UpdateCheck=Check for updates",
                    action = function (button)
                        LrTasks.startAsyncTask(function ()
                            UpdateCheck.checkForNewVersion()
                        end)
                    end,
                },
            },
        },
        {
            title = "CREDITS",
            f:row {
                f:static_text {
                    title = Defaults.copyrightString,
                    width_in_chars = 140,
                    height_in_lines = 20,
                },
            },
        },
    }
end

function PluginInfoDialogSections.sectionsForTopOfDialog(f, propertyTable)

    local bind = LrView.bind
    local share = LrView.share

    propertyTable.models = {}
    
    propertyTable.promptTitleMenu = f:popup_menu {
        items = bind 'promptTitles',
        value = bind 'prompt',
    }

    return {

        {
            bind_to_object = propertyTable,

            title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/header=LrGeniusAI configuration",

            f:row {
                f:push_button {
                    title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/Docs=Read documentation online",
                    action = function(button) 
                        LrHttp.openUrlInBrowser("https://lrgenius.com/help/")
                    end,
                },
            },
            f:group_box {
                width = share 'groupBoxWidth',
                title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/ApiKeys=API keys",
                f:row {
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/GoogleApiKey=Google API key",
                        -- alignment = 'right',
                        width = share 'labelWidth'
                    },
                    f:edit_field {
                        value = bind 'geminiApiKey',
                        width = share 'inputWidth',
                        width_in_chars = 30,
                    },
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/GetAPIkey=Get API key",
                        action = function(button) 
                            LrHttp.openUrlInBrowser("https://aistudio.google.com/app/apikey")                           
                        end,
                        width = share 'apiButtonWidth',
                    },
                },
                f:row {
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/ChatGPTApiKey=ChatGPT API key",
                        -- alignment = 'right',
                        width = share 'labelWidth'
                    },
                    f:edit_field {
                        value = bind 'chatgptApiKey',
                        width = share 'inputWidth',
                        width_in_chars = 30,
                    },
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/GetAPIkey=Get API key",
                        action = function(button) 
                            LrHttp.openUrlInBrowser("https://platform.openai.com/api-keys")
                        end,
                        width = share 'apiButtonWidth',
                    },
                },
                f:row {
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/VertexProjectId=Vertex AI Project ID",
                        width = share 'labelWidth'
                    },
                    f:edit_field {
                        value = bind 'vertexProjectId',
                        width = share 'inputWidth',
                        width_in_chars = 30,
                    },
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/VertexConsole=Console",
                        action = function(button) 
                            LrHttp.openUrlInBrowser("https://console.cloud.google.com/vertex-ai")
                        end,
                        width = share 'apiButtonWidth',
                    },
                },
                f:row {
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/PluginInfo/VertexLocation=Vertex AI Location",
                        width = share 'labelWidth'
                    },
                    f:edit_field {
                        value = bind 'vertexLocation',
                        width = share 'inputWidth',
                        width_in_chars = 20,
                    },
                },
            },
            f:group_box {
                width = share 'groupBoxWidth',
                title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/BackendServer=Backend Server",
                f:row {
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/BackendServerUrl=Backend server URL (IP/hostname)",
                        width = share 'labelWidth'
                    },
                    f:edit_field {
                        value = bind 'backendServerUrl',
                        width = share 'inputWidth',
                        width_in_chars = 35,
                    },
                },
                f:row {
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/BackendServerUrlDesc=Example: http://192.168.1.100:19819 or 192.168.1.100:19819. Leave empty for localhost.",
                        width_in_chars = 60,
                        wrap = true,
                    },
                },
            },
            f:group_box {
                width = share 'groupBoxWidth',
                title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/ollamaSettings=Ollama Settings",
                f:row {
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/OllamaBaseUrl=Ollama Base URL",
                        width = share 'labelWidth'
                    },
                    f:edit_field {
                        value = bind 'ollamaBaseUrl',
                        width = share 'inputWidth',
                        width_in_chars = 35,
                    },
                    f:push_button {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/OllamaSetup=Setup Ollama",
                        action = function(button)
                            LrHttp.openUrlInBrowser("https://lrgenius.com/help/ollama-setup/")
                        end,
                        width = share 'apiButtonWidth',
                    },
                },
                f:row {
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/OllamaBaseUrlDesc=For local Ollama leave default. Use IP/hostname to run Ollama on another machine (e.g. http://192.168.1.50:11434).",
                        width_in_chars = 60,
                        wrap = true,
                    },
                },
            },
            f:group_box {
                width = share 'groupBoxWidth',
                title = LOC "$$$/LrGeniusAI/UI/Prompts=Prompts",
                f:row {
                    f:static_text {
                        width = share 'labelWidth',
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/editPrompts=Edit prompts",
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
                    f:static_text {
                        width = share 'labelWidth',
                        title = LOC "$$$/LrGeniusAI/PromptConfig/PromptField=Prompt",
                    },
                    f:scrolled_view {
                        horizontal_scroller = false,
                        vertical_scroller = true,
                        width = 500,
                        f:edit_field {
                            value = bind 'selectedPrompt',
                            width = 480,
                            height_in_lines = 30,
                            wraps = true,
                            -- enabled = false,
                        },
                    },
                },
            },
            f:group_box {
                width = share 'groupBoxWidth',
                title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/exportSettings=Export settings",
                f:row {
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/exportSize=Export size in pixel (long edge)",
                    },
                    f:popup_menu {
                        value = bind 'exportSize',
                        items = Defaults.exportSizes,
                    },
                },
                f:row {
                    f:static_text {
                        title = LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/exportQuality=Export JPEG quality in percent",
                    },
                    f:slider {
                        value = bind 'exportQuality',
                        min = 1,
                        max = 100,
                        integral = true,
                        immediate = true,
                    },
                    f:static_text {
                        title = bind 'exportQuality'
                    },
                },
            },
            f:group_box {
                width = share 'groupBoxWidth',
                f:checkbox {
                    value = bind 'useClip',
                    title = "Use OpenCLIP AI model for advanced search",
                },
                f:group_box {
                    width = share 'groupBoxWidth',
                    title = LOC "Advanced search",
                    f:row {
                        f:checkbox {
                            value = bind 'clipReady',
                            enabled = false,
                            title = "OpenCLIP AI model is ready",
                        },
                        f:push_button {
                            title = "Download now",
                            action = function (button)
                                LrTasks.startAsyncTask(function ()
                                    SearchIndexAPI.startClipDownload()
                                end)
                            end,
                            enabled = bind 'useClip',
                        }
                    },
                }
            },
        },
    }
end


function PluginInfoDialogSections.endDialog(propertyTable)
    prefs.geminiApiKey = propertyTable.geminiApiKey
    prefs.chatgptApiKey = propertyTable.chatgptApiKey
    prefs.vertexProjectId = (propertyTable.vertexProjectId and propertyTable.vertexProjectId:gsub("^%s*(.-)%s*$", "%1")) or ""
    prefs.vertexLocation = (propertyTable.vertexLocation and propertyTable.vertexLocation:gsub("^%s*(.-)%s*$", "%1")) or "us-central1"
    prefs.exportSize = propertyTable.exportSize
    prefs.exportQuality = propertyTable.exportQuality

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

    propertyTable.keepChecksRunning = false -- Stop the async task checking for CLIP readiness

end
