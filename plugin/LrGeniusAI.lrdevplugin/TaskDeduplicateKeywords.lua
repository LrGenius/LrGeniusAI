-- TaskDeduplicateKeywords.lua
-- Finds duplicate keywords via two passes:
--   1. Exact: catalog keywords whose name matches a synonym of another keyword.
--   2. Semantic: CLIP-embedding clusters of semantically similar keyword names.
-- Both passes show per-item checkboxes so the user controls every merge.

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

-- Returns pairs {canonical, canonicalName, duplicate, duplicateName} where
-- 'duplicate' is a catalog keyword whose name matches a synonym of 'canonical'.
local function findExactPairs(selectedKeywords, allNameMap)
	local pairs = {}
	local scheduled = {}

	for _, kw in ipairs(selectedKeywords) do
		if not scheduled[kw] then
			local okSyn, synonyms = LrTasks.pcall(function()
				return kw:getSynonyms() or {}
			end)
			if okSyn and type(synonyms) == "table" then
				for _, syn in ipairs(synonyms) do
					if type(syn) == "string" then
						local key = string.lower(Util.trim(syn))
						if key ~= "" then
							local duplicate = allNameMap[key]
							if duplicate and duplicate ~= kw and not scheduled[duplicate] then
								local okDup, dupName = LrTasks.pcall(function()
									return duplicate:getName()
								end)
								local okCan, canName = LrTasks.pcall(function()
									return kw:getName()
								end)
								if okDup and okCan then
									scheduled[duplicate] = true
									table.insert(pairs, {
										canonical = kw,
										canonicalName = canName,
										duplicate = duplicate,
										duplicateName = dupName,
									})
								end
							end
						end
					end
				end
			end
		end
	end
	return pairs
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

		-- ── Step 1: Warning + backup confirmation ─────────────────────────
		local warnProps = LrBinding.makePropertyTable(context)
		warnProps.hasBackup = false

		local warnView = f:column({
			bind_to_object = warnProps,
			spacing = f:control_spacing(),
			width = 430,
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/WarningIntro=This tool finds keywords that exist as standalone catalog keywords but are also listed as synonyms of another keyword. Those duplicates are merged: photos are re-tagged with the canonical keyword and the standalone duplicate is deleted."
				),
				fill_horizontal = 1,
				wrap = true,
				height_in_lines = 3,
			}),
			f:separator({ fill_horizontal = 1 }),
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/WarningRisk=Warning: This permanently modifies your catalog. Deleted keywords cannot be recovered. Back up your catalog first (File > Catalog Settings > Back Up Catalog)."
				),
				fill_horizontal = 1,
				wrap = true,
				height_in_lines = 2,
				text_color = LrColor(0.8, 0.2, 0.0),
			}),
			f:spacer({ height = 6 }),
			f:checkbox({
				value = bind("hasBackup"),
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/BackupConfirm=I have a recent catalog backup and understand this operation cannot be undone."
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
					"$$$/LrGeniusAI/DeduplicateKeywords/SelectPathsHint=Select the top-level keyword branches to scan for synonym duplicates. The search for duplicates will cover the entire catalog."
				),
				fill_horizontal = 1,
				wrap = true,
				height_in_lines = 2,
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

		-- ── Step 3: Scan — exact + semantic ───────────────────────────────
		local scanScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/ScanProgressTitle=Scanning keyword catalog..."),
			functionContext = context,
		})
		scanScope:setCaption(LOC("$$$/LrGeniusAI/DeduplicateKeywords/ScanningCaption=Building keyword index..."))
		LrTasks.yield()

		local allCatalogKeywords = collectAllKeywords(topKeywords)
		local allNameMap = buildNameMap(allCatalogKeywords)
		local selectedKeywords = collectAllKeywords(selectedRoots)
		local exactPairs = findExactPairs(selectedKeywords, allNameMap)

		-- Collect all keyword names for the semantic pass
		local allKeywordNames = {}
		for _, kw in ipairs(allCatalogKeywords) do
			local okN, name = LrTasks.pcall(function()
				return kw:getName()
			end)
			if okN and type(name) == "string" and name ~= "" then
				table.insert(allKeywordNames, Util.trim(name))
			end
		end

		-- Semantic clustering via the CLIP backend
		scanScope:setCaption(
			LOC("$$$/LrGeniusAI/DeduplicateKeywords/SemanticScanCaption=Querying AI for semantic clusters...")
		)
		LrTasks.yield()

		local semanticPairs = {}
		local semanticWarning = nil

		local clusterResp, clusterErr = SearchIndexAPI.clusterKeywords(allKeywordNames, 0.88)
		if clusterResp and clusterResp.results then
			if clusterResp.warning and clusterResp.warning ~= "" then
				semanticWarning = clusterResp.warning
			end

			-- Build a set of names already scheduled by the exact pass (either side)
			local alreadyScheduled = {}
			for _, p in ipairs(exactPairs) do
				alreadyScheduled[p.duplicateName:lower()] = true
				alreadyScheduled[p.canonicalName:lower()] = true
			end

			for _, cluster in ipairs(clusterResp.results) do
				-- Sort alphabetically; first entry becomes the canonical (kept keyword)
				table.sort(cluster, function(a, b)
					return a:lower() < b:lower()
				end)
				local canonicalName = cluster[1]
				local canonicalKw = allNameMap[canonicalName:lower()]
				if canonicalKw then
					for j = 2, #cluster do
						local dupName = cluster[j]
						local dupKey = dupName:lower()
						if not alreadyScheduled[dupKey] then
							local dupKw = allNameMap[dupKey]
							if dupKw and dupKw ~= canonicalKw then
								alreadyScheduled[dupKey] = true
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
			end
		elseif clusterErr then
			semanticWarning = LOC(
				"$$$/LrGeniusAI/DeduplicateKeywords/SemanticUnavailable=AI semantic clustering unavailable (CLIP model not loaded)."
			)
			log:warn("DeduplicateKeywords: semantic cluster call failed: " .. tostring(clusterErr))
		end

		scanScope:done()

		if #exactPairs == 0 and #semanticPairs == 0 then
			local msg = LOC(
				"$$$/LrGeniusAI/DeduplicateKeywords/NoDuplicatesMessage=No synonym duplicates were found in the selected keyword branches. Your catalog is already clean."
			)
			if semanticWarning then
				msg = msg .. "\n\n" .. semanticWarning
			end
			LrDialogs.message(LOC("$$$/LrGeniusAI/DeduplicateKeywords/NoDuplicatesTitle=No Duplicates Found"), msg)
			return
		end

		-- ── Step 4: Preview with per-item checkboxes ──────────────────────
		local previewProps = LrBinding.makePropertyTable(context)

		for i = 1, #exactPairs do
			previewProps["sel_exact_" .. i] = true
		end
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

		-- Section 1: exact synonym pairs
		local exactSection
		if #exactPairs > 0 then
			local exactRows = { spacing = 2 }
			for i, pair in ipairs(exactPairs) do
				table.insert(
					exactRows,
					f:row({
						f:checkbox({ value = bind("sel_exact_" .. i) }),
						f:static_text({
							title = '"' .. pair.duplicateName .. '"  →  "' .. pair.canonicalName .. '"',
							font = "<system>",
						}),
					})
				)
			end
			exactSection = f:group_box({
				bind_to_object = previewProps,
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/ExactHeader=Exact Synonym Duplicates (^1)",
					#exactPairs
				),
				fill_horizontal = 1,
				makeSelectButtons("sel_exact_", #exactPairs, previewProps),
				f:scrolled_view({
					height = 180,
					width = 490,
					f:column(exactRows),
				}),
			})
		end

		-- Section 2: semantic pairs
		local semanticSection
		if #semanticPairs > 0 then
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
			semanticSection = f:group_box({
				bind_to_object = previewProps,
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/SemanticHeader=AI Semantic Suggestions (^1)",
					#semanticPairs
				),
				fill_horizontal = 1,
				f:static_text({
					title = LOC(
						"$$$/LrGeniusAI/DeduplicateKeywords/SemanticNote=These keywords are semantically similar according to the AI. Uncheck any pair you want to keep separate."
					),
					fill_horizontal = 1,
					wrap = true,
					height_in_lines = 2,
				}),
				f:spacer({ height = 4 }),
				makeSelectButtons("sel_sem_", #semanticPairs, previewProps),
				f:scrolled_view({
					height = 180,
					width = 490,
					f:column(semRows),
				}),
			})
		elseif semanticWarning then
			semanticSection = f:static_text({
				title = semanticWarning,
				fill_horizontal = 1,
				wrap = true,
				text_color = LrColor(0.5, 0.5, 0.5),
			})
		end

		-- Assemble preview column
		local previewChildren = {
			spacing = f:control_spacing(),
			width = 520,
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/PreviewHint=^1 duplicate keyword(s) found. Each will be merged into its canonical synonym. Photos will be re-tagged and the duplicate keyword deleted. Keywords with child keywords will be skipped.",
					#exactPairs + #semanticPairs
				),
				fill_horizontal = 1,
				wrap = true,
				height_in_lines = 3,
			}),
		}
		if exactSection then
			table.insert(previewChildren, exactSection)
		end
		if semanticSection then
			table.insert(previewChildren, semanticSection)
		end

		local previewView = f:column(previewChildren)

		local previewResult = LrDialogs.presentModalDialog({
			title = LOC(
				"$$$/LrGeniusAI/DeduplicateKeywords/PreviewTitle=Preview: ^1 Duplicate(s) to Merge",
				#exactPairs + #semanticPairs
			),
			contents = previewView,
			actionVerb = LOC("$$$/LrGeniusAI/DeduplicateKeywords/MergeSelected=Merge Selected"),
			cancelVerb = LOC("$$$/LrGeniusAI/common/Cancel=Cancel"),
		})
		if previewResult ~= "ok" then
			return
		end

		-- Build the final list from checked items
		local finalPairs = {}
		for i, pair in ipairs(exactPairs) do
			if previewProps["sel_exact_" .. i] then
				table.insert(finalPairs, pair)
			end
		end
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
			else
				table.insert(skippedNames, reason)
			end

			mergeScope:setPortionComplete(i, #finalPairs)
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

		LrDialogs.message(LOC("$$$/LrGeniusAI/DeduplicateKeywords/ResultTitle=Deduplication Complete"), resultMsg)

		log:info("DeduplicateKeywords complete: merged=" .. mergedCount .. " skipped=" .. #skippedNames)
	end)
end)
