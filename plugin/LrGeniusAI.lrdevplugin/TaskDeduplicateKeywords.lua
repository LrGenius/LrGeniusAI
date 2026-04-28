-- TaskDeduplicateKeywords.lua
-- Finds keywords that exist as standalone catalog keywords but also appear as
-- synonyms of another keyword, then merges them: photos are re-tagged with the
-- canonical keyword and the duplicate keyword is deleted.

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
-- 'allNameMap' covers the entire catalog so duplicates outside the selected scope
-- are also detected.
local function findDedupPairs(selectedKeywords, allNameMap)
	local pairs = {}
	local scheduled = {} -- duplicate kw -> true

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

		-- ── Step 3: Scan ──────────────────────────────────────────────────
		local scanScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/ScanProgressTitle=Scanning keyword catalog..."),
			functionContext = context,
		})
		scanScope:setCaption(LOC("$$$/LrGeniusAI/DeduplicateKeywords/ScanningCaption=Building keyword index..."))
		LrTasks.yield()

		local allCatalogKeywords = collectAllKeywords(topKeywords)
		local allNameMap = buildNameMap(allCatalogKeywords)
		local selectedKeywords = collectAllKeywords(selectedRoots)
		local dedupPairs = findDedupPairs(selectedKeywords, allNameMap)

		scanScope:done()

		if #dedupPairs == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/DeduplicateKeywords/NoDuplicatesTitle=No Duplicates Found"),
				LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/NoDuplicatesMessage=No synonym duplicates were found in the selected keyword branches. Your catalog is already clean."
				)
			)
			return
		end

		-- ── Step 4: Preview + confirm ──────────────────────────────────────
		local previewRows = { spacing = 2 }
		for _, pair in ipairs(dedupPairs) do
			table.insert(
				previewRows,
				f:static_text({
					title = '"' .. pair.duplicateName .. '"  →  "' .. pair.canonicalName .. '"',
					font = "<system>",
				})
			)
		end

		local previewView = f:column({
			spacing = f:control_spacing(),
			width = 520,
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/PreviewHint=^1 duplicate keyword(s) found. Each will be merged into its canonical synonym. Photos will be re-tagged and the duplicate keyword deleted. Keywords with child keywords will be skipped.",
					#dedupPairs
				),
				fill_horizontal = 1,
				wrap = true,
				height_in_lines = 3,
			}),
			f:spacer({ height = 4 }),
			f:static_text({
				title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/PreviewHeader=Duplicate  →  Canonical"),
				font = "<system/bold>",
			}),
			f:scrolled_view({
				height = 260,
				width = 500,
				f:column(previewRows),
			}),
		})

		local previewResult = LrDialogs.presentModalDialog({
			title = LOC(
				"$$$/LrGeniusAI/DeduplicateKeywords/PreviewTitle=Preview: ^1 Duplicate(s) to Merge",
				#dedupPairs
			),
			contents = previewView,
			actionVerb = LOC("$$$/LrGeniusAI/DeduplicateKeywords/MergeNow=Merge Now"),
			cancelVerb = LOC("$$$/LrGeniusAI/common/Cancel=Cancel"),
		})
		if previewResult ~= "ok" then
			return
		end

		-- ── Step 5: Execute merges ─────────────────────────────────────────
		local mergeScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/DeduplicateKeywords/MergeProgressTitle=Merging duplicate keywords..."),
			functionContext = context,
		})

		local mergedCount = 0
		local skippedNames = {}

		mergeScope:setPortionComplete(0, #dedupPairs)

		for i, pair in ipairs(dedupPairs) do
			if mergeScope:isCanceled() then
				break
			end

			mergeScope:setCaption(
				LOC(
					"$$$/LrGeniusAI/DeduplicateKeywords/MergingCaption=Merging ^1 of ^2: ^3",
					i,
					#dedupPairs,
					pair.duplicateName
				)
			)
			mergeScope:setPortionComplete(i - 1, #dedupPairs)
			LrTasks.yield()

			-- Keywords with children cannot be safely deleted
			local okChildren, children = LrTasks.pcall(function()
				return pair.duplicate:getChildren() or {}
			end)
			if not okChildren or (type(children) == "table" and #children > 0) then
				log:warn(
					"DeduplicateKeywords: Skipping '" .. pair.duplicateName .. "' — has child keywords, cannot delete"
				)
				table.insert(skippedNames, pair.duplicateName .. " (has children)")
			else
				local okPhotos, photos = LrTasks.pcall(function()
					return pair.duplicate:getPhotos() or {}
				end)
				if not okPhotos then
					log:error(
						"DeduplicateKeywords: getPhotos failed for '" .. pair.duplicateName .. "': " .. tostring(photos)
					)
					table.insert(skippedNames, pair.duplicateName .. " (error reading photos)")
				else
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
								catalog:deleteKeyword(pair.duplicate)
							end,
							Defaults.catalogWriteAccessOptions
						)
					end)
					if ok then
						mergedCount = mergedCount + 1
						log:info(
							"DeduplicateKeywords: Merged '"
								.. pair.duplicateName
								.. "' → '"
								.. pair.canonicalName
								.. "' ("
								.. #photos
								.. " photo(s) re-tagged)"
						)
					else
						log:error(
							"DeduplicateKeywords: merge failed for '" .. pair.duplicateName .. "': " .. tostring(err)
						)
						table.insert(skippedNames, pair.duplicateName .. " (merge failed)")
					end
				end
			end

			mergeScope:setPortionComplete(i, #dedupPairs)
		end

		mergeScope:done()

		-- ── Results ────────────────────────────────────────────────────────
		local resultMsg =
			LOC("$$$/LrGeniusAI/DeduplicateKeywords/ResultSuccess=^1 keyword(s) merged successfully.", mergedCount)
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
