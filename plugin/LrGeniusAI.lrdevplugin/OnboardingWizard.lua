OnboardingWizard = {}

---
-- First-run setup wizard.
--
-- Three steps, in the order the pieces actually depend on each other: the
-- backend server has to be up before anything can be listed or downloaded, then
-- the AI provider that writes the metadata, then the models that make the
-- plug-in work without a cloud account at all.
--
-- The model step is the point of the wizard: semantic search needs the SigLIP2
-- model, and the local (MLX / llama.cpp) providers do nothing until a model is
-- on disk, so this is where the user is told to fetch them rather than
-- discovering the empty state later from a greyed-out feature.
--
function OnboardingWizard.show(manualTrigger)
	LrTasks.startAsyncTask(function()
		LrFunctionContext.callWithContext("OnboardingWizard", function(context)
			local propertyTable = LrBinding.makePropertyTable(context)

			-- Initial states with robust defaults
			propertyTable.backendRunning = SearchIndexAPI.pingServer() or false
			propertyTable.assetsReady = false
			propertyTable.assetsSizeText = ""
			propertyTable.useClip = prefs.useClip or false
			propertyTable.geminiApiKey = prefs.geminiApiKey or ""
			propertyTable.chatgptApiKey = prefs.chatgptApiKey or ""
			LocalModelCatalog.initFields(propertyTable)

			local f = LrView.osFactory()
			local bind = LrView.bind
			local share = LrView.share

			local function updateBackendStatus()
				propertyTable.backendRunning = SearchIndexAPI.pingServer()
			end

			-- The on-device models answer through /assets/status, which covers
			-- every family in one request. Same source as the Plug-in Manager's
			-- indicator, so the two cannot disagree about what is on disk.
			local function refreshAssetStatus()
				local assets = SearchIndexAPI.getAssetStatus()
				if not assets then
					return
				end
				propertyTable.assetsReady = assets.ready == true
				local missing = tonumber(assets.missing_approx_bytes) or 0
				if assets.ready == true or missing <= 0 then
					propertyTable.assetsSizeText = ""
				else
					propertyTable.assetsSizeText = LOC(
						"$$$/LrGeniusAI/Onboarding/AssetsSize=A one-time download of about ^1 GB.",
						string.format("%.1f", missing / 1e9)
					)
				end
			end

			local function refreshModelStatus()
				refreshAssetStatus()
				LocalModelCatalog.refresh(propertyTable)
			end

			local function startBackend()
				propertyTable.backendRunning = "starting"
				LrTasks.startAsyncTask(function()
					SearchIndexAPI.startServer({ readyTimeoutSeconds = 30 })
					updateBackendStatus()
					refreshModelStatus()
				end)
			end

			-- Downloads run in the background behind their own progress bar, so
			-- the dialog polls instead of waiting: a model that finishes (or a
			-- backend that comes up late) has to flip the status lines here
			-- without the user reopening the wizard.
			propertyTable.keepChecksRunning = true
			LrTasks.startAsyncTask(function()
				refreshModelStatus()
				while propertyTable.keepChecksRunning do
					LrTasks.sleep(5)
					updateBackendStatus()
					refreshModelStatus()
				end
			end)

			-- Shared shape for the two local-LLM group boxes: same three rows
			-- (status, installed, pick-and-download) against different fields.
			local function localModelGroup(args)
				return f:group_box({
					title = args.title,
					fill_horizontal = 1,
					f:static_text({
						title = args.description,
						width_in_chars = 60,
					}),
					f:spacer({ height = 5 }),
					f:row({
						f:static_text({
							title = LOC("$$$/LrGeniusAI/Onboarding/ModelStatus=Status:"),
							width = share("modelLabel"),
						}),
						f:static_text({
							title = bind(args.statusKey),
							fill_horizontal = 1,
						}),
					}),
					f:row({
						f:static_text({
							title = LOC("$$$/LrGeniusAI/Onboarding/ModelInstalled=Installed:"),
							width = share("modelLabel"),
						}),
						f:static_text({
							title = bind(args.installedKey),
							fill_horizontal = 1,
						}),
					}),
					f:row({
						f:popup_menu({
							items = bind(args.choicesKey),
							value = bind(args.choiceKey),
							enabled = bind(args.supportedKey),
							width_in_chars = 32,
						}),
						f:push_button({
							title = LOC("$$$/LrGeniusAI/Onboarding/DownloadModel=Download"),
							enabled = bind({
								key = args.choiceKey,
								transform = function(v)
									return v ~= nil
								end,
							}),
							action = function()
								LocalModelCatalog.startDownload(propertyTable, args.kind)
							end,
						}),
					}),
				})
			end

			local dialogContents = f:column({
				bind_to_object = propertyTable,
				spacing = f:control_spacing(),
				width = 650,

				f:tab_view({
					fill_horizontal = 1,

					-- BACKEND TAB
					f:tab_view_item({
						title = LOC("$$$/LrGeniusAI/Onboarding/BackendTitle=Backend Server"),
						identifier = "backend",

						f:group_box({
							title = LOC("$$$/LrGeniusAI/Onboarding/WelcomeTitle=Welcome to LrGeniusAI!"),
							fill_horizontal = 1,
							f:static_text({
								title = LOC(
									"$$$/LrGeniusAI/Onboarding/WelcomeMessage=Thank you for choosing LrGeniusAI. This wizard will guide you through the initial setup to ensure everything is running smoothly."
								),
								width_in_chars = 60,
								wrap = true,
							}),
						}),

						f:group_box({
							title = LOC("$$$/LrGeniusAI/Onboarding/BackendTitle=Backend Server"),
							fill_horizontal = 1,
							f:static_text({
								title = LOC(
									"$$$/LrGeniusAI/Onboarding/BackendDesc=LrGeniusAI requires a local backend server to process your photos. We will attempt to start it now."
								),
								width_in_chars = 60,
								wrap = true,
							}),
							f:spacer({ height = 5 }),
							f:row({
								f:static_text({
									title = LOC("$$$/LrGeniusAI/Onboarding/BackendStatus=Server Status:"),
									width = share("label"),
								}),
								Util.statusIcon(f, "backendRunning", function(v)
									return v == true
								end),
								f:static_text({
									title = bind({
										key = "backendRunning",
										transform = function(v)
											if v == true then
												return LOC("$$$/LrGeniusAI/Onboarding/BackendRunning=Running")
											end
											if v == "starting" then
												return LOC("$$$/LrGeniusAI/Onboarding/BackendStarting=Starting...")
											end
											return LOC("$$$/LrGeniusAI/Onboarding/BackendError=Failed to start")
										end,
									}),
									text_color = bind({
										key = "backendRunning",
										transform = function(v)
											if v == true then
												return LrColor(0, 0.8, 0)
											end
											if v == "starting" then
												return LrColor(0.8, 0.8, 0)
											end
											return LrColor(0.8, 0, 0)
										end,
									}),
								}),
								f:push_button({
									title = LOC("$$$/LrGeniusAI/common/Start=Start"),
									action = startBackend,
									enabled = bind({
										key = "backendRunning",
										transform = function(v)
											return v ~= true and v ~= "starting"
										end,
									}),
								}),
							}),
							f:spacer({ height = 5 }),
							f:static_text({
								title = LOC(
									"$$$/LrGeniusAI/Onboarding/BackendHint=If the server fails to start, check if another application is using port 19819 or if your firewall is blocking it."
								),
								size = "small",
								width_in_chars = 60,
								wrap = true,
							}),
						}),
					}),

					-- MODELS TAB
					f:tab_view_item({
						title = LOC("$$$/LrGeniusAI/Onboarding/ModelsTitle=AI Models"),
						identifier = "models",

						f:group_box({
							title = LOC("$$$/LrGeniusAI/Onboarding/ModelsTitle=AI Models"),
							fill_horizontal = 1,
							f:static_text({
								title = LOC(
									"$$$/LrGeniusAI/Onboarding/ModelsDesc=These models run on your own computer, and downloading them now\nis recommended: the search model is what makes search-by-content\nwork, and a local AI model generates metadata without an API key\nor an internet connection.\nDownloads run in the background — you can keep working while\nthey finish."
								),
								width_in_chars = 60,
							}),
							f:spacer({ height = 5 }),
							f:row({
								f:push_button({
									title = LOC("$$$/LrGeniusAI/Onboarding/RefreshModels=Refresh"),
									action = function()
										LrTasks.startAsyncTask(refreshModelStatus)
									end,
								}),
							}),
						}),

						-- Semantic search model (SigLIP2). Separate from the two
						-- LLM backends below: it is an embedding model for search,
						-- not something that writes metadata, and it is needed
						-- whichever AI provider the user ends up with.
						f:group_box({
							title = LOC("$$$/LrGeniusAI/Onboarding/OnDeviceTitle=On-Device Models"),
							fill_horizontal = 1,
							f:static_text({
								title = LOC(
									"$$$/LrGeniusAI/Onboarding/OnDeviceDesc=Search your catalog by what is in the picture, and identify\nanimals and plants down to the species. Both run entirely on\nthis computer."
								),
								width_in_chars = 60,
							}),
							f:static_text({
								title = bind("assetsSizeText"),
								width_in_chars = 60,
							}),
							f:spacer({ height = 5 }),
							f:checkbox({
								title = LOC(
									"$$$/LrGeniusAI/Onboarding/UseClip=Enable smart photo search (recommended)"
								),
								value = bind("useClip"),
							}),
							f:row({
								Util.statusIndicator(
									f,
									"assetsReady",
									LOC("$$$/LrGeniusAI/Onboarding/AssetsReady=The AI models are ready.")
								),
								f:push_button({
									title = LOC("$$$/LrGeniusAI/Onboarding/DownloadModels=Download AI Models"),
									action = function()
										LrTasks.startAsyncTask(function()
											-- Clicking Download is an unambiguous request for the
											-- feature, so turn it on rather than silently doing nothing.
											propertyTable.useClip = true
											prefs.useClip = true
											SearchIndexAPI.startAssetDownload()
											refreshModelStatus()
										end)
									end,
									enabled = bind({
										key = "assetsReady",
										transform = function(v)
											return not v
										end,
									}),
								}),
							}),
						}),

						-- One engine per platform, never both. macOS ships MLX
						-- only (the release build does not compile the llamacpp
						-- feature there), and MLX is Apple silicon only, so the
						-- other box would always be a dead group reporting that
						-- its backend is unsupported. Same reasoning as in
						-- PluginInfoDialogSections.sectionsForTopOfDialog.
						MAC_ENV
								and localModelGroup({
									kind = "mlx",
									title = LOC("$$$/LrGeniusAI/MlxModel/Title=Local AI Model — MLX (Apple silicon)"),
									description = LOC(
										"$$$/LrGeniusAI/Onboarding/MlxDesc=On Apple silicon, MLX is the fastest way to run an AI model\nlocally. Pick a model and download it to generate keywords,\ntitles and captions without a cloud account."
									),
									statusKey = "mlxStatusText",
									installedKey = "mlxInstalledText",
									choicesKey = "mlxDownloadChoices",
									choiceKey = "mlxDownloadChoice",
									supportedKey = "mlxSupported",
								})
							or localModelGroup({
								kind = "llm",
								title = LOC("$$$/LrGeniusAI/LocalModel/Title=Local AI Model — llama.cpp"),
								description = LOC(
									"$$$/LrGeniusAI/Onboarding/LlamaDesc=The built-in llama.cpp engine runs GGUF models with or without\na GPU. Pick a model and download it to generate keywords, titles\nand captions without a cloud account."
								),
								statusKey = "llmStatusText",
								installedKey = "llmInstalledText",
								choicesKey = "llmDownloadChoices",
								choiceKey = "llmDownloadChoice",
								supportedKey = "llmSupported",
							}),

						f:group_box({
							title = LOC("$$$/LrGeniusAI/Onboarding/FinishTitle=All Set!"),
							fill_horizontal = 1,
							f:static_text({
								title = LOC(
									"$$$/LrGeniusAI/Onboarding/FinishDesc=Configuration complete. LrGeniusAI is ready to help you manage\nyour Lightroom catalog. You can change any of this later under\nFile → Plug-in Manager → LrGeniusAI."
								),
								width_in_chars = 60,
							}),
						}),
					}),

					-- PROVIDERS TAB
					f:tab_view_item({
						title = LOC("$$$/LrGeniusAI/Onboarding/ProvidersTitle=Optional AI Providers"),
						identifier = "providers",

						f:group_box({
							title = LOC("$$$/LrGeniusAI/Onboarding/ProvidersTitle=AI Providers"),
							fill_horizontal = 1,
							f:static_text({
								title = LOC(
									"$$$/LrGeniusAI/Onboarding/ProvidersDesc=Choose which AI models you want to use for metadata generation\nand edits. Prefer to keep your photos on this machine? Skip the\nkeys and set up a local model under AI Models instead."
								),
								width_in_chars = 60,
							}),
						}),

						f:group_box({
							title = LOC("$$$/LrGeniusAI/Onboarding/GeminiTitle=Google Gemini"),
							fill_horizontal = 1,
							f:row({
								f:static_text({
									title = LOC("$$$/LrGeniusAI/Onboarding/ApiKeyLabel=API Key:"),
									width = share("label"),
								}),
								f:edit_field({ value = bind("geminiApiKey"), width_in_chars = 40 }),
								f:push_button({
									title = "?",
									action = function()
										LrHttp.openUrlInBrowser("https://aistudio.google.com/app/apikey")
									end,
								}),
							}),
						}),
						f:group_box({
							title = LOC("$$$/LrGeniusAI/Onboarding/ChatGPTTitle=OpenAI ChatGPT"),
							fill_horizontal = 1,
							f:row({
								f:static_text({
									title = LOC("$$$/LrGeniusAI/Onboarding/ApiKeyLabel=API Key:"),
									width = share("label"),
								}),
								f:edit_field({ value = bind("chatgptApiKey"), width_in_chars = 40 }),
								f:push_button({
									title = "?",
									action = function()
										LrHttp.openUrlInBrowser("https://platform.openai.com/api-keys")
									end,
								}),
							}),
						}),
						f:group_box({
							title = LOC("$$$/LrGeniusAI/Onboarding/LocalTitle=Local AI (Ollama / LM Studio)"),
							fill_horizontal = 1,
							f:row({
								f:push_button({
									title = LOC("$$$/LrGeniusAI/Onboarding/LocalTitle=Local AI (Ollama / LM Studio)"),
									action = function()
										LrHttp.openUrlInBrowser("https://lrgenius.com/help/ollama-setup/")
									end,
								}),
							}),
						}),
					}),
				}),
			})

			local result = LrDialogs.presentModalDialog({
				title = LOC("$$$/LrGeniusAI/Onboarding/WizardTitle=LrGeniusAI Setup"),
				contents = dialogContents,
				actionVerb = LOC("$$$/LrGeniusAI/common/OK=OK"),
				cancelVerb = LOC("$$$/LrGeniusAI/common/Cancel=Cancel"),
				otherVerb = LOC("$$$/LrGeniusAI/Onboarding/Skip=Skip Setup"),
				resizable = false,
			})

			-- Stops the polling task; without this it keeps hitting the backend
			-- for the rest of the Lightroom session.
			propertyTable.keepChecksRunning = false

			if result == "ok" or result == "other" then
				prefs.onboardingCompleted = true
				if result == "ok" then
					-- Save settings
					prefs.geminiApiKey = propertyTable.geminiApiKey
					prefs.chatgptApiKey = propertyTable.chatgptApiKey
					prefs.useClip = propertyTable.useClip
					log:info("Onboarding wizard completed with OK.")
				else
					log:info("Onboarding wizard skipped.")
				end
			end
		end)
	end)
end

return OnboardingWizard
