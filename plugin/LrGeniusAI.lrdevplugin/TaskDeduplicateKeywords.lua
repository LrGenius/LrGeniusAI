-- TaskDeduplicateKeywords.lua
-- SigLIP-based keyword clustering + optional LLM validation.
-- Only leaf keywords (no children) are candidates for deduplication.

-- How many keywords the tree walk visits between progress callbacks.
local KEYWORDS_PER_YIELD = 200

-- Walk keyword tree top-down and group the direct leaf children of each parent.
-- Only groups with ≥ 2 leaves are added (clustering requires at least 2 items).
-- Clustering within a parent prevents cross-category false positives.
-- @param roots table Array of top-level LrKeyword objects to walk
-- @param onProgress function|nil Called every KEYWORDS_PER_YIELD keywords with
--        (keywordsExamined, groupsFound); must yield so the UI can repaint.
--        A large catalog spends a long time in here, and without the callback
--        the walk blocks Lightroom outright — the progress dialog never paints.
-- @param isCanceled function|nil Polled at the same interval; a true return
--        aborts the walk and yields whatever has been collected so far
local function collectLeafGroups(roots, onProgress, isCanceled)
	local groups = {}
	local examined = 0
	local canceled = false

	local function tick()
		examined = examined + 1
		if examined % KEYWORDS_PER_YIELD ~= 0 then
			return
		end
		if onProgress then
			onProgress(examined, #groups)
		end
		if isCanceled and isCanceled() then
			canceled = true
		end
	end

	local function walk(parent, parentName)
		if canceled then
			return
		end
		local okC, children = LrTasks.pcall(function()
			return parent:getChildren() or {}
		end)
		if not okC or type(children) ~= "table" or #children == 0 then
			return
		end
		local directLeaves = {}
		for _, child in ipairs(children) do
			if canceled then
				break
			end
			tick()
			local okCC, grandchildren = LrTasks.pcall(function()
				return child:getChildren() or {}
			end)
			if okCC and type(grandchildren) == "table" and #grandchildren == 0 then
				local okN, name = LrTasks.pcall(function()
					return child:getName()
				end)
				if okN and type(name) == "string" and name ~= "" then
					table.insert(directLeaves, { name = Util.trim(name), kw = child })
				end
			else
				local okN, name = LrTasks.pcall(function()
					return child:getName()
				end)
				walk(child, (okN and name) and name or "?")
			end
		end
		if #directLeaves >= 2 then
			table.insert(groups, { parentName = parentName, leaves = directLeaves })
		end
	end

	for _, root in ipairs(roots) do
		if canceled then
			break
		end
		local okN, name = LrTasks.pcall(function()
			return root:getName()
		end)
		walk(root, (okN and name) and name or "?")
	end
	return groups, examined, canceled
end

-- Turn a backend progress report from /keywords/cluster/status into a phrase
-- for the progress caption.
local function describeClusterStage(progress)
	local stage = progress and progress.stage
	if stage == "embedding" then
		return LOC("$$$/LrGeniusAI/DeduplicateKeywords/StageEmbedding=comparing keyword meanings")
	elseif stage == "clustering" then
		return LOC("$$$/LrGeniusAI/DeduplicateKeywords/StageClustering=grouping similar keywords")
	elseif stage == "llm" then
		local done = tonumber(progress.done)
		local total = tonumber(progress.total)
		if done and total and total > 0 then
			return LOC(
				"$$$/LrGeniusAI/DeduplicateKeywords/StageLlmN=AI reviewing suggestions (batch ^1/^2)",
				math.min(done + 1, total),
				total
			)
		end
		return LOC("$$$/LrGeniusAI/DeduplicateKeywords/StageLlm=AI reviewing suggestions")
	end
	return LOC("$$$/LrGeniusAI/DeduplicateKeywords/StageStarting=waiting for the AI backend")
end

-- Photos re-tagged per write-access block. A keyword can hold thousands of
-- photos, and doing them in one block means one long stretch with no repaint
-- and no way to see that anything is happening. Chunking lets us yield and
-- report between blocks; the cost is one undo step per chunk instead of one
-- per merge, which is a fair trade for a merge that would otherwise look hung.
local PHOTOS_PER_WRITE_CHUNK = 200

-- Executes a single keyword merge: re-tags photos with the canonical keyword
-- and removes them from the duplicate. The duplicate keyword entry itself is
-- left in the catalog (the Lightroom SDK has no deleteKeyword API); it will
-- appear with 0 photos and can be removed via Metadata > Purge Unused Keywords.
-- @param catalog table The active LrCatalog
-- @param pair table { canonical, canonicalName, duplicate, duplicateName }
-- @param onProgress function|nil Called with (photosDone, photosTotal) before
--        each chunk so the caller can update its progress scope and yield
-- Returns true on success, or nil + reason string on failure/skip.
local function executeMerge(catalog, pair, onProgress)
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

	local total = #photos
	local ok, err = LrTasks.pcall(function()
		local firstIndex = 1
		while firstIndex <= total do
			local lastIndex = math.min(firstIndex + PHOTOS_PER_WRITE_CHUNK - 1, total)
			if onProgress then
				onProgress(firstIndex - 1, total)
			end
			catalog:withWriteAccessDo(
				"Deduplicate keyword: " .. pair.duplicateName .. " → " .. pair.canonicalName,
				function()
					for i = firstIndex, lastIndex do
						local photo = photos[i]
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
			-- Yield outside the write-access block: the gate must not be held
			-- across a yield, but between chunks the UI is free to repaint.
			LrTasks.yield()
			firstIndex = lastIndex + 1
		end
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
		warnProps.threshold = prefs.deduplicateThreshold or 0.85

		local warnView = f:column({
			bind_to_object = warnProps,
			spacing = f:control_spacing(),
			width = 450,
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/WarningIntro=This tool uses AI to find duplicate keywords that mean the same thing and suggests merging them.\nPhotos are re-tagged with the kept keyword. The duplicate entries\nremain in the catalog with 0 photos — remove them via\nMetadata > Purge Unused Keywords."
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
						width = 120,
					}),
					f:popup_menu({
						value = bind("modelKey"),
						items = modelItems,
						width = 290,
					}),
				}),
				f:spacer({ height = 6 }),
				f:row({
					f:static_text({
						title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/ThresholdLabel=Matching strictness:"),
						width = 120,
					}),
					f:slider({
						value = bind("threshold"),
						min = 0.70,
						max = 0.98,
						width = 200,
					}),
					f:static_text({
						title = bind({
							key = "threshold",
							transform = function(v, fromModel)
								if fromModel then
									return string.format("%.2f", v or 0.85)
								end
								return tonumber(v) or 0.85
							end,
						}),
						width = 35,
						alignment = "right",
					}),
				}),
				f:static_text({
					title = LOC(
						"$$$/LrGeniusAI/DeduplicateKeywords/ThresholdHint=Lower: more suggestions (may include false positives) — Higher: fewer, more precise matches"
					),
					fill_horizontal = 1,
					wrap = true,
					font = "<system/small>",
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

		-- Save preferences for next run
		prefs.deduplicateModelKey = warnProps.modelKey
		prefs.deduplicateThreshold = warnProps.threshold

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

		-- ── Step 3: Scan — AI semantic clustering per parent keyword ──────────
		local scanScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/ScanProgressTitle=Scanning keyword catalog..."),
			functionContext = context,
		})
		scanScope:setCaption(LOC("$$$/LrGeniusAI/DeduplicateKeywords/ScanningCaption=Building keyword index..."))
		LrTasks.yield()

		-- Group leaf keywords by their direct parent so clustering stays within
		-- each category — prevents cross-category false positives like place names
		-- merging into unrelated descriptors.
		local leafGroups, keywordsExamined, walkCanceled = collectLeafGroups(selectedRoots, function(examined, found)
			scanScope:setCaption(
				LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/ScanningCaptionN=Reading keywords: ^1 examined, ^2 group(s) found",
					examined,
					found
				)
			)
			LrTasks.yield()
		end, function()
			return scanScope:isCanceled()
		end)
		log:info(
			"DeduplicateKeywords: examined "
				.. keywordsExamined
				.. " keyword(s), "
				.. #leafGroups
				.. " group(s) to cluster"
				.. (walkCanceled and " (canceled)" or "")
		)
		if walkCanceled then
			scanScope:done()
			return
		end

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
		local scanAborted = false

		for gi, group in ipairs(leafGroups) do
			if scanScope:isCanceled() then
				break
			end
			scanScope:setCaption(
				LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/SemanticScanCaptionN=Querying AI: ^1 (^2/^3)",
					group.parentName,
					gi,
					#leafGroups
				)
			)
			scanScope:setPortionComplete(gi - 1, #leafGroups)
			LrTasks.yield()

			local names = {}
			local nameMap = {}
			for _, leaf in ipairs(group.leaves) do
				table.insert(names, leaf.name)
				nameMap[leaf.name:lower()] = leaf.kw
			end

			-- Refresh the caption on every poll. A single group can occupy the
			-- backend for minutes (LLM validation runs in chunks), so without
			-- this the dialog freezes on one line and looks dead.
			local clusterResp, clusterErr = SearchIndexAPI.clusterKeywords(
				names,
				warnProps.threshold,
				clusterOptions,
				scanScope,
				function(progress)
					scanScope:setCaption(
						LOC(
							"$$$/LrGeniusAI/DeduplicateKeywords/SemanticScanCaptionProgress=Querying AI: ^1 (^2/^3) — ^4 [^5]",
							group.parentName,
							gi,
							#leafGroups,
							describeClusterStage(progress),
							Util.formatElapsedTime(progress.elapsed)
						)
					)
					LrTasks.yield()
				end
			)
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
					local canonicalKw = nameMap[canonicalName:lower()]
					if canonicalKw then
						for j = 2, #cluster do
							local dupName = cluster[j]
							local dupKw = nameMap[dupName:lower()]
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
				local errText = tostring(clusterErr)
				log:warn("DeduplicateKeywords: cluster call failed for '" .. group.parentName .. "': " .. errText)
				if errText == "canceled" then
					scanAborted = true
					break
				elseif errText == "clustering stalled" or errText == "clustering timed out" then
					-- Whatever wedged the backend on this branch will wedge it on
					-- the next one too. Stop and say so, rather than making the
					-- user sit through one long timeout per remaining branch.
					semanticWarning = LOC(
						'$$$/LrGeniusAI/DeduplicateKeywords/SemanticTimedOut=The AI backend stopped responding while analyzing "^1", so the scan was stopped after ^2 of ^3 branch(es). A local AI model can need a very long time for a large keyword branch — try scanning fewer branches at once, or pick a faster model.',
						group.parentName,
						gi,
						#leafGroups
					)
					scanAborted = true
					break
				else
					semanticWarning = LOC(
						'$$$/LrGeniusAI/DeduplicateKeywords/SemanticFailed=AI clustering failed for "^1": ^2',
						group.parentName,
						errText
					)
				end
			end

			scanScope:setPortionComplete(gi, #leafGroups)
		end

		scanScope:done()
		log:info(
			"DeduplicateKeywords: scan finished with "
				.. #semanticPairs
				.. " suggestion(s)"
				.. (scanAborted and " (scan stopped early)" or "")
		)

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
			-- A partial scan still produces usable suggestions, but the user has to
			-- be told the list is incomplete before they act on it.
			semanticWarning and f:static_text({
				title = semanticWarning,
				fill_horizontal = 1,
				wrap = true,
				text_color = LrColor(0.5, 0.35, 0.0),
			}) or f:spacer({ height = 0 }),
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

			-- Re-tagging a keyword that sits on thousands of photos is the other
			-- place this task can look stalled; show the photo count as it goes.
			local ok, reason = executeMerge(catalog, pair, function(photosDone, photosTotal)
				if photosTotal > PHOTOS_PER_WRITE_CHUNK then
					mergeScope:setCaption(
						LOC(
							"$$$/LrGeniusAI/DeduplicateKeywords/MergingCaptionPhotos=Merging ^1 of ^2: ^3 (^4/^5 photos)",
							i,
							#finalPairs,
							pair.duplicateName,
							photosDone,
							photosTotal
						)
					)
				end
			end)
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
				backendUpdated = false -- flag: sync failed
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
		if backendUpdated == false then
			resultMsg = resultMsg
				.. "\n\n"
				.. LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/ResultBackendSyncFailed=Warning: The AI search index could not be updated. Your Lightroom catalog was merged successfully, but semantic search may show outdated results. Re-run indexing to fix this."
				)
		elseif backendUpdated ~= nil then
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
