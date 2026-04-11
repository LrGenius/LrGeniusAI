OnboardingWizard = {}

local LrView = import 'LrView'
local LrDialogs = import 'LrDialogs'
local LrHttp = import 'LrHttp'
local LrTasks = import 'LrTasks'
local LrPrefs = import 'LrPrefs'
local LrLocalization = import 'LrLocalization'
local LrBinding = import 'LrBinding'

local LOC = LrLocalization.LOC

function OnboardingWizard.show(manualTrigger)
    LrTasks.startAsyncTask(function()
        local propertyTable = LrBinding.makePropertyTable(LrFunctionContext.createContext())
        
        -- Initial states
        propertyTable.currentPage = 1
        propertyTable.backendRunning = SearchIndexAPI.pingServer()
        propertyTable.clipReady = SearchIndexAPI.isClipReady()
        propertyTable.geminiApiKey = prefs.geminiApiKey or ""
        propertyTable.chatgptApiKey = prefs.chatgptApiKey or ""
        
        local f = LrView.osFactory()
        local bind = LrView.bind
        local share = LrView.share

        local function updateBackendStatus()
            propertyTable.backendRunning = SearchIndexAPI.pingServer()
        end

        local function startBackend()
            propertyTable.backendRunning = "starting"
            LrTasks.startAsyncTask(function()
                SearchIndexAPI.startServer({ readyTimeoutSeconds = 30 })
                updateBackendStatus()
            end)
        end

        local function getPage(pageIndex)
            if pageIndex == 1 then
                -- Welcome Page
                return f:column {
                    spacing = f:label_spacing(),
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/WelcomeTitle",
                        font = "<system/bold>",
                        size = "large",
                    },
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/WelcomeMessage",
                        width_in_chars = 60,
                        wrap = true,
                    },
                }
            elseif pageIndex == 2 then
                -- Backend Page
                return f:column {
                    spacing = f:label_spacing(),
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/BackendTitle",
                        font = "<system/bold>",
                    },
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/BackendDesc",
                        width_in_chars = 60,
                        wrap = true,
                    },
                    f:row {
                        f:static_text {
                            title = LOC "$$$/LrGeniusAI/Onboarding/BackendStatus",
                        },
                        f:static_text {
                            title = bind {
                                key = 'backendRunning',
                                transform = function(v)
                                    if v == true then return LOC "$$$/LrGeniusAI/Onboarding/BackendRunning" end
                                    if v == "starting" then return LOC "$$$/LrGeniusAI/Onboarding/BackendStarting" end
                                    return LOC "$$$/LrGeniusAI/Onboarding/BackendError"
                                end
                            },
                            text_color = bind {
                                key = 'backendRunning',
                                transform = function(v)
                                    if v == true then return { 0, 0.8, 0 } end
                                    if v == "starting" then return { 0.8, 0.8, 0 } end
                                    return { 0.8, 0, 0 }
                                end
                            }
                        },
                    },
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/common/Start",
                        action = startBackend,
                        enabled = bind {
                            key = 'backendRunning',
                            transform = function(v) return v ~= true and v ~= "starting" end
                        }
                    },
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/BackendHint",
                        size = "small",
                        width_in_chars = 60,
                        wrap = true,
                    },
                }
            elseif pageIndex == 3 then
                -- Providers Page
                return f:column {
                    spacing = f:label_spacing(),
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/ProvidersTitle",
                        font = "<system/bold>",
                    },
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/ProvidersDesc",
                        width_in_chars = 60,
                        wrap = true,
                    },
                    f:group_box {
                        title = LOC "$$$/LrGeniusAI/Onboarding/GeminiTitle",
                        f:row {
                            f:static_text { title = LOC "$$$/LrGeniusAI/Onboarding/ApiKeyLabel", width = share 'label' },
                            f:edit_field { value = bind 'geminiApiKey', width_in_chars = 40 },
                            f:push_button {
                                title = "?",
                                action = function() LrHttp.openUrlInBrowser("https://aistudio.google.com/app/apikey") end
                            }
                        }
                    },
                    f:group_box {
                        title = LOC "$$$/LrGeniusAI/Onboarding/ChatGPTTitle",
                        f:row {
                            f:static_text { title = LOC "$$$/LrGeniusAI/Onboarding/ApiKeyLabel", width = share 'label' },
                            f:edit_field { value = bind 'chatgptApiKey', width_in_chars = 40 },
                            f:push_button {
                                title = "?",
                                action = function() LrHttp.openUrlInBrowser("https://platform.openai.com/api-keys") end
                            }
                        }
                    },
                    f:row {
                        f:push_button {
                            title = LOC "$$$/LrGeniusAI/Onboarding/LocalTitle",
                            action = function() LrHttp.openUrlInBrowser("https://lrgenius.com/help/ollama-setup/") end
                        }
                    }
                }
            elseif pageIndex == 4 then
                -- Semantic Page
                return f:column {
                    spacing = f:label_spacing(),
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/SemanticTitle",
                        font = "<system/bold>",
                    },
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/SemanticDesc",
                        width_in_chars = 60,
                        wrap = true,
                    },
                    f:row {
                        f:checkbox {
                            title = LOC "$$$/LrGeniusAI/Onboarding/ClipAlreadyDownloaded",
                            value = bind 'clipReady',
                            enabled = false,
                        },
                        f:push_button {
                            title = LOC "$$$/LrGeniusAI/Onboarding/DownloadClip",
                            action = function()
                                LrTasks.startAsyncTask(function()
                                    SearchIndexAPI.startClipDownload()
                                    propertyTable.clipReady = SearchIndexAPI.isClipReady()
                                end)
                            end,
                            enabled = bind {
                                key = 'clipReady',
                                transform = function(v) return not v end
                            }
                        }
                    }
                }
            elseif pageIndex == 5 then
                -- Finish Page
                return f:column {
                    spacing = f:label_spacing(),
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/FinishTitle",
                        font = "<system/bold>",
                        size = "large",
                    },
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/Onboarding/FinishDesc",
                        width_in_chars = 60,
                        wrap = true,
                    },
                }
            end
        end

        local contents = f:column {
            spacing = f:label_spacing(),
            f:simple_list {
                f:column {
                    id = "page_container",
                    propertyTable.currentPage == 1 and getPage(1) or f:row{}
                }
            }
        }

        -- Update contents when currentPage changes
        propertyTable:addObserver('currentPage', function(props, key, value)
            -- This is a bit tricky in LR SDK as we can't easily swap views in a dialog
            -- We might need to use invisible rows or a more complex approach.
            -- For simplicity in this wizard, we will use a single view and update its children if possible,
            -- or recreate the dialog (not ideal).
            -- Actually, let's use a hidden/visible approach with all pages pre-rendered.
        end)

        -- Improved multi-page logic for LR View
        local pages = {}
        for i = 1, 5 do
            pages[i] = f:column {
                visible = bind {
                    key = 'currentPage',
                    transform = function(v) return v == i end
                },
                getPage(i)
            }
        end

        local dialogContents = f:column {
            spacing = f:label_spacing(),
            pages[1], pages[2], pages[3], pages[4], pages[5],
            f:separator { fill_horizontal = 1 },
            f:row {
                fill_horizontal = 1,
                f:push_button {
                    title = LOC "$$$/LrGeniusAI/Onboarding/Skip",
                    action = function(d) d:done("skip") end,
                    visible = bind {
                        key = 'currentPage',
                        transform = function(v) return v < 5 end
                    }
                },
                f:spacer { fill_horizontal = 1 },
                f:push_button {
                    title = LOC "$$$/LrGeniusAI/Onboarding/Back",
                    enabled = bind {
                        key = 'currentPage',
                        transform = function(v) return v > 1 end
                    },
                    action = function() propertyTable.currentPage = propertyTable.currentPage - 1 end,
                    visible = bind {
                        key = 'currentPage',
                        transform = function(v) return v < 5 end
                    }
                },
                f:push_button {
                    title = bind {
                        key = 'currentPage',
                        transform = function(v)
                            if v == 5 then return LOC "$$$/LrGeniusAI/Onboarding/Finish" end
                            return LOC "$$$/LrGeniusAI/Onboarding/Next"
                        end
                    },
                    action = function(d)
                        if propertyTable.currentPage < 5 then
                            propertyTable.currentPage = propertyTable.currentPage + 1
                        else
                            d:done("ok")
                        end
                    end
                },
            }
        }

        local result = LrDialogs.presentModalDialog({
            title = LOC "$$$/LrGeniusAI/Onboarding/WizardTitle",
            contents = dialogContents,
            actionVerb = "OK", -- Overridden by my own buttons
            cancelVerb = "Cancel",
            resizable = false,
        })

        if result == "ok" or result == "skip" then
            prefs.onboardingCompleted = true
            -- Save settings
            prefs.geminiApiKey = propertyTable.geminiApiKey
            prefs.chatgptApiKey = propertyTable.chatgptApiKey
            log:info("Onboarding wizard completed with result: " .. tostring(result))
        end
    end)
end

return OnboardingWizard
