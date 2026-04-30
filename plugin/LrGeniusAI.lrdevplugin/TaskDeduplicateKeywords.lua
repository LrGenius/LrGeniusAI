-- TaskDeduplicateKeywords.lua
-- SigLIP-based keyword clustering + optional LLM validation.
-- Only leaf keywords (no children) are candidates for deduplication.

local function collectKeywordsUnder(keyword, result)
	result = result or {}
	table.insert(result, keyword)
	local ok, children = LrTasks.pcall(function()
		return keyword:getChildren() or {}
	end)
	if ok and type(children) == "table" then
		for _, child in ipairs(children) do
			collectKeywordsUnder(child, result)
		end
	end
	return result
end

local function collectAllKeywords(roots)
	local all = {}
	for _, kw in ipairs(roots) do
		collectKeywordsUnder(kw, all)
	end
	return all
end

local function buildNameMap(keywords)
	local map = {}
	for _, kw in ipairs(keywords) do
		local ok, name = LrTasks.pcall(function()
			return kw:getName()
		end)
		if ok and type(name) == "string" and name ~= "" then
			local key = string.lower(Util.trim(name))
			if not map[key] then
				map[key] = kw
			end
		end
	end
	return map
end

-- Executes a single keyword merge: re-tags photos with the canonical keyword
-- and removes them from the duplicate. The duplicate keyword entry itself is
-- left in the catalog (the Lightroom SDK has no deleteKeyword API); it will
-- appear with 0 photos and can be removed via Metadata > Purge Unused Keywords.
-- Returns true on success, or nil + reason string on failure/skip.
local function executeMerge(catalog, pair)
	local okChildren, children = LrTasks.pcall(function()
		return pair.duplicate:getChildren() or {}
	end)
	if not okChildren or (type(children) == "table" and #children > 0) then
		log:warn("DeduplicateKeywords: Skipping '" .. pair.duplicateName .. "' — has child keywords")
		return nil, pair.duplicateName .. " (has children)"
	end

	local okPhotos, photos = LrTasks.pcall(function()
		return pair.duplicate:getPhotos() or {}
	end)
	if not okPhotos then
		log:error("DeduplicateKeywords: getPhotos failed for '" .. pair.duplicateName .. "': " .. tostring(photos))
		return nil, pair.duplicateName .. " (error reading photos)"
	end

	local ok, err = LrTasks.pcall(function()
		catalog:withWriteAccessDo(
			"Deduplicate keyword: " .. pair.duplicateName .. " → " .. pair.canonicalName,
			function()
				for _, photo in ipairs(photos) do
					local addOk, addErr = LrTasks.pcall(function()
						photo:addKeyword(pair.canonical)
					end)
					if not addOk then
						log:error(
							"DeduplicateKeywords: addKeyword failed for '"
								.. pair.duplicateName
								.. "': "
								.. tostring(addErr)
						)
					end
					local rmOk, rmErr = LrTasks.pcall(function()
						photo:removeKeyword(pair.duplicate)
					end)
					if not rmOk then
						log:error(
							"DeduplicateKeywords: removeKeyword failed for '"
								.. pair.duplicateName
								.. "': "
								.. tostring(rmErr)
						)
					end
				end
			end,
			Defaults.catalogWriteAccessOptions
		)
	end)
	if ok then
		log:info(
			"DeduplicateKeywords: Merged '"
				.. pair.duplicateName
				.. "' → '"
				.. pair.canonicalName
				.. "' ("
				.. #photos
				.. " photo(s) re-tagged, keyword entry remains — purge via Metadata > Purge Unused Keywords)"
		)
		return true
	else
		log:error("DeduplicateKeywords: merge failed for '" .. pair.duplicateName .. "': " .. tostring(err))
		return nil, pair.duplicateName .. " (merge failed)"
	end
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("DeduplicateKeywordsTask", function(context)
		local catalog = LrApplication.activeCatalog()
		local f = LrView.osFactory()
		local bind = LrView.bind

		-- Load available LLM models from server
		local modelItems = {}
		do
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
				table.insert(modelItems, { title = "qwen: (default)", value = "qwen::" })
			end
		end

		-- ── Step 1: Warning + model selection + backup confirmation ──────────
		local warnProps = LrBinding.makePropertyTable(context)
		warnProps.hasBackup = false
		warnProps.modelKey = prefs.deduplicateModelKey or prefs.modelKey or modelItems[1].value
		if not warnProps.modelKey or warnProps.modelKey == "" then
			warnProps.modelKey = modelItems[1].value
		end

		local warnView = f:column({
			bind_to_object = warnProps,
			spacing = f:control_spacing(),
			width = 450,
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/WarningIntro=This tool uses AI (SigLIP similarity + LLM validation) to find\nleaf keywords that are true synonyms and suggests merging them.\nPhotos are re-tagged with the canonical keyword. Duplicate entries\nremain in the catalog with 0 photos — remove via\nMetadata > Purge Unused Keywords."
				),
				fill_horizontal = 1,
				wrap = true,
			}),
			f:separator({ fill_horizontal = 1 }),
			f:group_box({
				title = LOC("$$$/LrGeniusAI/AnalyzeAndIndex/AISettings=AI Model"),
				fill_horizontal = 1,
				f:row({
					f:static_text({
						title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/AIModelLabel=AI Model:"),
						width = 70,
					}),
					f:popup_menu({
						value = bind("modelKey"),
						items = modelItems,
						width = 290,
					}),
				}),
			}),
			f:separator({ fill_horizontal = 1 }),
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/WarningRisk=Warning: This permanently modifies your catalog.\nDeleted keywords cannot be recovered.\nBack up first: File > Catalog Settings > Back Up Catalog."
				),
				fill_horizontal = 1,
				wrap = true,
				text_color = LrColor(0.8, 0.2, 0.0),
			}),
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/LLMCostNote=Note: When using ChatGPT or Gemini, AI analysis will incur API costs."
				),
				fill_horizontal = 1,
				wrap = true,
				text_color = LrColor(0.5, 0.35, 0.0),
			}),
			f:spacer({ height = 4 }),
			f:checkbox({
				value = bind("hasBackup"),
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/BackupConfirm=I have a recent catalog backup and understand\nthis operation cannot be undone."
				),
				wrap = true,
			}),
		})

		local warnResult = LrDialogs.presentModalDialog({
			title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/WarningTitle=Deduplicate Keyword Synonyms"),
			contents = warnView,
			actionVerb = LOC("$$$/LrGeniusAI/DeduplicateKeywords/ContinueToSelect=Continue"),
			cancelVerb = LOC("$$$/LrGeniusAI/common/Cancel=Cancel"),
		})
		if warnResult ~= "ok" then
			return
		end

		if not warnProps.hasBackup then
			LrDialogs.showError(
				LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/BackupRequiredMessage=Please confirm you have a catalog backup before continuing."
				)
			)
			return
		end

		-- Save model preference for next run
		prefs.deduplicateModelKey = warnProps.modelKey

		-- ── Step 2: Keyword branch selection ──────────────────────────────
		local okTopKw, topKeywords = LrTasks.pcall(function()
			return catalog:getKeywords() or {}
		end)
		if not okTopKw then
			log:error("DeduplicateKeywords: getKeywords failed: " .. tostring(topKeywords))
			LrDialogs.showError(
				LOC("$$$/LrGeniusAI/DeduplicateKeywords/GetKeywordsError=Failed to read catalog keywords.")
			)
			return
		end
		if #topKeywords == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/DeduplicateKeywords/NoKeywordsTitle=No Keywords"),
				LOC("$$$/LrGeniusAI/DeduplicateKeywords/NoKeywordsMessage=The catalog has no keywords to process.")
			)
			return
		end

		local kwEntries = {}
		for _, kw in ipairs(topKeywords) do
			local ok, name = LrTasks.pcall(function()
				return kw:getName()
			end)
			if ok and name then
				table.insert(kwEntries, { kw = kw, name = name })
			end
		end
		table.sort(kwEntries, function(a, b)
			return a.name < b.name
		end)

		local configProps = LrBinding.makePropertyTable(context)
		for i = 1, #kwEntries do
			configProps["kwSel_" .. i] = true
		end

		local kwCheckboxRows = { spacing = 2 }
		for i, entry in ipairs(kwEntries) do
			table.insert(
				kwCheckboxRows,
				f:checkbox({
					value = bind("kwSel_" .. i),
					title = entry.name,
				})
			)
		end

		local configView = f:column({
			bind_to_object = configProps,
			spacing = f:control_spacing(),
			width = 520,
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/SelectPathsHint=Select the top-level keyword branches to scan for duplicates.\nOnly leaf keywords (without sub-keywords) are considered."
				),
				fill_horizontal = 1,
				wrap = true,
			}),
			f:spacer({ height = 4 }),
			f:row({
				f:push_button({
					title = LOC("$$$/LrGeniusAI/MetadataManager/SelectAll=Select All"),
					action = function()
						for i = 1, #kwEntries do
							configProps["kwSel_" .. i] = true
						end
					end,
				}),
				f:push_button({
					title = LOC("$$$/LrGeniusAI/MetadataManager/DeselectAll=Deselect All"),
					action = function()
						for i = 1, #kwEntries do
							configProps["kwSel_" .. i] = false
						end
					end,
				}),
			}),
			f:scrolled_view({
				height = 280,
				width = 500,
				f:column(kwCheckboxRows),
			}),
		})

		local configResult = LrDialogs.presentModalDialog({
			title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/ConfigTitle=Select Keyword Branches to Scan"),
			contents = configView,
			actionVerb = LOC("$$$/LrGeniusAI/DeduplicateKeywords/Analyze=Scan for Duplicates"),
			cancelVerb = LOC("$$$/LrGeniusAI/common/Cancel=Cancel"),
		})
		if configResult ~= "ok" then
			return
		end

		local selectedRoots = {}
		for i, entry in ipairs(kwEntries) do
			if configProps["kwSel_" .. i] then
				table.insert(selectedRoots, entry.kw)
			end
		end
		if #selectedRoots == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/DeduplicateKeywords/NoSelectionTitle=Nothing Selected"),
				LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/NoSelectionMessage=No keyword branches were selected. Please select at least one branch."
				)
			)
			return
		end

		-- ── Step 3: Scan — AI semantic clustering (leaf keywords only) ────────
		local scanScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/ScanProgressTitle=Scanning keyword catalog..."),
			functionContext = context,
		})
		scanScope:setCaption(LOC("$$$/LrGeniusAI/DeduplicateKeywords/ScanningCaption=Building keyword index..."))
		LrTasks.yield()

		local allCatalogKeywords = collectAllKeywords(topKeywords)
		local allNameMap = buildNameMap(allCatalogKeywords)
		local selectedKeywords = collectAllKeywords(selectedRoots)

		-- Only pass leaf keywords (no children) to the cluster API
		local leafKeywordNames = {}
		for _, kw in ipairs(selectedKeywords) do
			local okC, children = LrTasks.pcall(function()
				return kw:getChildren() or {}
			end)
			if okC and type(children) == "table" and #children == 0 then
				local okN, name = LrTasks.pcall(function()
					return kw:getName()
				end)
				if okN and type(name) == "string" and name ~= "" then
					table.insert(leafKeywordNames, Util.trim(name))
				end
			end
		end

		scanScope:setCaption(
			LOC("$$$/LrGeniusAI/DeduplicateKeywords/SemanticScanCaption=Querying AI for semantic clusters...")
		)
		LrTasks.yield()

		-- Build provider options from model key selected in warning dialog
		local clusterOptions = {}
		if warnProps.modelKey and warnProps.modelKey ~= "" then
			local sep = string.find(warnProps.modelKey, "::", 1, true)
			if sep then
				local prov = string.sub(warnProps.modelKey, 1, sep - 1)
				local mdl = string.sub(warnProps.modelKey, sep + 2)
				clusterOptions.provider = prov
				clusterOptions.model = (mdl ~= "") and mdl or nil
				if prov == "chatgpt" and prefs.chatgptApiKey and prefs.chatgptApiKey ~= "" then
					clusterOptions.api_key = prefs.chatgptApiKey
				elseif prov == "gemini" and prefs.geminiApiKey and prefs.geminiApiKey ~= "" then
					clusterOptions.api_key = prefs.geminiApiKey
				elseif prov == "ollama" and prefs.ollamaBaseUrl and prefs.ollamaBaseUrl ~= "" then
					clusterOptions.ollama_base_url = prefs.ollamaBaseUrl
				elseif prov == "lmstudio" and prefs.lmstudioBaseUrl and prefs.lmstudioBaseUrl ~= "" then
					clusterOptions.lmstudio_base_url = prefs.lmstudioBaseUrl
				end
			end
		end

		local semanticPairs = {}
		local semanticWarning = nil

		local clusterResp, clusterErr = SearchIndexAPI.clusterKeywords(leafKeywordNames, nil, clusterOptions)
		if clusterResp and clusterResp.results then
			if clusterResp.warning and clusterResp.warning ~= "" then
				semanticWarning = clusterResp.warning
			end

			for _, cluster in ipairs(clusterResp.results) do
				-- LLM returns canonical name first; CLIP-only falls back to alphabetical
				if not clusterOptions.provider then
					table.sort(cluster, function(a, b)
						return a:lower() < b:lower()
					end)
				end
				local canonicalName = cluster[1]
				local canonicalKw = allNameMap[canonicalName:lower()]
				if canonicalKw then
					for j = 2, #cluster do
						local dupName = cluster[j]
						local dupKw = allNameMap[dupName:lower()]
						if dupKw and dupKw ~= canonicalKw then
							table.insert(semanticPairs, {
								canonical = canonicalKw,
								canonicalName = canonicalName,
								duplicate = dupKw,
								duplicateName = dupName,
							})
						end
					end
				end
			end
		elseif clusterErr then
			semanticWarning = LOC(
				"$$$/LrGeniusAI/DeduplicateKeywords/SemanticUnavailable=AI semantic clustering unavailable (CLIP model not loaded)."
			)
			log:warn("DeduplicateKeywords: semantic cluster call failed: " .. tostring(clusterErr))
		end

		scanScope:done()

		if #semanticPairs == 0 then
			local msg = LOC(
				"$$$/LrGeniusAI/DeduplicateKeywords/NoDuplicatesMessage=No similar leaf keywords were found in the selected branches. Your catalog is already clean."
			)
			if semanticWarning then
				msg = msg .. "\n\n" .. semanticWarning
			end
			LrDialogs.message(LOC("$$$/LrGeniusAI/DeduplicateKeywords/NoDuplicatesTitle=No Duplicates Found"), msg)
			return
		end

		-- ── Step 4: Preview with per-item checkboxes ──────────────────────
		local previewProps = LrBinding.makePropertyTable(context)
		previewProps.syncBackend = true

		for i = 1, #semanticPairs do
			previewProps["sel_sem_" .. i] = true
		end

		local function makeSelectButtons(prefix, count, props)
			return f:row({
				f:push_button({
					title = LOC("$$$/LrGeniusAI/MetadataManager/SelectAll=Select All"),
					action = function()
						for i = 1, count do
							props[prefix .. i] = true
						end
					end,
				}),
				f:push_button({
					title = LOC("$$$/LrGeniusAI/MetadataManager/DeselectAll=Deselect All"),
					action = function()
						for i = 1, count do
							props[prefix .. i] = false
						end
					end,
				}),
			})
		end

		local semRows = { spacing = 2 }
		for i, pair in ipairs(semanticPairs) do
			table.insert(
				semRows,
				f:row({
					f:checkbox({ value = bind("sel_sem_" .. i) }),
					f:static_text({
						title = '"' .. pair.duplicateName .. '"  →  "' .. pair.canonicalName .. '"',
						font = "<system>",
					}),
				})
			)
		end

		local semanticSection = f:group_box({
			bind_to_object = previewProps,
			title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/SemanticHeader=AI Suggestions (^1)", #semanticPairs),
			fill_horizontal = 1,
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/SemanticNote=These keywords are semantically similar according to the AI.\nUncheck any pair you want to keep separate."
				),
				fill_horizontal = 1,
				wrap = true,
			}),
			f:spacer({ height = 4 }),
			makeSelectButtons("sel_sem_", #semanticPairs, previewProps),
			f:scrolled_view({
				height = 240,
				width = 490,
				f:column(semRows),
			}),
		})

		local previewView = f:column({
			spacing = f:control_spacing(),
			width = 520,
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/PreviewHint=^1 duplicate(s) found. Photos will be re-tagged with the canonical keyword.\nThe duplicate entry remains empty in the catalog.",
					#semanticPairs
				),
				fill_horizontal = 1,
				wrap = true,
			}),
			semanticSection,
			f:spacer({ height = 4 }),
			f:row({
				bind_to_object = previewProps,
				f:checkbox({ value = bind("syncBackend") }),
				f:static_text({
					title = LOC(
						"$$$/LrGeniusAI/DeduplicateKeywords/SyncBackendLabel=Also update AI search index (recommended)"
					),
				}),
			}),
		})

		local previewResult = LrDialogs.presentModalDialog({
			title = LOC(
				"$$$/LrGeniusAI/DeduplicateKeywords/PreviewTitle=Preview: ^1 Duplicate(s) to Merge",
				#semanticPairs
			),
			contents = previewView,
			actionVerb = LOC("$$$/LrGeniusAI/DeduplicateKeywords/MergeSelected=Merge Selected"),
			cancelVerb = LOC("$$$/LrGeniusAI/common/Cancel=Cancel"),
		})
		if previewResult ~= "ok" then
			return
		end

		local finalPairs = {}
		for i, pair in ipairs(semanticPairs) do
			if previewProps["sel_sem_" .. i] then
				table.insert(finalPairs, pair)
			end
		end

		if #finalPairs == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/DeduplicateKeywords/NoSelectionTitle=Nothing Selected"),
				LOC("$$$/LrGeniusAI/DeduplicateKeywords/NoMergesSelected=No pairs were selected for merging.")
			)
			return
		end

		-- ── Step 5: Execute merges ─────────────────────────────────────────
		local mergeScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/MergeProgressTitle=Merging duplicate keywords..."),
			functionContext = context,
		})

		local mergedCount = 0
		local skippedNames = {}
		local successfulPairs = {}

		mergeScope:setPortionComplete(0, #finalPairs)

		for i, pair in ipairs(finalPairs) do
			if mergeScope:isCanceled() then
				break
			end

			mergeScope:setCaption(
				LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/MergingCaption=Merging ^1 of ^2: ^3",
					i,
					#finalPairs,
					pair.duplicateName
				)
			)
			mergeScope:setPortionComplete(i - 1, #finalPairs)
			LrTasks.yield()

			local ok, reason = executeMerge(catalog, pair)
			if ok then
				mergedCount = mergedCount + 1
				table.insert(successfulPairs, pair)
			else
				table.insert(skippedNames, reason)
			end

			mergeScope:setPortionComplete(i, #finalPairs)
		end

		-- Sync backend database if the user opted in
		local backendUpdated = nil
		if previewProps.syncBackend and #successfulPairs > 0 then
			mergeScope:setCaption(LOC("$$$/LrGeniusAI/DeduplicateKeywords/SyncingBackend=Updating AI search index..."))
			LrTasks.yield()
			local syncResp, syncErr = SearchIndexAPI.applyKeywordMerges(successfulPairs)
			if syncErr then
				log:warn("DeduplicateKeywords: backend sync failed: " .. tostring(syncErr))
			elseif syncResp then
				backendUpdated = syncResp.updated_photos
			end
		end

		mergeScope:done()

		-- ── Results ────────────────────────────────────────────────────────
		local resultMsg =
			LOC("$$$/LrGeniusAI/DeduplicateKeywords/ResultSuccess=^1 keyword(s) merged successfully.", mergedCount)
		if mergedCount > 0 then
			resultMsg = resultMsg
				.. "\n\n"
				.. LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/ResultPurgeHint=The duplicate keyword entries are now empty. To remove them from the keyword list, choose Metadata > Purge Unused Keywords in Lightroom."
				)
		end
		if #skippedNames > 0 then
			resultMsg = resultMsg
				.. "\n\n"
				.. LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/ResultSkipped=^1 keyword(s) could not be processed:\n^2",
					#skippedNames,
					table.concat(skippedNames, "\n")
				)
		end
		if backendUpdated ~= nil then
			resultMsg = resultMsg
				.. "\n\n"
				.. LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/ResultBackendSync=AI search index updated: ^1 photo(s) updated.",
					tostring(backendUpdated)
				)
		end

		LrDialogs.message(LOC("$$$/LrGeniusAI/DeduplicateKeywords/ResultTitle=Deduplication Complete"), resultMsg)

		log:info("DeduplicateKeywords complete: merged=" .. mergedCount .. " skipped=" .. #skippedNames)
	end)
end)
