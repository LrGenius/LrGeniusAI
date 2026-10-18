PluginInfoDialogSections = {}

-- Which run of startDialog the polling loops below belong to. `keepChecksRunning`
-- alone was not enough to stop them: it is cleared in endDialog, so a dialog that
-- went away without endDialog completing — an error while saving preferences, a
-- teardown that never called it — left a loop hitting the backend every ten
-- seconds for the rest of the Lightroom session. Each run takes the next
-- generation and the loops stop as soon as they are no longer the current one,
-- so reopening the dialog retires whatever the last one leaked.
local checksGeneration = 0

function PluginInfoDialogSections.startDialog(propertyTable)
	propertyTable.useClip = prefs.useClip

	-- One poller for every model, not one per model: /v1/models/assets answers for
	-- all of them in a single request, and two loops on separate 5s timers used
	-- to make the two indicators update at visibly different moments.
	--
	-- Note none of this is gated on `useClip`. That preference turns the search
	-- *feature* on; it has never had anything to do with whether the files are
	-- on disk, and gating the poll on it meant the indicator stayed stuck at
	-- "not ready" for anyone who had unticked it.
	propertyTable.clipReady = false
	propertyTable.bioclipReady = false
	propertyTable.faceReady = false
	propertyTable.assetsReady = false
	propertyTable.keepChecksRunning = true

	checksGeneration = checksGeneration + 1
	local myGeneration = checksGeneration
	local function checksStillWanted()
		return propertyTable.keepChecksRunning == true and checksGeneration == myGeneration
	end

	local function refreshAssetStatus()
		local assets = SearchIndexAPI.getAssetStatus()
		if not assets or type(assets.families) ~= "table" then
			return
		end
		propertyTable.assetsReady = assets.ready == true
		for _, family in ipairs(assets.families) do
			if family.id == "clip" then
				propertyTable.clipReady = family.ready == true
			elseif family.id == "bioclip" then
				propertyTable.bioclipReady = family.ready == true
			elseif family.id == "face" then
				-- Listed since the face models became downloadable like the
				-- others. Before that they had to be fetched by hand and were
				-- excluded from `ready`, so there was nothing an indicator
				-- could usefully tell anyone.
				propertyTable.faceReady = family.ready == true
			end
		end
	end

	LrTasks.startAsyncTask(function()
		refreshAssetStatus()
		while checksStillWanted() do
			LrTasks.sleep(5)
			refreshAssetStatus()
		end
	end)
	propertyTable.logging = prefs.logging
	-- All four keys, the Other AI server's included. Kept for endDialog, which
	-- saves only the ones the user changed.
	propertyTable.loadedApiKeys = Secrets.loadInto(propertyTable)

	propertyTable.exportSize = prefs.exportSize
	propertyTable.exportQuality = prefs.exportQuality
	propertyTable.indexSubmitOriginals = (prefs.indexSubmitOriginals == true)

	propertyTable.promptTitles = Util.promptMenuItems(prefs.prompts, Defaults.defaultPromptName)

	-- A copy, assigned back in endDialog. LrPrefs saves a key when it is
	-- assigned, not when the table under it changes.
	propertyTable.prompts = Util.deepcopy(prefs.prompts)
	-- See Util.resolvePromptName: a name the prompt table no longer holds
	-- leaves the picker with no matching item, which is where both the lost
	-- edit and the crash in the observer below came from.
	propertyTable.prompt = Util.resolvePromptName(prefs.prompts, prefs.prompt, Defaults.defaultPromptName)

	propertyTable.selectedPrompt = propertyTable.prompt and prefs.prompts[propertyTable.prompt] or ""

	propertyTable:addObserver("prompt", function(properties, key, newValue)
		properties.selectedPrompt = (newValue ~= nil and properties.prompts[newValue]) or ""
	end)

	propertyTable:addObserver("selectedPrompt", function(properties, key, newValue)
		Util.storePromptText(properties.prompts, properties.prompt, newValue)
	end)

	propertyTable.periodicalUpdateCheck = prefs.periodicalUpdateCheck
	propertyTable.dbStoragePath = prefs.dbStoragePath or ""
	propertyTable.backendServerUrl = prefs.backendServerUrl or Defaults.defaultBackendServerUrl

	-- The Other AI server. Its status line is checked when the dialog opens
	-- and whenever the address or key changes — an edit field commits when
	-- the user leaves it, not on every keystroke. The 10 s health loop below
	-- never asks the server: a remote one must not be polled for that.
	propertyTable.aiServerUrl = prefs.aiServerUrl or ""
	local statusColors = {
		hint = { 0.5, 0.5, 0.5 },
		ok = { 0, 0.6, 0 },
		error = { 0.8, 0, 0 },
	}
	local serverCheck = 0
	local function refreshServerStatus()
		serverCheck = serverCheck + 1
		local thisCheck = serverCheck
		local url = propertyTable.aiServerUrl or ""
		local key = propertyTable.aiServerApiKey or ""
		if Util.nilOrEmpty(url) then
			local text, state = AiProviders.describeServerStatus(url)
			propertyTable.aiServerStatus = text
			propertyTable.aiServerStatusColor = statusColors[state]
			return
		end
		propertyTable.aiServerStatus = "Checking the server..."
		propertyTable.aiServerStatusColor = statusColors.hint
		LrTasks.startAsyncTask(function()
			local resp, err = SearchIndexAPI.getModels({ serverUrl = url, serverApiKey = key })
			if thisCheck ~= serverCheck then
				-- The address changed again while this check was running.
				return
			end
			local text, state = AiProviders.describeServerStatus(url, resp, err)
			propertyTable.aiServerStatus = text
			propertyTable.aiServerStatusColor = statusColors[state]
		end)
	end
	propertyTable:addObserver("aiServerUrl", refreshServerStatus)
	propertyTable:addObserver("aiServerApiKey", refreshServerStatus)
	refreshServerStatus()

	-- Local (in-process) model settings.
	propertyTable.llmContextSize = prefs.llmContextSize or Defaults.defaultLlmContextSize
	propertyTable.llmParallel = prefs.llmParallel or Defaults.defaultLlmParallel
	propertyTable.llmGpuLayers = prefs.llmGpuLayers or Defaults.defaultLlmGpuLayers
	LocalModelCatalog.initFields(propertyTable)
	LocalModelCatalog.refresh(propertyTable)

	-- Training/Style Profile stats (loaded asynchronously).
	propertyTable.trainingCount = 0
	propertyTable.styleStats = nil
	propertyTable.styleReadiness = "cold_start"
	propertyTable.styleReadyText =
		LOC("$$$/LrGeniusAI/Training/Status/ColdStart=Not started — save some edited photos to begin")
	propertyTable.styleReadyColor = { 0.7, 0.7, 0.7 }

	local function updateStats()
		LrTasks.startAsyncTask(function()
			local stats = SearchIndexAPI.getTrainingStats()
			if stats then
				propertyTable.styleStats = stats
				propertyTable.trainingCount = stats.count or 0

				local readiness = stats.readiness or "cold_start"
				propertyTable.styleReadiness = readiness

				if readiness == "active" then
					propertyTable.styleReadyText =
						LOC("$$$/LrGeniusAI/Training/Status/Active=Active — matching your style precisely")
					propertyTable.styleReadyColor = { 0.2, 0.8, 0.2 }
				elseif readiness == "limited" then
					propertyTable.styleReadyText =
						LOC("$$$/LrGeniusAI/Training/Status/Limited=Learning — getting better with more examples")
					propertyTable.styleReadyColor = { 0.8, 0.8, 0.2 }
				elseif readiness == "warming_up" then
					propertyTable.styleReadyText = LOC(
						"$$$/LrGeniusAI/Training/Status/WarmingUp=Building up — ^1 of 10 examples saved",
						tostring(stats.count)
					)
					propertyTable.styleReadyColor = { 0.8, 0.4, 0.1 }
				else
					propertyTable.styleReadyText =
						LOC("$$$/LrGeniusAI/Training/Status/ColdStart=Not started — save some edited photos to begin")
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
				table.insert(issues, LOC("$$$/LrGeniusAI/Health/BackendFailed=Backend server is not reachable."))
				color = { 0.8, 0, 0 }
			end
			if not health.clip and prefs.useClip then
				if status ~= "critical" then
					status = "warning"
					color = { 0.8, 0.8, 0 }
				end
				table.insert(issues, LOC("$$$/LrGeniusAI/Health/ClipMissing=AI search model is missing."))
			end
			if not SearchIndexAPI.hasAnyLlmProvider(health) then
				if status ~= "critical" then
					status = "warning"
					color = { 0.8, 0.8, 0 }
				end
				table.insert(
					issues,
					LOC("$$$/LrGeniusAI/Health/ApiKeysMissing=No AI providers configured for AI generation.")
				)
			end
			-- Set once, by the move of an old Ollama / LM Studio address to the
			-- Other AI server (AiProviders.migrateLegacyPrefs); cleared when
			-- this dialog closes.
			if not Util.nilOrEmpty(prefs.pendingProviderNotice) then
				if status == "healthy" then
					status = "warning"
					color = { 0.8, 0.8, 0 }
				end
				table.insert(issues, prefs.pendingProviderNotice)
			end
			-- A key that could not be moved to, or read from, the password
			-- storage. Repeats each session for as long as the cause lasts.
			for _, problem in ipairs(Secrets.problems()) do
				if status == "healthy" then
					status = "warning"
					color = { 0.8, 0.8, 0 }
				end
				table.insert(issues, problem)
			end

			propertyTable.healthStatus = status
			propertyTable.healthIssues = table.concat(issues, "; ")
			propertyTable.healthColor = color
		end)
	end

	updateHealth()
	LrTasks.startAsyncTask(function()
		while checksStillWanted() do
			LrTasks.sleep(10)
			updateHealth()
		end
	end)

	-- Update Check initialization
	propertyTable.updateStatus = ""
	propertyTable.updateStatusColor = { 0.7, 0.7, 0.7 }
	propertyTable.updateButtonTitle = LOC("$$$/lrc-ai-assistant/PluginInfoDialogSections/UpdateCheck=Check for updates")
	propertyTable.updateAvailable = false
	propertyTable.latestReleaseInfo = nil

	local function checkUpdates()
		LrTasks.startAsyncTask(function()
			local info = UpdateCheck.getLatestReleaseInfo()
			if not info then
				-- A failed reach is not "up to date". getLatestReleaseInfo
				-- returns nil for any network or parse failure and only logs, so
				-- claiming the plugin is current here would report a check that
				-- never actually happened.
				propertyTable.updateStatus = "Could not check for updates"
				propertyTable.updateStatusColor = { 0.7, 0.4, 0.1 }
				propertyTable.updateButtonTitle =
					LOC("$$$/lrc-ai-assistant/PluginInfoDialogSections/UpdateCheck=Check for updates")
				propertyTable.updateAvailable = false
				propertyTable.latestReleaseInfo = nil
			elseif info.is_newer then
				propertyTable.latestReleaseInfo = info
				propertyTable.updateAvailable = true
				propertyTable.updateStatus =
					LOC("$$$/LrGeniusAI/PluginInfo/UpdateAvailable=Update Available: ^1", info.tag_name)
				propertyTable.updateStatusColor = { 0.1, 0.5, 0.8 }
				if info.is_code_only then
					propertyTable.updateButtonTitle = LOC("$$$/LrGeniusAI/UpdateCheck/UpdateNow=Update Now")
				else
					propertyTable.updateButtonTitle = LOC("$$$/LrGeniusAI/PluginInfo/DownloadUpdate=Download Update")
				end
			else
				propertyTable.updateStatus = LOC("$$$/LrGeniusAI/PluginInfo/UpToDate=Plugin is up to date")
				propertyTable.updateStatusColor = { 0.5, 0.5, 0.5 }
				propertyTable.updateButtonTitle =
					LOC("$$$/lrc-ai-assistant/PluginInfoDialogSections/UpdateCheck=Check for updates")
				propertyTable.updateAvailable = false
			end
		end)
	end

	checkUpdates()
	propertyTable.manualCheckUpdates = checkUpdates

	-- AI agent bridge. Saved the moment they change, not in endDialog: the
	-- bridge may be running while this dialog is open, and it reads them
	-- live (and re-registers, so the backend learns them too).
	propertyTable.agentBridgeEnabled = prefs.agentBridgeEnabled ~= false
	propertyTable.agentBridgeAllowChanges = prefs.agentBridgeAllowChanges ~= false
	propertyTable.agentBridgeDevTools = prefs.agentBridgeDevTools == true
	propertyTable:addObserver("agentBridgeEnabled", function(_, _, value)
		prefs.agentBridgeEnabled = value == true
		if value then
			AgentBridge.start()
		else
			AgentBridge.stop()
		end
	end)
	propertyTable:addObserver("agentBridgeAllowChanges", function(_, _, value)
		prefs.agentBridgeAllowChanges = value == true
		AgentBridge.refreshRegistration()
	end)
	-- Developer commands let an agent run code in Lightroom, so turning them
	-- on asks first; the pref changes only once the user agreed.
	propertyTable:addObserver("agentBridgeDevTools", function(_, _, value)
		if value ~= true then
			prefs.agentBridgeDevTools = false
			AgentBridge.refreshRegistration()
			return
		end
		if prefs.agentBridgeDevTools == true then
			return
		end
		LrTasks.startAsyncTask(function()
			local choice = LrDialogs.confirm(
				"Allow developer commands?",
				"With developer commands on, a connected AI agent can run any Lua code inside Lightroom "
					.. "and reload the plugin's code. That can change or damage your catalog in ways Undo "
					.. "cannot reverse.\n\nThey exist for developing LrGeniusAI. Turn them on only with a "
					.. "test catalog.",
				"Allow developer commands",
				"Cancel"
			)
			if choice == "ok" then
				prefs.agentBridgeDevTools = true
				AgentBridge.refreshRegistration()
			else
				propertyTable.agentBridgeDevTools = false
			end
		end)
	end)
	local function refreshBridgeStatus()
		local status = AgentBridge.status()
		local token = Secrets.getMcpToken()
		propertyTable.mcpTokenShown = token
		propertyTable.mcpTokenReady = token ~= ""
		if status.stoppedByUser then
			propertyTable.agentBridgeStatus = "Stopped from the activity bar. Untick and tick Enable to start it again."
		elseif not status.running then
			propertyTable.agentBridgeStatus = "Off."
		elseif not status.registered then
			propertyTable.agentBridgeStatus = status.lastError and "Waiting for the LrGeniusAI backend..."
				or "Starting..."
		else
			propertyTable.agentBridgeStatus = string.format(
				"Ready for AI agents - %d request%s handled%s.",
				status.commandsDone,
				status.commandsDone == 1 and "" or "s",
				status.socketConnected and "" or " (polling)"
			)
		end
	end
	refreshBridgeStatus()
	LrTasks.startAsyncTask(function()
		while checksStillWanted() do
			refreshBridgeStatus()
			LrTasks.sleep(2)
		end
	end)
end

function PluginInfoDialogSections.sectionsForBottomOfDialog(f, propertyTable)
	local bind = LrView.bind
	local share = LrView.share

	local mcpUrl = SearchIndexAPI.mcpUrl()

	--- JSON string escaping for the setups below (a Windows path has backslashes).
	local function jsonString(text)
		return '"' .. tostring(text):gsub("\\", "\\\\"):gsub('"', '\\"') .. '"'
	end

	--- The setups, built when a button is pressed so they carry the token
	--- as it is then. Copied to the clipboard, never shown: the token is a
	--- secret, and a screenshot of this dialog must not leak it.
	local setups = {
		claudeCode = function(token)
			return "claude mcp add --transport http lrgeniusai "
				.. mcpUrl
				.. ' --header "Authorization: Bearer '
				.. token
				.. '"'
		end,
		claudeDesktop = function(token)
			local command = WIN_ENV and "C:\\Program Files\\LrGeniusAI\\backend\\lrgenius-server.exe"
				or "/Applications/LrGeniusAI/Server/lrgenius-server"
			return '{\n  "mcpServers": {\n    "lrgeniusai": {\n      "command": '
				.. jsonString(command)
				.. ',\n      "args": ["mcp"],\n      "env": { "LRGENIUS_MCP_TOKEN": '
				.. jsonString(token)
				.. " }\n    }\n  }\n}\n"
		end,
		otherApps = function(token)
			return '{\n  "mcpServers": {\n    "lrgeniusai": {\n      "url": '
				.. jsonString(mcpUrl)
				.. ',\n      "headers": { "Authorization": '
				.. jsonString("Bearer " .. token)
				.. " }\n    }\n  }\n}\n"
		end,
		token = function(token)
			return token
		end,
	}

	--- Shown once (per version of the warning) before the first agent setup
	--- leaves the dialog: connecting an agent is the step that hands it the
	--- catalog, so that is where the warning belongs.
	local function acceptAgentDisclaimer()
		if prefs.agentDisclaimerAccepted == Defaults.agentDisclaimerVersion then
			return true
		end
		local choice = LrDialogs.confirm(
			"Before you connect an AI agent",
			"An AI agent works in your catalog on its own and can make mistakes, including many changes "
				.. "at once. Text in your photos' metadata can mislead it.\n\n"
				.. "What the agent looks at - photo previews, metadata, file paths, names of recognised "
				.. "people - goes to your AI app and, unless it runs a local model, to the company behind "
				.. "it.\n\n"
				.. "The token in this setup is a key to your Lightroom: keep it secret.\n\n"
				.. "Back up your catalog and check what the agent changes. LrGeniusAI comes without any "
				.. "warranty.",
			"I understand",
			"Cancel",
			"Read the disclaimer"
		)
		if choice == "other" then
			LrHttp.openUrlInBrowser(Defaults.disclaimerUrl)
			return false
		end
		if choice ~= "ok" then
			return false
		end
		prefs.agentDisclaimerAccepted = Defaults.agentDisclaimerVersion
		return true
	end

	local function copySetup(which, label)
		LrTasks.startAsyncTask(function()
			if not acceptAgentDisclaimer() then
				return
			end
			local token = Secrets.getMcpToken()
			if token == "" then
				LrDialogs.message(
					"No agent token yet",
					"It is made when the agent bridge first connects to the backend. Enable the bridge, "
						.. "wait a moment and try again.",
					"info"
				)
				return
			end
			local ok, err = Util.copyToClipboard(setups[which](token))
			if ok then
				LrDialogs.showBezel(label .. " copied to the clipboard")
			else
				ErrorHandler.handleError("Could not copy to the clipboard", tostring(err))
			end
		end)
	end

	return {
		{
			bind_to_object = propertyTable,
			title = "AI Agents (MCP)",

			f:group_box({
				title = "AI Agents (MCP)",
				width = 600,
				f:static_text({
					-- Manual line breaks: wrap = true does nothing on static_text.
					title = "Let an AI agent - Claude Code, Claude Desktop, LM Studio, Cursor - search your catalog\n"
						.. "with LrGeniusAI and work in Lightroom: find, cull, rate, keyword, collect and edit.\n"
						.. "Every change is one step in Edit > Undo. While an agent works, a progress bar shows\n"
						.. "what it does; its X stops the agent.",
				}),
				f:static_text({
					title = "Caution: an agent changes your catalog on its own and can be wrong. Text in your\n"
						.. "photos' metadata can mislead it. Previews, metadata and names it looks at go to\n"
						.. "your AI app and its provider. Back up your catalog and keep the token secret.",
					text_color = LrColor(0.75, 0.35, 0.05),
				}),
				f:push_button({
					title = "Read the disclaimer",
					action = function()
						LrHttp.openUrlInBrowser(Defaults.disclaimerUrl)
					end,
				}),
				f:checkbox({
					value = bind("agentBridgeEnabled"),
					title = "Enable the agent bridge (starts with Lightroom)",
				}),
				f:static_text({ title = bind("agentBridgeStatus"), width_in_chars = 70 }),
				f:checkbox({
					value = bind("agentBridgeAllowChanges"),
					title = "Allow agents to change photos (ratings, keywords, collections, develop settings)",
				}),
				f:checkbox({
					value = bind("agentBridgeDevTools"),
					title = "Allow developer commands (run Lua in the plugin) - for plugin development only",
					enabled = AgentBridge.isDevInstall(),
				}),
				f:spacer({ height = 6 }),
				f:row({
					f:static_text({ title = "Agent token:" }),
					-- Shown masked: AI apps must present it, and it is as
					-- secret as an API key.
					f:password_field({
						value = bind("mcpTokenShown"),
						width_in_chars = 36,
						enabled = false,
					}),
					f:push_button({
						title = "Copy token",
						enabled = bind("mcpTokenReady"),
						action = function()
							copySetup("token", "Agent token")
						end,
					}),
					f:push_button({
						title = "New token...",
						enabled = bind("mcpTokenReady"),
						action = function()
							LrTasks.startAsyncTask(function()
								local choice = LrDialogs.confirm(
									"Make a new agent token?",
									"Every AI app set up with the current token stops working until you copy the "
										.. "new setup into it.",
									"Make new token",
									"Cancel"
								)
								if choice == "ok" then
									AgentBridge.rotateMcpToken()
									LrDialogs.showBezel("New agent token requested")
								end
							end)
						end,
					}),
				}),
				f:static_text({
					title = "Set up an AI app - copies its setup, token included, to the clipboard:",
				}),
				f:row({
					f:push_button({
						title = "Claude Code",
						enabled = bind("mcpTokenReady"),
						action = function()
							copySetup("claudeCode", "Claude Code command")
						end,
					}),
					f:push_button({
						title = "Claude Desktop",
						enabled = bind("mcpTokenReady"),
						action = function()
							copySetup("claudeDesktop", "Claude Desktop config")
						end,
					}),
					f:push_button({
						title = "LM Studio, Cursor, others",
						enabled = bind("mcpTokenReady"),
						action = function()
							copySetup("otherApps", "MCP config")
						end,
					}),
				}),
				f:static_text({
					title = "Claude Code: paste in a terminal. Claude Desktop: merge into claude_desktop_config.json.\n"
						.. "Others: merge into their mcp.json. Endpoint: "
						.. mcpUrl
						.. " (Streamable HTTP, bearer token).",
				}),
			}),
		},
		{
			title = "Disclaimer",
			f:group_box({
				title = "Disclaimer",
				width = 600,
				f:static_text({
					title = "LrGeniusAI comes without any warranty (AGPL-3.0). It changes your catalog: back it up.\n"
						.. "AI results can be wrong - check them before you rely on them. With a cloud AI provider,\n"
						.. "photo previews and metadata are sent to that provider under its terms.",
				}),
				f:push_button({
					title = "Read the disclaimer",
					action = function()
						LrHttp.openUrlInBrowser(Defaults.disclaimerUrl)
					end,
				}),
			}),
		},
		{
			bind_to_object = propertyTable,
			title = LOC("$$$/LrGeniusAI/PluginInfo/Logging=Logging"),

			f:group_box({
				title = LOC("$$$/LrGeniusAI/PluginInfo/Logging=Logging"),
				width = 600,

				f:row({
					f:static_text({
						title = bind("updateStatus"),
						text_color = bind("updateStatusColor"),
						width = share("bottomButtons"),
						alignment = "center",
					}),
				}),
				f:row({
					f:push_button({
						title = LOC("$$$/lrc-ai-assistant/PluginInfoDialogSections/ShowLogfile=Show logfile"),
						action = function(button)
							LrShell.revealInShell(Util.getLogfilePath())
						end,
						width = share("bottomButtons"),
					}),
					f:push_button({
						title = LOC(
							"$$$/lrc-ai-assistant/PluginInfoDialogSections/CopyLogToDesktop=Copy logfiles to Desktop"
						),
						action = function(button)
							LrTasks.startAsyncTask(function()
								Util.copyLogfilesToDesktop()
							end)
						end,
						width = share("bottomButtons"),
					}),
					f:push_button({
						title = bind("updateButtonTitle"),
						action = function(button)
							if propertyTable.updateAvailable then
								if propertyTable.latestReleaseInfo.is_code_only then
									local tu = require("TaskUpdate")
									tu.runUpdate(propertyTable.latestReleaseInfo)
								else
									LrHttp.openUrlInBrowser(
										propertyTable.latestReleaseInfo.release_url or UpdateCheck.latestReleaseUrl
									)
								end
							else
								propertyTable.manualCheckUpdates()
							end
						end,
						width = share("bottomButtons"),
					}),
				}),
				f:row({
					f:checkbox({
						value = bind("periodicalUpdateCheck"),
						title = LOC(
							"$$$/lrc-ai-assistant/PluginInfoDialogSections/periodUpdateCheck=Periodically check for Updates"
						),
					}),
				}),
			}),
		},
		{
			title = LOC("$$$/LrGeniusAI/PluginInfo/Credits=CREDITS"),
			f:group_box({
				width = 600,
				title = LOC("$$$/LrGeniusAI/PluginInfo/Credits=CREDITS"),
				f:row({
					-- The list is far longer than the box, so it scrolls. The
					-- static_text inside still needs its own full height: it
					-- clips to height_in_lines rather than growing to fit, so
					-- without it only the first entry shows.
					f:scrolled_view({
						horizontal_scroller = false,
						vertical_scroller = true,
						fill_horizontal = 1,
						height_in_lines = 20,
						width = 580,
						f:static_text({
							title = Defaults.copyrightString,
							width_in_chars = 100,
							height_in_lines = Defaults.copyrightLineCount,
						}),
					}),
				}),
			}),
		},
	}
end

function PluginInfoDialogSections.sectionsForTopOfDialog(f, propertyTable)
	local bind = LrView.bind
	local share = LrView.share

	local groupBoxWidth = 600

	propertyTable.models = {}

	propertyTable.promptTitleMenu = f:popup_menu({
		items = bind("promptTitles"),
		value = bind("prompt"),
	})

	-- Exactly one local engine is offered, and which one is a property of the
	-- platform rather than a choice the user makes.
	--
	-- macOS ships MLX only: the macOS release build does not compile the
	-- `llamacpp` cargo feature at all (see `cargo_features` in
	-- .github/workflows/release.yml), so a llama.cpp box there could never do
	-- anything but report "this backend build has no local-model support".
	-- Windows is the mirror image — MLX is Apple silicon only, and its half of
	-- /v1/llm/catalog always comes back unsupported.
	--
	-- Building only the box that applies, rather than hiding the other one,
	-- keeps the dialog free of a dead group and means neither set of bindings
	-- is created on a platform that cannot serve them.
	local localEngineBox
	if MAC_ENV then
		localEngineBox = f:group_box({
			width = groupBoxWidth,
			title = LOC("$$$/LrGeniusAI/MlxModel/Title=Local AI Model — MLX (Apple silicon)"),
			f:row({
				fill_horizontal = 1,
				f:static_text({
					title = LOC("$$$/LrGeniusAI/MlxModel/Status=Status"),
					width = share("setupLabelWidth"),
				}),
				f:static_text({
					title = bind("mlxStatusText"),
					fill_horizontal = 1,
				}),
			}),
			f:row({
				fill_horizontal = 1,
				f:static_text({
					title = LOC("$$$/LrGeniusAI/MlxModel/Installed=Installed"),
					width = share("setupLabelWidth"),
				}),
				f:static_text({
					title = bind("mlxInstalledText"),
					fill_horizontal = 1,
				}),
			}),
			f:row({
				fill_horizontal = 1,
				f:popup_menu({
					items = bind("mlxDownloadChoices"),
					value = bind("mlxDownloadChoice"),
					enabled = bind("mlxSupported"),
					width_in_chars = 34,
				}),
				f:push_button({
					title = LOC("$$$/LrGeniusAI/MlxModel/Download=Download"),
					enabled = bind({
						key = "mlxDownloadChoice",
						transform = function(v)
							return v ~= nil
						end,
					}),
					action = function(button)
						LocalModelCatalog.startDownload(propertyTable, "mlx")
					end,
					width = share("setupButtonWidth"),
				}),
			}),
		})
	else
		localEngineBox = f:group_box({
			width = groupBoxWidth,
			title = LOC("$$$/LrGeniusAI/LocalModel/Title=Local AI Model — llama.cpp"),
			f:row({
				fill_horizontal = 1,
				f:static_text({
					title = LOC("$$$/LrGeniusAI/LocalModel/Status=Status"),
					width = share("setupLabelWidth"),
				}),
				f:static_text({
					title = bind("llmStatusText"),
					fill_horizontal = 1,
				}),
			}),
			f:row({
				fill_horizontal = 1,
				f:static_text({
					title = LOC("$$$/LrGeniusAI/LocalModel/Installed=Installed"),
					width = share("setupLabelWidth"),
				}),
				f:static_text({
					title = bind("llmInstalledText"),
					fill_horizontal = 1,
				}),
			}),
			f:row({
				fill_horizontal = 1,
				f:popup_menu({
					items = bind("llmDownloadChoices"),
					value = bind("llmDownloadChoice"),
					enabled = bind("llmSupported"),
					width_in_chars = 34,
				}),
				f:push_button({
					title = LOC("$$$/LrGeniusAI/LocalModel/Download=Download"),
					enabled = bind({
						key = "llmDownloadChoice",
						transform = function(v)
							return v ~= nil
						end,
					}),
					action = function(button)
						LocalModelCatalog.startDownload(propertyTable, "llm")
					end,
					width = share("setupButtonWidth"),
				}),
			}),
			f:group_box({
				fill_horizontal = 1,
				title = LOC("$$$/LrGeniusAI/LocalModel/AdvancedTitle=Advanced"),
				-- llama.cpp splits the KV cache between the sequences it
				-- decodes at once, so these two trade against each other: each
				-- photo gets context size / photos in parallel, and Max Tokens
				-- has to fit in that share alongside the prompt. The server
				-- reduces the parallel count rather than overrunning it.
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC("$$$/LrGeniusAI/LocalModel/ContextSize=Context size (tokens)"),
						width = share("setupLabelWidth"),
					}),
					f:edit_field({
						value = bind("llmContextSize"),
						enabled = bind("llmSupported"),
						precision = 0,
						min = 1024,
						max = 131072,
						width_in_chars = 8,
					}),
					f:static_text({
						title = LOC("$$$/LrGeniusAI/LocalModel/Parallel=Photos in parallel"),
					}),
					f:edit_field({
						value = bind("llmParallel"),
						enabled = bind("llmSupported"),
						precision = 0,
						min = 1,
						max = 16,
						width_in_chars = 4,
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC("$$$/LrGeniusAI/LocalModel/GpuLayers=Layers on the GPU"),
						width = share("setupLabelWidth"),
					}),
					f:edit_field({
						value = bind("llmGpuLayers"),
						enabled = bind("llmSupported"),
						precision = 0,
						min = 0,
						max = 999,
						width_in_chars = 8,
					}),
					f:static_text({
						title = LOC(
							"$$$/LrGeniusAI/LocalModel/GpuLayersHint=999 = all; anything that does not fit stays on the CPU"
						),
					}),
				}),
			}),
		})
	end

	-- One collapsible block per section, in the order someone actually meets
	-- them: is it working at all, which model does the analysis, then search,
	-- then the knobs you only reach for once you want different behaviour, and
	-- the backend plumbing last.
	return {
		{
			bind_to_object = propertyTable,
			title = LOC("$$$/LrGeniusAI/PluginInfo/SectionStatus=Status"),
			f:group_box({
				width = groupBoxWidth,
				title = LOC("$$$/LrGeniusAI/Health/SummaryTitle=System Health"),

				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC("$$$/LrGeniusAI/Health/SummaryTitle=System Health"),
						font = "<system/bold>",
						alignment = "right",
					}),
					Util.statusIcon(f, "healthStatus", function(v)
						return v == "healthy"
					end),
					f:static_text({
						title = bind({
							key = "healthStatus",
							transform = function(v)
								if v == "healthy" then
									return LOC("$$$/LrGeniusAI/Health/StatusHealthy=Everything looks good!")
								end
								if v == "warning" then
									return LOC(
										"$$$/LrGeniusAI/Health/StatusWarning=Some features might not work correctly."
									)
								end
								return LOC(
									"$$$/LrGeniusAI/Health/StatusCritical=Critical issues detected. Plugin cannot function."
								)
							end,
						}),
						text_color = bind("healthColor"),
					}),
				}),
				f:row({
					f:push_button({
						title = LOC("$$$/LrGeniusAI/Health/RunWizard=Run Setup Wizard"),
						action = function()
							OnboardingWizard.show(true)
						end,
					}),
					-- The backend holds its models for 30 minutes after the
					-- last request, and the plug-in could only ask it to let go
					-- at shutdown. This reaches the same route the shutdown
					-- function calls, so the memory can be handed back mid-
					-- session instead of at the end of one.
					f:push_button({
						title = "Unload AI Models",
						action = function()
							LrTasks.startAsyncTask(function()
								Util.unloadModelsDialog()
							end)
						end,
					}),
				}),
			}),
			f:row({
				visible = bind({
					key = "healthIssues",
					transform = function(v)
						return v ~= ""
					end,
				}),
				f:spacer({ width = share("labelWidth") }),
				f:static_text({
					fill_horizontal = 1,
					title = bind("healthIssues"),
					text_color = bind("healthColor"),
					size = "small",
					wrap = true,
				}),
			}),
			f:row({
				f:push_button({
					title = LOC("$$$/lrc-ai-assistant/PluginInfoDialogSections/Docs=Read documentation online"),
					action = function(button)
						LrHttp.openUrlInBrowser("https://github.com/LrGenius/LrGeniusAI/wiki")
					end,
				}),
			}),
		},
		{
			bind_to_object = propertyTable,
			title = LOC("$$$/LrGeniusAI/PluginInfo/SectionLocalAi=AI model (on this computer)"),
			f:static_text({
				title = MAC_ENV
						and LOC(
							"$$$/LrGeniusAI/PluginInfo/LocalAiHintMac=The analysis runs on this computer by default, so no account is\nneeded. On Apple silicon it uses MLX, Apple's own machine learning\nstack. Download one model below and the plug-in is ready."
						)
					or LOC(
						"$$$/LrGeniusAI/PluginInfo/LocalAiHintWin=The analysis runs on this computer by default, so no account is\nneeded. Download one model below and the plug-in is ready."
					),
			}),
			localEngineBox,
		},
		-- One section and one button for both models. They used to be two, which
		-- asked the reader to know that "smart photo search" and "species
		-- recognition" are different neural networks with separate downloads --
		-- a distinction that matters to this codebase and to nobody using it.
		-- The per-model indicators stay, so it is still obvious which half is
		-- missing when only one is.
		{
			bind_to_object = propertyTable,
			title = LOC("$$$/LrGeniusAI/PluginInfo/SectionModels=On-device AI models"),
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/PluginInfo/ModelsHint=Search your catalog by what is in the picture, and identify\nanimals, plants and fungi down to the species. Both run entirely\non this computer and are a one-time download."
				),
			}),
			f:checkbox({
				value = bind("useClip"),
				title = LOC("$$$/LrGeniusAI/PluginInfo/UseOpenClip=Use AI model for smart photo search"),
			}),
			f:group_box({
				width = groupBoxWidth,
				title = LOC("$$$/LrGeniusAI/PluginInfo/ModelsTitle=Models"),
				f:row({
					fill_horizontal = 1,
					Util.statusIndicator(
						f,
						"assetsReady",
						LOC("$$$/LrGeniusAI/PluginInfo/AssetsReady=All AI models are ready")
					),
					f:push_button({
						title = LOC("$$$/LrGeniusAI/PluginInfo/DownloadModels=Download AI models"),
						action = function(button)
							LrTasks.startAsyncTask(function()
								SearchIndexAPI.startAssetDownload()
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
				f:row({
					Util.statusIndicator(
						f,
						"clipReady",
						LOC("$$$/LrGeniusAI/PluginInfo/OpenClipReady=Smart photo search")
					),
				}),
				f:row({
					Util.statusIndicator(
						f,
						"bioclipReady",
						LOC("$$$/LrGeniusAI/PluginInfo/BioclipReady=Species identification")
					),
				}),
				f:row({
					Util.statusIndicator(
						f,
						"faceReady",
						LOC("$$$/LrGeniusAI/PluginInfo/FaceReady=Face detection and people")
					),
				}),
			}),
		},
		{
			bind_to_object = propertyTable,
			title = LOC("$$$/LrGeniusAI/UI/Prompts=Prompts"),
			f:group_box({
				width = groupBoxWidth,
				title = LOC("$$$/LrGeniusAI/UI/Prompts=Prompts"),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						alignment = "right",
						width = share("promptLabelWidth"),
						title = LOC("$$$/LrGeniusAI/UI/PromptTitle=Title"),
					}),
					propertyTable.promptTitleMenu,
					f:push_button({
						title = LOC("$$$/lrc-ai-assistant/PluginInfoDialogSections/add=Add"),
						action = function(button)
							PromptConfigProvider.addPrompt(propertyTable)
						end,
					}),
					f:push_button({
						title = LOC("$$$/lrc-ai-assistant/PluginInfoDialogSections/delete=Delete"),
						action = function(button)
							PromptConfigProvider.deletePrompt(propertyTable)
						end,
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						alignment = "right",
						width = share("promptLabelWidth"),
						title = LOC("$$$/LrGeniusAI/PromptConfig/PromptField=Prompt"),
					}),
					f:scrolled_view({
						horizontal_scroller = false,
						vertical_scroller = true,
						fill_horizontal = 1,
						height_in_lines = 15,
						width = 500,
						f:edit_field({
							value = bind("selectedPrompt"),
							fill_horizontal = 1,
							height_in_lines = 30,
							wraps = true,
							width = 480,
						}),
					}),
				}),
			}),
		},
		{
			bind_to_object = propertyTable,
			title = LOC("$$$/LrGeniusAI/Training/SectionTitle=My Style Profile"),
			f:group_box({
				width = groupBoxWidth,
				title = LOC("$$$/LrGeniusAI/Training/SectionTitle=My Style Profile"),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC("$$$/LrGeniusAI/Training/EngineStatus=Style Engine Status:"),
						alignment = "right",
						width = share("labelWidth"),
					}),
					f:static_text({
						title = bind("styleReadyText"),
						text_color = bind("styleReadyColor"),
						font = "<system/bold>",
						fill_horizontal = 1,
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC("$$$/LrGeniusAI/Training/SavedExamples=Saved training examples:"),
						alignment = "right",
						width = share("labelWidth"),
					}),
					f:static_text({
						title = bind({
							key = "trainingCount",
							transform = function(v)
								return tostring(v or 0)
							end,
						}),
						fill_horizontal = 1,
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC("$$$/LrGeniusAI/Training/TopScenes=Top Scene Types:"),
						alignment = "right",
						width = share("labelWidth"),
					}),
					f:static_text({
						fill_horizontal = 1,
						title = bind({
							key = "styleStats",
							transform = function(s)
								if not s or not s.scene_distribution then
									return "..."
								end
								local sorted = {}
								for k, v in pairs(s.scene_distribution) do
									table.insert(sorted, { name = k, count = v })
								end
								table.sort(sorted, function(a, b)
									return a.count > b.count
								end)
								local top = {}
								for i = 1, math.min(3, #sorted) do
									local name = sorted[i].name:gsub("^scene_", ""):gsub("_", " ")
									table.insert(top, name:sub(1, 1):upper() .. name:sub(2))
								end
								return #top > 0 and table.concat(top, ", ") or "None yet"
							end,
						}),
						font = "<system/italic>",
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC("$$$/LrGeniusAI/Training/LearnedCameras=Learned Cameras:"),
						alignment = "right",
						width = share("labelWidth"),
					}),
					f:static_text({
						fill_horizontal = 1,
						title = bind({
							key = "styleStats",
							transform = function(s)
								if not s or not s.camera_distribution then
									return "..."
								end
								local sorted = {}
								for k, v in pairs(s.camera_distribution) do
									table.insert(sorted, { name = k, count = v })
								end
								table.sort(sorted, function(a, b)
									return a.count > b.count
								end)
								local top = {}
								for i = 1, math.min(5, #sorted) do
									table.insert(top, string.format("%s (%d)", sorted[i].name, sorted[i].count))
								end
								return #top > 0 and table.concat(top, "\n") or "None yet"
							end,
						}),
						font = "<system/italic>",
						height_in_lines = 5,
						wrap = true,
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC("$$$/LrGeniusAI/Training/StyleDNA=Your Average Style Settings:"),
						alignment = "right",
						width = share("labelWidth"),
					}),
					f:static_text({
						fill_horizontal = 1,
						title = bind({
							key = "styleStats",
							transform = function(s)
								if not s or not s.exposure then
									return "..."
								end
								local e = s.exposure
								local parts = {}
								if e.mean_luminance then
									table.insert(parts, string.format("Lum: %.0f%%", e.mean_luminance * 100))
								end
								if e.mean_contrast then
									table.insert(parts, string.format("Con: %.0f%%", e.mean_contrast * 100))
								end
								if e.mean_colorfulness then
									table.insert(parts, string.format("Color: %.0f%%", e.mean_colorfulness * 100))
								end
								return #parts > 0 and table.concat(parts, " | ") or "..."
							end,
						}),
					}),
				}),
				f:row({
					f:push_button({
						title = LOC("$$$/LrGeniusAI/common/Refresh=Refresh"),
						action = function(button)
							propertyTable.refreshStyleStats()
						end,
					}),
					f:push_button({
						title = LOC("$$$/LrGeniusAI/Training/ClearAll=Clear all training examples"),
						action = function(button)
							local confirm = LrDialogs.confirm(
								LOC("$$$/LrGeniusAI/Training/ClearConfirmTitle=Clear Training Examples"),
								LOC(
									"$$$/LrGeniusAI/Training/ClearConfirmMsg=This will permanently delete all saved training examples. The Style Engine will be reset to Cold Start. Continue?"
								),
								LOC("$$$/LrGeniusAI/Training/ClearConfirmOk=Delete All"),
								LOC("$$$/LrGeniusAI/Training/ClearConfirmCancel=Cancel")
							)
							if confirm == "ok" then
								LrTasks.startAsyncTask(function()
									local ok, err = SearchIndexAPI.clearAllTrainingExamples()
									if ok then
										propertyTable.refreshStyleStats()
										LrDialogs.message(
											LOC("$$$/LrGeniusAI/Training/ClearedTitle=Training Data Cleared"),
											LOC(
												"$$$/LrGeniusAI/Training/ClearedMsg=All training examples have been removed."
											),
											"info"
										)
									else
										ErrorHandler.handleError(
											LOC("$$$/LrGeniusAI/Training/ClearFailedTitle=Clear Failed"),
											tostring(err or "Unknown error")
										)
									end
								end)
							end
						end,
					}),
				}),
			}),
		},
		{
			bind_to_object = propertyTable,
			title = "Optional AI providers",
			-- One row per provider, no group boxes. Ollama and LM Studio have no
			-- rows: they are found at their default address on this computer, and
			-- running elsewhere, they are the Other AI server.
			f:static_text({
				title = "Not needed if you use the AI model above. Ollama and LM Studio on this computer\n"
					.. "are found automatically.",
			}),
			f:row({
				fill_horizontal = 1,
				f:static_text({
					title = "Google Gemini key",
					alignment = "right",
					width = share("apiKeyLabelWidth"),
				}),
				f:password_field({
					value = bind("geminiApiKey"),
					fill_horizontal = 1,
				}),
				f:push_button({
					title = "Get key",
					action = function(button)
						LrHttp.openUrlInBrowser("https://aistudio.google.com/app/apikey")
					end,
					width = share("apiKeyButtonWidth"),
				}),
			}),
			f:row({
				fill_horizontal = 1,
				f:static_text({
					title = "OpenAI key",
					alignment = "right",
					width = share("apiKeyLabelWidth"),
				}),
				f:password_field({
					value = bind("chatgptApiKey"),
					fill_horizontal = 1,
				}),
				f:push_button({
					title = "Get key",
					action = function(button)
						LrHttp.openUrlInBrowser("https://platform.openai.com/api-keys")
					end,
					width = share("apiKeyButtonWidth"),
				}),
			}),
			-- Claude through Anthropic's own API. Not the Other AI server: Anthropic's
			-- OpenAI-compatible endpoint lists no models and ignores the answer format.
			f:row({
				fill_horizontal = 1,
				f:static_text({
					title = "Anthropic key",
					alignment = "right",
					width = share("apiKeyLabelWidth"),
				}),
				f:password_field({
					value = bind("anthropicApiKey"),
					fill_horizontal = 1,
				}),
				f:push_button({
					title = "Get key",
					action = function(button)
						LrHttp.openUrlInBrowser("https://platform.claude.com/settings/keys")
					end,
					width = share("apiKeyButtonWidth"),
				}),
			}),
			f:row({
				fill_horizontal = 1,
				f:static_text({
					title = "Other AI server",
					alignment = "right",
					width = share("apiKeyLabelWidth"),
				}),
				f:edit_field({
					value = bind("aiServerUrl"),
					placeholder_string = "e.g. https://openrouter.ai/api/v1",
					fill_horizontal = 1,
				}),
				f:push_button({
					title = "Help",
					action = function(button)
						LrHttp.openUrlInBrowser("https://github.com/LrGenius/LrGeniusAI/wiki/Help-Other-AI-Server")
					end,
					width = share("apiKeyButtonWidth"),
				}),
			}),
			f:row({
				fill_horizontal = 1,
				f:static_text({
					title = "API key",
					alignment = "right",
					width = share("apiKeyLabelWidth"),
				}),
				f:password_field({
					value = bind("aiServerApiKey"),
					placeholder_string = "only if the server needs one",
					fill_horizontal = 1,
				}),
				f:spacer({ width = share("apiKeyButtonWidth") }),
			}),
			f:row({
				fill_horizontal = 1,
				f:spacer({ width = share("apiKeyLabelWidth") }),
				f:static_text({
					title = bind("aiServerStatus"),
					text_color = bind("aiServerStatusColor"),
					size = "small",
					wrap = true,
					fill_horizontal = 1,
				}),
			}),
		},
		{
			bind_to_object = propertyTable,
			title = LOC("$$$/LrGeniusAI/PluginInfo/SectionIndexing=Indexing and export"),
			f:group_box({
				width = groupBoxWidth,
				title = LOC("$$$/lrc-ai-assistant/PluginInfoDialogSections/exportSettings=Export settings"),
				f:row({
					fill_horizontal = 1,
					f:radio_button({
						value = bind("indexSubmitOriginals"),
						checked_value = true,
						title = LOC(
							"$$$/LrGeniusAI/PluginInfo/IndexSubmitOriginals=Submit original files for fastest indexing (experimental)"
						),
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC(
							"$$$/LrGeniusAI/PluginInfo/IndexSubmitOriginalsInfo=The backend reads the original file (RAW/JPEG/HEIC) directly, skipping the export step.\nNote: uses the camera preview, so crops and develop edits are not reflected,\nlocation keywords from Lightroom's address lookup are unavailable, and results may\ndiffer slightly from photos indexed the classic way. Re-index to keep search consistent."
						),
						enabled = false,
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:radio_button({
						value = bind("indexSubmitOriginals"),
						checked_value = false,
						title = LOC("$$$/LrGeniusAI/PluginInfo/IndexModeExport=Export photos for indexing"),
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC(
							"$$$/lrc-ai-assistant/PluginInfoDialogSections/exportSize=Export size in pixel (long edge)"
						),
						alignment = "right",
						width = share("labelWidth"),
					}),
					f:popup_menu({
						value = bind("exportSize"),
						items = Defaults.exportSizes,
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC(
							"$$$/lrc-ai-assistant/PluginInfoDialogSections/exportQuality=Export JPEG quality in percent"
						),
						alignment = "right",
						width = share("labelWidth"),
					}),
					f:slider({
						value = bind("exportQuality"),
						min = 1,
						max = 100,
						integral = true,
						immediate = true,
						fill_horizontal = 1,
					}),
					f:static_text({
						title = bind("exportQuality"),
						width_in_chars = 5,
					}),
				}),
			}),
		},
		{
			bind_to_object = propertyTable,
			title = LOC("$$$/lrc-ai-assistant/PluginInfoDialogSections/BackendServer=Backend Server"),
			f:group_box({
				width = groupBoxWidth,
				title = LOC("$$$/lrc-ai-assistant/PluginInfoDialogSections/BackendServer=Backend Server"),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC(
							"$$$/lrc-ai-assistant/PluginInfoDialogSections/BackendServerUrl=Backend server URL (IP/hostname)"
						),
						alignment = "right",
						width = share("labelWidth"),
					}),
					f:edit_field({
						value = bind("backendServerUrl"),
						fill_horizontal = 1,
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:static_text({
						title = LOC("$$$/LrGeniusAI/PluginInfo/DbStoragePath=Database storage folder"),
						alignment = "right",
						width = share("labelWidth"),
					}),
					f:edit_field({
						value = bind("dbStoragePath"),
						fill_horizontal = 1,
					}),
					f:push_button({
						title = LOC("$$$/LrGeniusAI/PluginInfo/Browse=Browse..."),
						action = function(button)
							local result = LrDialogs.runOpenPanel({
								title = LOC(
									"$$$/LrGeniusAI/PluginInfo/SelectDbFolderTitle=Select Database Storage Folder"
								),
								prompt = LOC("$$$/LrGeniusAI/PluginInfo/SelectFolder=Select"),
								canChooseFiles = false,
								canChooseDirectories = true,
								allowsMultipleSelection = false,
							})
							if result and result[1] then
								local newPath = result[1]
								local oldPath = (propertyTable.dbStoragePath or ""):gsub("^%s*(.-)%s*$", "%1")
								if oldPath ~= "" and oldPath ~= newPath then
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/PluginInfo/DbPathChangedTitle=Database Path Changed"),
										LOC(
											"$$$/LrGeniusAI/PluginInfo/DbPathChangedMessage=The AI search index will be created fresh at the new location. Your existing index data will remain at the old path and will not be moved. Re-run indexing after saving to rebuild the index."
										),
										"warning"
									)
								end
								propertyTable.dbStoragePath = newPath
							end
						end,
					}),
				}),
				f:row({
					fill_horizontal = 1,
					f:spacer({ width = share("labelWidth") }),
					f:static_text({
						title = LOC(
							"$$$/LrGeniusAI/PluginInfo/DbStoragePathDesc=Leave empty to store next to the catalog (default)."
						),
						size = "small",
						fill_horizontal = 1,
						wrap = true,
					}),
				}),
				f:row({
					f:push_button({
						title = LOC("$$$/LrGeniusAI/PluginInfo/ClaimPhotos=Claim photos for this catalog"),
						width = share("backendButtonWidth"),
						action = function(button)
							LrTasks.startAsyncTask(function()
								local progressScope = LrProgressScope({
									title = LOC(
										"$$$/LrGeniusAI/SearchIndexAPI/claimingPhotos=Claiming photos for this catalog..."
									),
									functionContext = nil,
								})
								local ok, err, result = SearchIndexAPI.claimPhotosForCatalog(progressScope)
								progressScope:done()
								if ok then
									local msg = LOC("$$$/LrGeniusAI/common/Done=Done.")
									if result then
										msg = LOC(
											"$$$/LrGeniusAI/PluginInfo/ClaimedPrefix=Claimed ^1 photo(s).",
											tostring(result.claimed)
										)
										if result.errors and result.errors > 0 then
											msg = msg
												.. "\n"
												.. LOC(
													"$$$/LrGeniusAI/PluginInfo/ClaimedErrors=^1 photo(s) could not be claimed; run Analyze & Index on them again.",
													tostring(result.errors)
												)
										end
									end
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/PluginInfo/ClaimPhotosTitle=Claim photos"),
										msg,
										"info"
									)
								else
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/PluginInfo/ClaimPhotosFailed=Claim photos failed"),
										tostring(err or LOC("$$$/LrGeniusAI/common/UnknownError=Unknown error")),
										"critical"
									)
								end
							end)
						end,
					}),
					f:push_button({
						title = LOC("$$$/LrGeniusAI/PluginInfo/ShowDbStats=Show DB stats"),
						width = share("backendButtonWidth"),
						action = function(button)
							LrTasks.startAsyncTask(function()
								local stats, err = SearchIndexAPI.getStats()
								if stats then
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/PluginInfo/DbStatsTitle=Database statistics"),
										SearchIndexAPI.formatStats(stats),
										"info"
									)
								else
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/PluginInfo/DbStatsFailed=Database statistics failed"),
										tostring(err or LOC("$$$/LrGeniusAI/common/UnknownError=Unknown error")),
										"critical"
									)
								end
							end)
						end,
					}),
					f:push_button({
						title = LOC("$$$/LrGeniusAI/PluginInfo/DownloadDbBackup=Download DB backup"),
						width = share("backendButtonWidth"),
						action = function(button)
							LrTasks.startAsyncTask(function()
								-- On failure the message is the *second* return value; `ok` is
								-- only ever `true`/`false`/`nil`, so never report it to the user.
								local ok, pathOrErr = SearchIndexAPI.downloadDatabaseBackup()
								if ok then
									LrShell.revealInShell(pathOrErr)
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/PluginInfo/DbBackupDownloaded=Database backup downloaded."),
										LrPathUtils.leafName(pathOrErr),
										"info"
									)
								elseif pathOrErr ~= "canceled" then
									-- A user-canceled save panel is not an error; anything else is.
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/PluginInfo/DbBackupFailed=Database backup failed"),
										tostring(pathOrErr or LOC("$$$/LrGeniusAI/common/UnknownError=Unknown error")),
										"critical"
									)
								end
							end)
						end,
					}),
				}),
				f:row({
					f:push_button({
						title = LOC("$$$/LrGeniusAI/PluginInfo/CheckVersions=Check Plugin/Backend versions"),
						width = share("backendButtonWidth"),
						action = function(button)
							LrTasks.startAsyncTask(function()
								local result, err = SearchIndexAPI.checkVersionCompatibility()
								if result then
									local backendTag = tostring(
										result.backend_release_tag
											or (
												"v"
												.. tostring(
													result.backend_version
														or LOC("$$$/LrGeniusAI/common/Unknown=unknown")
												)
											)
									)
									local pluginTag = tostring(
										result.plugin_release_tag
											or (
												"v"
												.. tostring(
													result.plugin_version
														or LOC("$$$/LrGeniusAI/common/Unknown=unknown")
												)
											)
									)
									local buildInfo = LOC("$$$/LrGeniusAI/PluginInfo/PluginBuild=Plugin build: ")
										.. tostring(result.plugin_build or "n/a")
										.. "\n"
										.. LOC("$$$/LrGeniusAI/PluginInfo/BackendBuild=Backend build: ")
										.. tostring(result.backend_build or "n/a")

									if result.compatible then
										LrDialogs.message(
											LOC("$$$/LrGeniusAI/PluginInfo/VersionCheckPassed=Version check passed"),
											LOC(
												"$$$/LrGeniusAI/PluginInfo/VersionMatch=Plugin and backend versions match.\n"
											)
												.. LOC("$$$/LrGeniusAI/PluginInfo/PluginPrefix=Plugin: ")
												.. pluginTag
												.. "\n"
												.. LOC("$$$/LrGeniusAI/PluginInfo/BackendPrefix=Backend: ")
												.. backendTag
												.. "\n\n"
												.. buildInfo,
											"info"
										)
									else
										LrDialogs.message(
											LOC("$$$/LrGeniusAI/PluginInfo/VersionMismatch=Version mismatch"),
											LOC(
												"$$$/LrGeniusAI/PluginInfo/VersionDiffer=Plugin and backend versions differ.\n"
											)
												.. LOC("$$$/LrGeniusAI/PluginInfo/PluginPrefix=Plugin: ")
												.. pluginTag
												.. "\n"
												.. LOC("$$$/LrGeniusAI/PluginInfo/BackendPrefix=Backend: ")
												.. backendTag
												.. "\n"
												.. LOC("$$$/LrGeniusAI/PluginInfo/ReasonPrefix=Reason: ")
												.. Util.userFacingError(
													tostring(
														result.reason or LOC("$$$/LrGeniusAI/common/Unknown=unknown")
													)
												)
												.. "\n\n"
												.. buildInfo,
											"warning"
										)
									end
								else
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/PluginInfo/VersionCheckFailed=Version check failed"),
										Util.userFacingError(
											tostring(err or LOC("$$$/LrGeniusAI/common/UnknownError=Unknown error"))
										),
										"critical"
									)
								end
							end)
						end,
					}),

					f:push_button({
						title = LOC("$$$/LrGeniusAI/PluginInfo/RestartBackend=Restart Backend"),
						width = share("backendButtonWidth"),
						action = function(button)
							LrTasks.startAsyncTask(function()
								local progressScope = LrProgressScope({
									title = LOC("$$$/LrGeniusAI/PluginInfo/Restarting=Restarting..."),
									functionContext = nil,
								})
								local ok, err = SearchIndexAPI.restartBackend()
								progressScope:done()
								if ok then
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/PluginInfo/RestartBackend=Restart Backend"),
										LOC("$$$/LrGeniusAI/PluginInfo/RestartSuccess=Backend restarted successfully."),
										"info"
									)
								else
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/PluginInfo/RestartBackend=Restart Backend"),
										LOC(
											"$$$/LrGeniusAI/PluginInfo/RestartFailed=Failed to restart backend: ^1",
											tostring(err)
										),
										"critical"
									)
								end
							end)
						end,
					}),
				}),
			}),
		},
	}
end

function PluginInfoDialogSections.endDialog(propertyTable)
	-- First, before any of the preference saving below: if one of those lines
	-- throws, the polling loops must still stop.
	propertyTable.keepChecksRunning = false

	local function trimmed(value)
		return (tostring(value or ""):gsub("^%s+", ""):gsub("%s+$", ""))
	end
	Secrets.saveChanged(propertyTable, propertyTable.loadedApiKeys)
	prefs.aiServerUrl = trimmed(propertyTable.aiServerUrl)
	-- Shown for one opening of this dialog; see updateHealth.
	prefs.pendingProviderNotice = nil
	prefs.exportSize = propertyTable.exportSize
	prefs.exportQuality = propertyTable.exportQuality
	prefs.indexSubmitOriginals = (propertyTable.indexSubmitOriginals == true)

	prefs.prompts = propertyTable.prompts
	prefs.prompt = Util.resolvePromptName(propertyTable.prompts, propertyTable.prompt, Defaults.defaultPromptName)

	prefs.logging = propertyTable.logging
	if propertyTable.logging then
		log:enable("logfile")
	else
		log:disable()
	end

	prefs.periodicalUpdateCheck = propertyTable.periodicalUpdateCheck
	prefs.dbStoragePath = (propertyTable.dbStoragePath and propertyTable.dbStoragePath:gsub("^%s*(.-)%s*$", "%1")) or ""

	if prefs.dbStoragePath ~= "" then
		LrTasks.startAsyncTask(function()
			local ok, errMsg = LrTasks.pcall(function()
				local _, err = SearchIndexAPI.initializeCatalog(prefs.dbStoragePath)
				if err then
					error(err)
				end
			end)
			if not ok then
				ErrorHandler.handleError(
					"DbPathInit",
					LOC(
						"$$$/LrGeniusAI/PluginInfo/DbPathInitFailed=Failed to initialize database at the selected path: ^1\n\nCheck that the folder exists and is writable.",
						tostring(errMsg)
					)
				)
			end
		end)
	end

	prefs.useClip = propertyTable.useClip

	if propertyTable.backendServerUrl and propertyTable.backendServerUrl:gsub("^%s*(.-)%s*$", "%1") ~= "" then
		prefs.backendServerUrl = propertyTable.backendServerUrl:gsub("^%s*(.-)%s*$", "%1")
	else
		prefs.backendServerUrl = Defaults.defaultBackendServerUrl
	end

	-- Local-model tuning. Clamped here rather than trusted: a context of zero
	-- or a negative parallel count would be rejected by the server anyway, and
	-- falling back to the default is friendlier than an error on first index.
	local function positiveIntOr(value, fallback)
		local n = tonumber(value)
		if n == nil or n ~= n or n < 1 then
			return fallback
		end
		return math.floor(n)
	end
	prefs.llmContextSize = positiveIntOr(propertyTable.llmContextSize, Defaults.defaultLlmContextSize)
	prefs.llmParallel = positiveIntOr(propertyTable.llmParallel, Defaults.defaultLlmParallel)
	-- Zero GPU layers is meaningful (pure CPU), so it is allowed through.
	local gpuLayers = tonumber(propertyTable.llmGpuLayers)
	if gpuLayers == nil or gpuLayers ~= gpuLayers or gpuLayers < 0 then
		prefs.llmGpuLayers = Defaults.defaultLlmGpuLayers
	else
		prefs.llmGpuLayers = math.floor(gpuLayers)
	end
end
