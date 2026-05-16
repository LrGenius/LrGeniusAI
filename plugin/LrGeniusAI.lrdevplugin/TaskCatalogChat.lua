--[[
    Catalog Chat — "Ask Your Catalog"
    Conversational interface to query and act on the Lightroom catalog via AI.
    The agent loop runs on the server; the plugin is a chat UI + action applier.
]]

local function getCatalogId()
	local catalog = LrApplication.activeCatalog()
	if catalog and catalog.localIdentifier then
		return tostring(catalog.localIdentifier)
	end
	return nil
end

local function splitModelKey(modelKey)
	local sep = string.find(modelKey or "", "::", 1, true)
	if sep then
		return string.sub(modelKey, 1, sep - 1), string.sub(modelKey, sep + 2)
	end
	return modelKey or "chatgpt", ""
end

-- ─── Model-picker dialog ─────────────────────────────────────────────────────

local function showModelPickerDialog(context)
	local f = LrView.osFactory()
	local bind = LrView.bind
	local share = LrView.share

	-- Fetch available models from server
	local modelItems = {}
	local openaiKey = (prefs and not Util.nilOrEmpty(prefs.chatgptApiKey)) and prefs.chatgptApiKey or nil
	local geminiKey = (prefs and not Util.nilOrEmpty(prefs.geminiApiKey)) and prefs.geminiApiKey or nil

	local modelsResp = SearchIndexAPI.getModels(openaiKey, geminiKey)
	if modelsResp and modelsResp.models then
		for provider, list in pairs(modelsResp.models) do
			for _, model in ipairs(list) do
				table.insert(modelItems, {
					title = provider .. ": " .. model,
					value = provider .. "::" .. model,
				})
			end
		end
	end
	table.sort(modelItems, function(a, b)
		return a.title < b.title
	end)
	if #modelItems == 0 then
		table.insert(modelItems, { title = "chatgpt: gpt-4.1", value = "chatgpt::gpt-4.1" })
	end

	local props = LrBinding.makePropertyTable(context)
	-- Start with the last-used chat model, fall back to global model key
	props.modelKey = prefs.chatModelKey or prefs.modelKey or modelItems[1].value
	-- Make sure the saved key is in the list; if not, default to first item
	local keyInList = false
	for _, item in ipairs(modelItems) do
		if item.value == props.modelKey then
			keyInList = true
			break
		end
	end
	if not keyInList then
		props.modelKey = modelItems[1].value
	end

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:row({
			spacing = f:label_spacing(),
			f:static_text({
				title = LOC("$$$/LrGeniusAI/CatalogChat/ModelLabel=AI Model:"),
				width = share("labelWidth"),
				alignment = "right",
			}),
			f:popup_menu({
				value = bind("modelKey"),
				items = modelItems,
				width = 300,
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/CatalogChat/Title=Catalog Chat (beta)"),
		contents = contents,
		actionVerb = LOC("$$$/LrGeniusAI/CatalogChat/StartButton=Start Chat"),
	})

	if result == "ok" then
		prefs.chatModelKey = props.modelKey
		return props.modelKey
	end
	return nil
end

-- ─── API helpers ────────────────────────────────────────────────────────────

local function apiCreateSession(provider, model, catalogId)
	local body = { provider = provider, model = model }
	if catalogId then
		body.catalog_id = catalogId
	end
	-- send the provider's API key so the server can authenticate
	local p = (provider or ""):lower()
	if p == "chatgpt" or p == "openai" then
		body.api_key = (prefs and prefs.chatgptApiKey ~= "") and prefs.chatgptApiKey or nil
	elseif p == "gemini" then
		body.api_key = (prefs and prefs.geminiApiKey ~= "") and prefs.geminiApiKey or nil
	end
	local result, err = SearchIndexAPI._chatRequest("POST", "/chat/session", body)
	if err then
		return nil, err
	end
	if result and result.error then
		return nil, result.error
	end
	return result and result.results, nil
end

local function apiPostTurn(sessionId, message)
	local body = { session_id = sessionId, message = message }
	local result, err = SearchIndexAPI._chatRequest("POST", "/chat/turn", body)
	if err then
		return nil, err
	end
	if result and result.error then
		return nil, result.error
	end
	return result and result.results, nil
end

local function apiGetEvents(sessionId, turnId, cursor)
	local url = SearchIndexAPI._chatBaseUrl()
		.. "/chat/turn/"
		.. turnId
		.. "/events?cursor="
		.. tostring(cursor)
		.. "&session_id="
		.. turnId
	-- inject db_path for local backends
	if SearchIndexAPI.isLocalBackend() then
		local dbPath = SearchIndexAPI._getDbPath()
		if dbPath then
			url = url .. "&db_path=" .. SearchIndexAPI._urlEncode(dbPath)
		end
	end
	local result, hdrs = LrHttp.get(url)
	if not result then
		return nil, "Network error"
	end
	local ok, decoded = LrTasks.pcall(JSON.decode, JSON, result)
	if not ok then
		return nil, "JSON decode error"
	end
	if decoded and decoded.error then
		return nil, decoded.error
	end
	return decoded and decoded.results, nil
end

local function apiCommit(sessionId, proposalId)
	local body = { session_id = sessionId, proposal_id = proposalId }
	local result, err = SearchIndexAPI._chatRequest("POST", "/chat/commit", body)
	if err then
		return nil, err
	end
	if result and result.error then
		return nil, result.error
	end
	return result and result.results, nil
end

-- ─── Apply actions ───────────────────────────────────────────────────────────

local function applyAction(catalog, action, props)
	local kind = action.kind or ""
	local payload = action.payload or {}
	local photoIds = payload.photo_ids or {}

	-- Resolve photo_ids to LrPhoto objects
	local photos = {}
	if #photoIds > 0 then
		LrTasks.pcall(function()
			for _, pid in ipairs(photoIds) do
				local photo = Util.getPhotoForGlobalPhotoId(pid)
				if photo then
					photos[#photos + 1] = photo
				end
			end
		end)
	end

	if kind == "create_collection" then
		local name = payload.collection_name
			or LOC("$$$/LrGeniusAI/CatalogChat/DefaultCollectionName=AI Chat Collection")
		catalog:withWriteAccessDo(LOC("$$$/LrGeniusAI/CatalogChat/CreateCollection=Create collection"), function()
			local coll = catalog:createCollection(name, nil, true)
			if coll and #photos > 0 then
				coll:addPhotos(photos)
			end
		end)
		return true,
			string.format(
				LOC("$$$/LrGeniusAI/CatalogChat/CollectionCreated=Collection '%s' created with ^1 photo(s)."),
				name,
				#photos
			)
	elseif kind == "set_rating" then
		local rating = tonumber(payload.rating) or 0
		catalog:withWriteAccessDo(LOC("$$$/LrGeniusAI/CatalogChat/SetRating=Set rating"), function()
			for _, photo in ipairs(photos) do
				photo:setRawMetadata("rating", rating)
			end
		end)
		return true,
			string.format(
				LOC("$$$/LrGeniusAI/CatalogChat/RatingSet=Rating set to ^1 for ^2 photo(s)."),
				rating,
				#photos
			)
	elseif kind == "set_flag" then
		local flagMap = { pick = 1, reject = -1, unflagged = 0 }
		local flagVal = flagMap[payload.flag or "unflagged"] or 0
		catalog:withWriteAccessDo(LOC("$$$/LrGeniusAI/CatalogChat/SetFlag=Set flag"), function()
			for _, photo in ipairs(photos) do
				photo:setRawMetadata("pickStatus", flagVal)
			end
		end)
		return true, string.format(LOC("$$$/LrGeniusAI/CatalogChat/FlagSet=Flag set for ^1 photo(s)."), #photos)
	elseif kind == "set_color_label" then
		local label = payload.color_label or ""
		catalog:withWriteAccessDo(LOC("$$$/LrGeniusAI/CatalogChat/SetColorLabel=Set color label"), function()
			for _, photo in ipairs(photos) do
				photo:setRawMetadata("colorNameForLabel", label)
			end
		end)
		return true,
			string.format(LOC("$$$/LrGeniusAI/CatalogChat/ColorLabelSet=Color label set for ^1 photo(s)."), #photos)
	elseif kind == "export_csv" then
		-- Write a simple CSV
		local lines = { "photo_id" }
		for _, pid in ipairs(photoIds) do
			lines[#lines + 1] = pid
		end
		local csvContent = table.concat(lines, "\n")
		local tmpDir = LrPathUtils.getStandardFilePath("temp")
		local csvPath = LrPathUtils.child(tmpDir, "catalog_chat_export.csv")
		LrTasks.pcall(function()
			local f = LrFileUtils.openForWriting(csvPath)
			if f then
				f:write(csvContent)
				f:close()
			end
		end)
		return true,
			string.format(
				LOC("$$$/LrGeniusAI/CatalogChat/CsvExported=CSV with ^1 IDs exported to: ") .. csvPath,
				#photoIds
			)
	end

	return false, LOC("$$$/LrGeniusAI/CatalogChat/UnknownAction=Unknown action: ") .. kind
end

-- ─── UI helpers ─────────────────────────────────────────────────────────────

local MAX_LINES = 200
local LINE_WIDTH = 70

local function wrapText(text, width)
	-- Simple word-wrap to avoid the broken wrap=true in LR SDK
	width = width or LINE_WIDTH
	local lines = {}
	for paragraph in (text .. "\n"):gmatch("([^\n]*)\n") do
		if #paragraph == 0 then
			lines[#lines + 1] = " "
		else
			local line = ""
			for word in paragraph:gmatch("%S+") do
				if #line == 0 then
					line = word
				elseif #line + 1 + #word <= width then
					line = line .. " " .. word
				else
					lines[#lines + 1] = line
					line = word
				end
			end
			if #line > 0 then
				lines[#lines + 1] = line
			end
		end
	end
	return table.concat(lines, "\n")
end

local function formatEventLine(event)
	local kind = event.kind or ""
	local payload = event.payload or {}
	if kind == "tool_call" then
		return "  [" .. (payload.tool or "?") .. "] " .. (payload.args_preview or "")
	elseif kind == "tool_result" then
		return "  → " .. (payload.summary_text or "")
	elseif kind == "assistant_text" then
		return wrapText(payload.text or "")
	elseif kind == "proposal" then
		return "  ★ " .. wrapText(payload.dry_run_summary or "Proposed action")
	elseif kind == "error" then
		return "  ⚠ " .. (payload.message or "Error")
	elseif kind == "done" then
		return nil
	end
	return nil
end

-- ─── Main dialog ─────────────────────────────────────────────────────────────

local function showChatDialog(context)
	local props = LrBinding.makePropertyTable(context)
	props.transcript = LOC("$$$/LrGeniusAI/CatalogChat/Welcome=Catalog Chat ready. Ask anything about your photos.")
	props.inputText = ""
	props.statusText = ""
	props.sessionId = nil
	props.pendingProposal = nil
	props.isSending = false
	props.hasProposal = false

	local f = LrView.osFactory()
	local bind = LrView.bind

	local function appendLine(text)
		if not text or text == "" then
			return
		end
		local current = props.transcript or ""
		local lines = {}
		for l in (current .. "\n"):gmatch("([^\n]*)\n") do
			lines[#lines + 1] = l
		end
		-- Append new lines (text may contain \n)
		for newLine in (text .. "\n"):gmatch("([^\n]*)\n") do
			lines[#lines + 1] = newLine
		end
		-- Keep at most MAX_LINES
		while #lines > MAX_LINES do
			table.remove(lines, 1)
		end
		props.transcript = table.concat(lines, "\n")
	end

	local function appendSeparator()
		appendLine(string.rep("─", 50))
	end

	-- Poll events for a turn
	local function pollEvents(sessionId, turnId)
		local cursor = 0
		local done = false
		while not done do
			local result, err = apiGetEvents(sessionId, turnId, cursor)
			if err then
				appendLine(LOC("$$$/LrGeniusAI/CatalogChat/PollError=Error: ") .. tostring(err))
				break
			end
			if result then
				local events = result.events or {}
				for _, event in ipairs(events) do
					local line = formatEventLine(event)
					if line then
						appendLine(line)
					end
					if event.kind == "proposal" and event.payload then
						props.pendingProposal = event.payload
						props.hasProposal = true
					end
				end
				cursor = result.next_cursor or (cursor + #events)
				done = result.done or false
			end
			if not done then
				LrTasks.sleep(0.3)
			end
		end
	end

	-- Send a message
	local function sendMessage()
		local text = (props.inputText or ""):match("^%s*(.-)%s*$")
		if text == "" then
			return
		end
		if props.isSending then
			return
		end
		if not props.sessionId then
			appendLine(LOC("$$$/LrGeniusAI/CatalogChat/NotConnected=Not connected. Please wait or reopen."))
			return
		end
		props.inputText = ""
		props.isSending = true
		props.hasProposal = false
		props.pendingProposal = nil
		props.statusText = LOC("$$$/LrGeniusAI/CatalogChat/Thinking=Thinking...")
		appendLine("")
		appendLine(LOC("$$$/LrGeniusAI/CatalogChat/YouLabel=You: ") .. text)
		appendLine(LOC("$$$/LrGeniusAI/CatalogChat/AssistantLabel=Assistant:"))

		LrTasks.startAsyncTask(function()
			local ok, errMsg = LrTasks.pcall(function()
				local result, err = apiPostTurn(props.sessionId, text)
				if err then
					appendLine(LOC("$$$/LrGeniusAI/CatalogChat/TurnError=Error: ") .. tostring(err))
					return
				end
				local turnId = result and result.turn_id
				if not turnId then
					appendLine(LOC("$$$/LrGeniusAI/CatalogChat/NoTurnId=Server returned no turn_id."))
					return
				end
				pollEvents(props.sessionId, turnId)
				appendSeparator()
			end)
			if not ok then
				appendLine(LOC("$$$/LrGeniusAI/CatalogChat/UnexpectedError=Unexpected error: ") .. tostring(errMsg))
			end
			props.isSending = false
			props.statusText = ""
		end)
	end

	-- Apply a pending proposal
	local function applyProposal()
		local proposal = props.pendingProposal
		if not proposal then
			return
		end
		props.isSending = true
		props.statusText = LOC("$$$/LrGeniusAI/CatalogChat/Applying=Applying...")

		LrTasks.startAsyncTask(function()
			local ok, errMsg = LrTasks.pcall(function()
				-- Commit on server
				local result, err = apiCommit(props.sessionId, proposal.proposal_id)
				if err then
					LrDialogs.message(
						LOC("$$$/LrGeniusAI/CatalogChat/ApplyErrorTitle=Apply Error"),
						tostring(err),
						"critical"
					)
					return
				end
				-- Apply in Lightroom
				local action = result and result.action
				if not action then
					LrDialogs.message(
						LOC("$$$/LrGeniusAI/CatalogChat/ApplyErrorTitle=Apply Error"),
						LOC("$$$/LrGeniusAI/CatalogChat/NoAction=Server returned no action"),
						"critical"
					)
					return
				end
				local catalog = LrApplication.activeCatalog()
				local applied, msg = applyAction(catalog, action, props)
				if applied then
					appendLine(LOC("$$$/LrGeniusAI/CatalogChat/Applied=✓ Applied: ") .. (msg or ""))
				else
					LrDialogs.message(
						LOC("$$$/LrGeniusAI/CatalogChat/ApplyErrorTitle=Apply Error"),
						msg or LOC("$$$/LrGeniusAI/CatalogChat/ApplyFailed=Action could not be applied."),
						"critical"
					)
				end
				props.hasProposal = false
				props.pendingProposal = nil
			end)
			if not ok then
				LrDialogs.message(
					LOC("$$$/LrGeniusAI/CatalogChat/ApplyErrorTitle=Apply Error"),
					tostring(errMsg),
					"critical"
				)
			end
			props.isSending = false
			props.statusText = ""
		end)
	end

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		width = 640,

		-- Transcript area
		f:group_box({
			title = LOC("$$$/LrGeniusAI/CatalogChat/TranscriptTitle=Conversation"),
			width = 640,
			f:scrolled_view({
				width = 620,
				height = 380,
				f:static_text({
					title = bind("transcript"),
					width = 600,
					height_in_lines = 20,
					font = "<system/small>",
				}),
			}),
		}),

		-- Status line
		f:row({
			f:static_text({
				title = bind("statusText"),
				width = 620,
				font = "<system/small>",
				text_color = LrColor(0.5, 0.5, 0.5),
			}),
		}),

		-- Input row
		f:row({
			spacing = f:label_spacing(),
			f:edit_field({
				value = bind("inputText"),
				width_in_chars = 50,
				height_in_lines = 3,
			}),
			f:push_button({
				title = LOC("$$$/LrGeniusAI/CatalogChat/SendButton=Send"),
				action = sendMessage,
				enabled = bind({
					key = "isSending",
					transform = function(v)
						return not v
					end,
				}),
			}),
		}),

		-- Proposal row (only visible when hasProposal is true)
		f:row({
			f:push_button({
				title = LOC("$$$/LrGeniusAI/CatalogChat/ApplyButton=Apply Proposed Action"),
				action = applyProposal,
				enabled = bind({
					keys = { "hasProposal", "isSending" },
					operation = function(binder, values, fromTable)
						return values.hasProposal and not values.isSending
					end,
				}),
			}),
			f:push_button({
				title = LOC("$$$/LrGeniusAI/CatalogChat/DiscardButton=Discard"),
				action = function()
					props.hasProposal = false
					props.pendingProposal = nil
					appendLine(LOC("$$$/LrGeniusAI/CatalogChat/Discarded=Action discarded."))
				end,
				enabled = bind("hasProposal"),
			}),
		}),
	})

	return LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/CatalogChat/Title=Catalog Chat (beta)"),
		contents = contents,
		actionVerb = LOC("$$$/LrGeniusAI/CatalogChat/CloseButton=Close"),
		cancelVerb = "< exclude >",
		resizable = false,
	})
end

-- ─── Entry point ─────────────────────────────────────────────────────────────

LrTasks.startAsyncTask(function()
	local ok, err = LrTasks.pcall(function()
		if not Util.waitForServerDialog({ requireClip = false }) then
			return
		end

		LrFunctionContext.callWithContext("CatalogChat", function(context)
			-- Step 1: show model picker
			local modelKey = showModelPickerDialog(context)
			if not modelKey then
				return
			end -- user cancelled

			local provider, model = splitModelKey(modelKey)
			local catalogId = getCatalogId()

			-- Step 2: create session with selected model
			local sessionResult, sessionErr = apiCreateSession(provider, model, catalogId)
			if sessionErr then
				LrDialogs.message(
					LOC("$$$/LrGeniusAI/CatalogChat/Title=Catalog Chat (beta)"),
					LOC("$$$/LrGeniusAI/CatalogChat/SessionError=Could not start chat session: ")
						.. tostring(sessionErr),
					"critical"
				)
				return
			end

			local sessionId = sessionResult and sessionResult.session_id
			if not sessionId then
				LrDialogs.message(
					LOC("$$$/LrGeniusAI/CatalogChat/Title=Catalog Chat (beta)"),
					LOC("$$$/LrGeniusAI/CatalogChat/SessionError=Could not start chat session: no session_id returned"),
					"critical"
				)
				return
			end

			-- Step 3: open full chat dialog
			LrFunctionContext.callWithContext("CatalogChatDialog", function(innerCtx)
				local f = LrView.osFactory()
				local bind = LrView.bind

				-- Chat dialog with sessionId bound via closure
				local chatProps = LrBinding.makePropertyTable(innerCtx)
				chatProps.transcript =
					LOC("$$$/LrGeniusAI/CatalogChat/Welcome=Catalog Chat ready. Ask anything about your photos.")
				chatProps.inputText = ""
				chatProps.statusText = ""
				chatProps.sessionId = sessionId
				chatProps.pendingProposal = nil
				chatProps.isSending = false
				chatProps.hasProposal = false

				local function appendLine(text)
					if not text or text == "" then
						return
					end
					local current = chatProps.transcript or ""
					local lines = {}
					for l in (current .. "\n"):gmatch("([^\n]*)\n") do
						lines[#lines + 1] = l
					end
					for newLine in (text .. "\n"):gmatch("([^\n]*)\n") do
						lines[#lines + 1] = newLine
					end
					while #lines > MAX_LINES do
						table.remove(lines, 1)
					end
					chatProps.transcript = table.concat(lines, "\n")
				end

				local function appendSeparator()
					appendLine(string.rep("─", 50))
				end

				local function pollEvents(sId, turnId)
					local cursor = 0
					local done = false
					while not done do
						local result, err2 = apiGetEvents(sId, turnId, cursor)
						if err2 then
							appendLine(LOC("$$$/LrGeniusAI/CatalogChat/PollError=Error: ") .. tostring(err2))
							break
						end
						if result then
							local events = result.events or {}
							for _, event in ipairs(events) do
								local line = formatEventLine(event)
								if line then
									appendLine(line)
								end
								if event.kind == "proposal" and event.payload then
									chatProps.pendingProposal = event.payload
									chatProps.hasProposal = true
								end
							end
							cursor = result.next_cursor or (cursor + #events)
							done = result.done or false
						end
						if not done then
							LrTasks.sleep(0.3)
						end
					end
				end

				local function sendMessage()
					local text = (chatProps.inputText or ""):match("^%s*(.-)%s*$")
					if text == "" then
						return
					end
					if chatProps.isSending then
						return
					end
					chatProps.inputText = ""
					chatProps.isSending = true
					chatProps.hasProposal = false
					chatProps.pendingProposal = nil
					chatProps.statusText = LOC("$$$/LrGeniusAI/CatalogChat/Thinking=Thinking...")
					appendLine("")
					appendLine(LOC("$$$/LrGeniusAI/CatalogChat/YouLabel=You: ") .. text)
					appendLine(LOC("$$$/LrGeniusAI/CatalogChat/AssistantLabel=Assistant:"))

					LrTasks.startAsyncTask(function()
						local pOk, pErr = LrTasks.pcall(function()
							local result, err2 = apiPostTurn(sessionId, text)
							if err2 then
								appendLine(LOC("$$$/LrGeniusAI/CatalogChat/TurnError=Error: ") .. tostring(err2))
								return
							end
							local turnId = result and result.turn_id
							if not turnId then
								appendLine(LOC("$$$/LrGeniusAI/CatalogChat/NoTurnId=Server returned no turn_id."))
								return
							end
							pollEvents(sessionId, turnId)
							appendSeparator()
						end)
						if not pOk then
							appendLine(
								LOC("$$$/LrGeniusAI/CatalogChat/UnexpectedError=Unexpected error: ") .. tostring(pErr)
							)
						end
						chatProps.isSending = false
						chatProps.statusText = ""
					end)
				end

				local function applyProposal()
					local proposal = chatProps.pendingProposal
					if not proposal then
						return
					end
					chatProps.isSending = true
					chatProps.statusText = LOC("$$$/LrGeniusAI/CatalogChat/Applying=Applying...")
					LrTasks.startAsyncTask(function()
						local aOk, aErr = LrTasks.pcall(function()
							local result, err2 = apiCommit(sessionId, proposal.proposal_id)
							if err2 then
								LrDialogs.message(
									LOC("$$$/LrGeniusAI/CatalogChat/ApplyErrorTitle=Apply Error"),
									tostring(err2),
									"critical"
								)
								return
							end
							local action = result and result.action
							if action then
								local catalog = LrApplication.activeCatalog()
								local applied, msg = applyAction(catalog, action, chatProps)
								if applied then
									appendLine(LOC("$$$/LrGeniusAI/CatalogChat/Applied=Applied: ") .. (msg or ""))
								else
									LrDialogs.message(
										LOC("$$$/LrGeniusAI/CatalogChat/ApplyErrorTitle=Apply Error"),
										msg
											or LOC(
												"$$$/LrGeniusAI/CatalogChat/ApplyFailed=Action could not be applied."
											),
										"critical"
									)
								end
							end
							chatProps.hasProposal = false
							chatProps.pendingProposal = nil
						end)
						if not aOk then
							LrDialogs.message(
								LOC("$$$/LrGeniusAI/CatalogChat/ApplyErrorTitle=Apply Error"),
								tostring(aErr),
								"critical"
							)
						end
						chatProps.isSending = false
						chatProps.statusText = ""
					end)
				end

				local contents = f:column({
					bind_to_object = chatProps,
					spacing = f:control_spacing(),
					width = 680,
					f:group_box({
						title = LOC("$$$/LrGeniusAI/CatalogChat/TranscriptTitle=Conversation"),
						width = 680,
						f:scrolled_view({
							width = 660,
							height = 400,
							f:static_text({
								title = bind("transcript"),
								width = 640,
								height_in_lines = 22,
								font = "<system/small>",
							}),
						}),
					}),
					f:row({
						f:static_text({
							title = bind("statusText"),
							width = 640,
							font = "<system/small>",
							text_color = LrColor(0.5, 0.5, 0.5),
						}),
					}),
					f:row({
						spacing = f:label_spacing(),
						f:edit_field({
							value = bind("inputText"),
							width_in_chars = 55,
							height_in_lines = 3,
						}),
						f:push_button({
							title = LOC("$$$/LrGeniusAI/CatalogChat/SendButton=Send"),
							action = sendMessage,
							enabled = bind({
								key = "isSending",
								transform = function(v)
									return not v
								end,
							}),
						}),
					}),
					f:row({
						spacing = f:label_spacing(),
						f:push_button({
							title = LOC("$$$/LrGeniusAI/CatalogChat/ApplyButton=Apply Proposed Action"),
							action = applyProposal,
							enabled = bind({
								keys = { "hasProposal", "isSending" },
								operation = function(binder, values, fromTable)
									return values.hasProposal and not values.isSending
								end,
							}),
						}),
						f:push_button({
							title = LOC("$$$/LrGeniusAI/CatalogChat/DiscardButton=Discard"),
							action = function()
								chatProps.hasProposal = false
								chatProps.pendingProposal = nil
								appendLine(LOC("$$$/LrGeniusAI/CatalogChat/Discarded=Action discarded."))
							end,
							enabled = bind("hasProposal"),
						}),
					}),
				})

				LrDialogs.presentFloatingDialog(_PLUGIN, {
					title = LOC("$$$/LrGeniusAI/CatalogChat/Title=Catalog Chat (beta)"),
					contents = contents,
					-- actionVerb = LOC("$$$/LrGeniusAI/CatalogChat/CloseButton=Close"),
					-- cancelVerb = "< exclude >",
					resizable = false,
				})
			end)
		end)
	end)
	if not ok then
		ErrorHandler.handleError(err)
	end
end)
