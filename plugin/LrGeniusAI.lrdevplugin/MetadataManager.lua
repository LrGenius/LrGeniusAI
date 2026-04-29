-- MetadataManager.lua
-- Handles reading and writing metadata from/to the Lightroom catalog.

MetadataManager = {}
local createKeywordSafely
local findKeywordByNameInParent

-- Session cache bucket for nil parent (cannot use nil as table key).
local KEYWORD_CACHE_ROOT = {}

local function keywordCacheGet(sessionCache, parent, name)
	if not sessionCache or type(name) ~= "string" or name == "" then
		return nil
	end
	local bucket = parent and sessionCache[parent] or sessionCache[KEYWORD_CACHE_ROOT]
	return bucket and bucket[name]
end

local function keywordCachePut(sessionCache, parent, name, keywordObj)
	if not sessionCache or not keywordObj or type(name) ~= "string" or name == "" then
		return
	end
	local key = parent or KEYWORD_CACHE_ROOT
	if not sessionCache[key] then
		sessionCache[key] = {}
	end
	sessionCache[key][name] = keywordObj
end

---
-- Finds a keyword already on the photo with this name and parent (avoids LrKeyword:getChildren()
-- when the SDK hits a format bug there).
--
local function findKeywordOnPhotoForParent(photo, parent, targetName)
	if not photo or type(targetName) ~= "string" or targetName == "" then
		return nil
	end
	local ok, result = LrTasks.pcall(function()
		local raw = photo:getRawMetadata("keywords") or {}
		for _, kw in pairs(raw) do
			if kw and kw.getName and kw.getParent then
				local okN, n = LrTasks.pcall(function()
					return kw:getName()
				end)
				local okP, p = LrTasks.pcall(function()
					return kw:getParent()
				end)
				if okN and okP and n == targetName then
					if parent == nil and p == nil then
						return kw
					end
					if parent ~= nil and p == parent then
						return kw
					end
				end
			end
		end
		return nil
	end)
	if ok then
		return result
	end
	return nil
end

---
-- Applies the AI-generated metadata to the photo.
-- @param photo The LrPhoto object.
-- @param aiResponse The parsed JSON response from the AI.
-- @param validatedData The data from the review dialog, indicating what to save.
-- @param ai (AiModelAPI instance) The AI model API instance.
--
function MetadataManager.applyMetadata(photo, response, validatedData, options)
	log:trace("Applying metadata to photo: " .. photo:getFormattedMetadata("fileName"))
	local catalog = LrApplication.activeCatalog()
	options = options or {}

	local title = response.metadata.title
	local caption = response.metadata.caption
	local altText = response.metadata.alt_text
	local keywords = response.metadata.keywords

	local saveTitle = true
	local saveCaption = true
	local saveAltText = true
	local saveKeywords = true

	-- If review was done, use the validated data
	if validatedData then
		saveTitle = validatedData.saveTitle and options.applyTitle ~= false
		title = validatedData.title
		saveCaption = validatedData.saveCaption and options.applyCaption ~= false
		caption = validatedData.caption
		saveAltText = validatedData.saveAltText and options.applyAltText ~= false
		altText = validatedData.altText
		saveKeywords = validatedData.saveKeywords and options.applyKeywords ~= false
		keywords = validatedData.keywords
	end

	-- When appending, merge resolved values with existing catalog metadata
	if options.appendMetadata then
		local existingTitle = photo:getFormattedMetadata("title") or ""
		local existingCaption = photo:getFormattedMetadata("caption") or ""
		local existingAltText = photo:getFormattedMetadata("altTextAccessibility") or ""
		if existingTitle ~= "" and title and title ~= "" then
			title = existingTitle .. "\n\n" .. title
		end
		if existingCaption ~= "" and caption and caption ~= "" then
			caption = existingCaption .. "\n\n" .. caption
		end
		if existingAltText ~= "" and altText and altText ~= "" then
			altText = existingAltText .. "\n\n" .. altText
		end
	end

	log:trace("Response: " .. Util.dumpTable(response))
	log:trace("validatedData: " .. Util.dumpTable(validatedData))

	log:trace("Saving title, caption, altText, keywords to catalog")
	catalog:withWriteAccessDo(
		LOC("$$$/lrc-ai-assistant/AnalyzeImageTask/saveTitleCaption=Save AI generated title and caption"),
		function()
			if saveCaption and caption and caption ~= "" then
				photo:setRawMetadata("caption", caption)
			end
			if saveTitle and title and title ~= "" then
				photo:setRawMetadata("title", title)
			end
			if saveAltText and altText and altText ~= "" then
				photo:setRawMetadata("altTextAccessibility", altText)
			end
		end,
		Defaults.catalogWriteAccessOptions
	)

	-- Save keywords (sessionCache avoids LrKeyword:getChildren() when the SDK errors there)
	log:trace("Saving keywords to catalog")
	if saveKeywords and keywords ~= nil and type(keywords) == "table" and prefs.generateKeywords then
		local keywordSessionCache = {}

		-- Build alias-dedup index when alias mode is on. Scope follows the user's
		-- top-level-keyword preference so we don't merge into hand-curated branches.
		if options.generateAliases then
			local indexScope = nil
			if options.useTopLevelKeyword and options.topLevelKeyword and options.topLevelKeyword ~= "" then
				indexScope = findKeywordByNameInParent(nil, catalog, keywordSessionCache, nil, options.topLevelKeyword)
			end
			keywordSessionCache._aliasIndex = MetadataManager.buildAliasIndex(catalog, indexScope)
			local aliasIndexCount = 0
			for _ in pairs(keywordSessionCache._aliasIndex) do
				aliasIndexCount = aliasIndexCount + 1
			end
			log:trace("Alias index built with " .. tostring(aliasIndexCount) .. " entries")
		end

		local topKeyword = nil
		if prefs.useKeywordHierarchy and options.useTopLevelKeyword then
			catalog:withWriteAccessDo(
				"$$$/lrc-ai-assistant/AnalyzeImageTask/saveTopKeyword=Save AI generated keywords",
				function()
					topKeyword = createKeywordSafely(
						catalog,
						options.topLevelKeyword or "LrGeniusAI",
						{ Defaults.topLevelKeywordSynonym },
						false,
						nil,
						keywordSessionCache
					)
					if topKeyword then
						local okAdd, errAdd = LrTasks.pcall(function()
							photo:addKeyword(topKeyword) -- Add top-level keyword to photo. To see the number of tagged photos in keyword list (Gerald Uhl)
						end)
						if not okAdd then
							log:error("Failed to add top-level keyword to photo: " .. tostring(errAdd))
						end
					end
				end
			)
			-- Keep track of used top-level keywords
			if not Util.table_contains(prefs.knownTopLevelKeywords, options.topLevelKeyword) then
				table.insert(prefs.knownTopLevelKeywords, options.topLevelKeyword)
			end
		end
		local existingKeywordNames = nil
		local currentTopLevelKeyword = options.useTopLevelKeyword and (options.topLevelKeyword or "LrGeniusAI") or nil
		catalog:withWriteAccessDo(
			"$$$/lrc-ai-assistant/AnalyzeImageTask/saveTopKeyword=Save AI generated keywords",
			function()
				MetadataManager.addKeywordRecursively(
					photo,
					catalog,
					keywords,
					topKeyword,
					existingKeywordNames,
					currentTopLevelKeyword,
					keywordSessionCache
				)
			end,
			Defaults.catalogWriteAccessOptions
		)
	end

	if response.ai_model then
		catalog:withPrivateWriteAccessDo(function()
			log:trace("Saving AI model to catalog")
			photo:setPropertyForPlugin(_PLUGIN, "aiModel", tostring(response.ai_model))
			photo:setPropertyForPlugin(_PLUGIN, "aiLastRun", tostring(response.ai_rundate or ""))
		end, Defaults.catalogWriteAccessOptions)
	end
end

---
-- Returns an existing child keyword by name under the given parent.
-- If parent is nil, searches top-level keywords.
-- Uses session cache and photo keyword list before LrKeyword:getChildren(), which can error in some SDK/catalog cases.
-- @param photo LrPhoto|nil
-- @param catalog The active LrCatalog object.
-- @param sessionCache table|nil Optional cache parent -> name -> LrKeyword for this applyMetadata pass.
-- @param parent Optional parent LrKeyword object.
-- @param keywordName The keyword name to find.
-- @return LrKeyword|nil
findKeywordByNameInParent = function(photo, catalog, sessionCache, parent, keywordName)
	if not catalog or type(keywordName) ~= "string" then
		return nil
	end
	local target = Util.trim(keywordName)
	if target == "" then
		return nil
	end

	local cached = keywordCacheGet(sessionCache, parent, target)
	if cached then
		return cached
	end

	local onPhoto = findKeywordOnPhotoForParent(photo, parent, target)
	if onPhoto then
		keywordCachePut(sessionCache, parent, target, onPhoto)
		return onPhoto
	end

	-- Fetch children via pcall: SDK can throw (e.g. bad argument to 'format' inside getChildren).
	local fetchKey = parent or KEYWORD_CACHE_ROOT
	if sessionCache and sessionCache._keywordFetchFailed and sessionCache._keywordFetchFailed[fetchKey] then
		return nil
	end

	local okFetch, siblingsOrErr = LrTasks.pcall(function()
		if parent and parent.getChildren then
			return parent:getChildren()
		end
		return catalog:getKeywords()
	end)

	if not okFetch then
		local errStr = tostring(siblingsOrErr)
		if sessionCache then
			sessionCache._keywordFetchFailed = sessionCache._keywordFetchFailed or {}
			sessionCache._keywordFetchFailed[fetchKey] = true
			sessionCache._keywordFetchLogged = sessionCache._keywordFetchLogged or {}
			if not sessionCache._keywordFetchLogged[fetchKey] then
				sessionCache._keywordFetchLogged[fetchKey] = true
				log:trace(
					"findKeywordByNameInParent: getChildren/getKeywords failed (SDK bug), using createKeyword fallback: "
						.. errStr
				)
			end
		else
			log:trace(
				"findKeywordByNameInParent: getChildren/getKeywords failed (SDK bug), using createKeyword fallback: "
					.. errStr
			)
		end

		-- Robust fallback: use catalog:createKeyword with returnIfExists=true (acts as a finder)
		local okFallback, fallbackResult = LrTasks.pcall(function()
			return catalog:createKeyword(target, nil, nil, parent, true)
		end)
		if okFallback and fallbackResult then
			keywordCachePut(sessionCache, parent, target, fallbackResult)
			return fallbackResult
		elseif not okFallback then
			log:trace("findKeywordByNameInParent: createKeyword fallback also failed: " .. tostring(fallbackResult))
		end
		return nil
	end
	local siblings = siblingsOrErr

	if type(siblings) ~= "table" then
		return nil
	end

	local found = nil
	for _, sibling in pairs(siblings) do
		if sibling and type(sibling.getName) == "function" then
			local okName, nameOrErr = LrTasks.pcall(function()
				return sibling:getName()
			end)
			if okName and nameOrErr == target then
				found = sibling
				break
			end
		end
	end

	if found then
		keywordCachePut(sessionCache, parent, target, found)
	end
	return found
end

---
-- Sanitizes a synonym list to a flat array of non-empty strings.
-- @param synonyms table|nil
-- @return table
local function sanitizeSynonyms(synonyms)
	if type(synonyms) ~= "table" then
		return {}
	end
	local cleaned = {}
	for _, synonym in ipairs(synonyms) do
		if type(synonym) == "string" then
			local synonymText = Util.trim(synonym)
			if synonymText ~= "" then
				table.insert(cleaned, synonymText)
			end
		end
	end
	return cleaned
end

---
-- Additively merges `incomingSynonyms` into the LR synonym field of `keywordObj`.
-- Existing synonyms are preserved; entries equal to the keyword name or already
-- present (case-insensitive) are skipped. No-op when there is nothing to add.
local function mergeKeywordSynonyms(keywordObj, incomingSynonyms)
	if not keywordObj or type(incomingSynonyms) ~= "table" or #incomingSynonyms == 0 then
		return
	end
	if type(keywordObj.getSynonyms) ~= "function" or type(keywordObj.setAttributes) ~= "function" then
		return
	end

	local okName, keywordName = LrTasks.pcall(function()
		return keywordObj:getName() or ""
	end)
	if not okName then
		return
	end

	local okSyn, existing = LrTasks.pcall(function()
		return keywordObj:getSynonyms() or {}
	end)
	if not okSyn or type(existing) ~= "table" then
		return
	end

	local merged = {}
	local seen = { [string.lower(keywordName)] = true }
	for _, synonym in ipairs(existing) do
		if type(synonym) == "string" then
			local text = Util.trim(synonym)
			local key = string.lower(text)
			if text ~= "" and not seen[key] then
				seen[key] = true
				table.insert(merged, text)
			end
		end
	end

	local added = false
	for _, synonym in ipairs(incomingSynonyms) do
		if type(synonym) == "string" then
			local text = Util.trim(synonym)
			local key = string.lower(text)
			if text ~= "" and not seen[key] then
				seen[key] = true
				table.insert(merged, text)
				added = true
			end
		end
	end

	if not added then
		return
	end

	local ok, err = LrTasks.pcall(function()
		keywordObj:setAttributes({ synonyms = merged })
	end)
	if not ok then
		log:warn("Failed to merge synonyms for keyword '" .. tostring(keywordName) .. "': " .. tostring(err))
	end
end

---
-- Creates a Lightroom keyword safely and returns nil on failure.
-- @param catalog LrCatalog
-- @param keywordName string
-- @param synonyms table|nil
-- @param includeOnExport boolean
-- @param parent LrKeyword|nil
-- @return LrKeyword|nil
createKeywordSafely = function(catalog, keywordName, synonyms, includeOnExport, parent, sessionCache)
	if type(keywordName) ~= "string" then
		return nil
	end
	local cleanName = Util.trim(keywordName)
	if cleanName == "" then
		return nil
	end

	local cleanSynonyms = sanitizeSynonyms(synonyms)
	local ok, keywordOrErr = LrTasks.pcall(function()
		return catalog:createKeyword(cleanName, cleanSynonyms, includeOnExport, parent, true)
	end)
	if not ok then
		log:error("Failed to create keyword '" .. tostring(cleanName) .. "': " .. tostring(keywordOrErr))
		return nil
	end
	keywordCachePut(sessionCache, parent, cleanName, keywordOrErr)
	return keywordOrErr
end

---
-- Builds a flat lookup map of (lower-cased name | synonym) -> LrKeyword by walking
-- the catalog keyword tree once per analysis run. Used for alias-based de-duplication
-- so a newly generated keyword can be matched against an existing keyword that lists
-- it as a synonym.
-- @param catalog LrCatalog
-- @param scope LrKeyword|nil If provided, only keywords under this subtree are indexed.
-- @return table Flat map of lower-cased text to LrKeyword.
function MetadataManager.buildAliasIndex(catalog, scope)
	local index = {}
	if not catalog then
		return index
	end

	-- Index by keyword name only. We deliberately do NOT index by LR synonyms:
	-- past AI runs may have polluted that field with hypernyms / co-occurring terms,
	-- and indexing them would silently re-route fresh keywords into the wrong bucket.
	local function indexKeyword(kw)
		if not kw or type(kw.getName) ~= "function" then
			return
		end
		local okName, name = LrTasks.pcall(function()
			return kw:getName()
		end)
		if okName and type(name) == "string" then
			local key = string.lower(Util.trim(name))
			if key ~= "" and not index[key] then
				index[key] = kw
			end
		end
	end

	local function walk(keywords)
		if type(keywords) ~= "table" then
			return
		end
		for _, kw in ipairs(keywords) do
			indexKeyword(kw)
			if type(kw.getChildren) == "function" then
				local okChildren, children = LrTasks.pcall(function()
					return kw:getChildren() or {}
				end)
				if okChildren then
					walk(children)
				end
			end
		end
	end

	local roots
	if scope and type(scope.getChildren) == "function" then
		local ok, children = LrTasks.pcall(function()
			return scope:getChildren() or {}
		end)
		if ok then
			roots = children
		end
	else
		local ok, kws = LrTasks.pcall(function()
			return catalog:getKeywords() or {}
		end)
		if ok then
			roots = kws
		end
	end
	walk(roots)
	return index
end

---
-- Looks up a candidate keyword in the alias index by its name first and
-- then by each of its aliases. Returns the matched LrKeyword or nil.
local function findKeywordByAliases(aliasIndex, candidateName, candidateAliases)
	if type(aliasIndex) ~= "table" or type(candidateName) ~= "string" then
		return nil
	end
	local nameKey = string.lower(Util.trim(candidateName))
	if nameKey == "" then
		return nil
	end
	local hit = aliasIndex[nameKey]
	if hit then
		return hit
	end
	if type(candidateAliases) == "table" then
		for _, alias in ipairs(candidateAliases) do
			if type(alias) == "string" then
				local key = string.lower(Util.trim(alias))
				if key ~= "" then
					hit = aliasIndex[key]
					if hit then
						return hit
					end
				end
			end
		end
	end
	return nil
end

---
-- Recursively adds keywords to a photo, creating parent keywords as needed.
-- @param photo The LrPhoto object.
-- @param catalog The LrCatalog object.
-- @param keywordSubTable A table of keywords, possibly nested.
-- @param parent The parent LrKeyword object for the current level.
-- @param existingKeywordNames Optional set of keyword names already on the photo (append mode).
-- @param currentTopLevelKeyword Optional top-level keyword for this task (avoids prefs race in parallel jobs).
-- @param sessionCache Optional table: parent -> keyword name -> LrKeyword (same pass as applyMetadata).
--
function MetadataManager.addKeywordRecursively(
	photo,
	catalog,
	keywordSubTable,
	parent,
	existingKeywordNames,
	currentTopLevelKeyword,
	sessionCache
)
	local function trimmedStringList(rawList)
		if type(rawList) ~= "table" then
			return {}
		end
		local cleaned = {}
		local seen = {}
		for _, entry in ipairs(rawList) do
			if type(entry) == "string" then
				local text = Util.trim(entry)
				local lowered = string.lower(text)
				if text ~= "" and not seen[lowered] then
					table.insert(cleaned, text)
					seen[lowered] = true
				end
			end
		end
		return cleaned
	end

	local function parseKeywordLeaf(leafValue)
		if type(leafValue) == "string" then
			local keywordName = Util.trim(leafValue)
			return keywordName, {}, {}, {}
		end
		if type(leafValue) == "table" and type(leafValue.name) == "string" then
			local keywordName = Util.trim(leafValue.name)
			local nameLower = string.lower(keywordName)

			local synonyms = trimmedStringList(leafValue.synonyms)
			-- Drop translations colliding with the primary name.
			local filteredSynonyms = {}
			for _, s in ipairs(synonyms) do
				if string.lower(s) ~= nameLower then
					table.insert(filteredSynonyms, s)
				end
			end

			local aliases = trimmedStringList(leafValue.aliases)
			local filteredAliases = {}
			for _, a in ipairs(aliases) do
				if string.lower(a) ~= nameLower then
					table.insert(filteredAliases, a)
				end
			end

			-- synonym_aliases must not collide with the primary name nor any translation.
			local synonymAliases = trimmedStringList(leafValue.synonym_aliases)
			local translationLowers = { [nameLower] = true }
			for _, s in ipairs(filteredSynonyms) do
				translationLowers[string.lower(s)] = true
			end
			local filteredSynonymAliases = {}
			for _, sa in ipairs(synonymAliases) do
				if not translationLowers[string.lower(sa)] then
					table.insert(filteredSynonymAliases, sa)
				end
			end

			return keywordName, filteredSynonyms, filteredAliases, filteredSynonymAliases
		end
		return nil, {}, {}, {}
	end

	local function isKeywordLeafObject(value)
		return type(value) == "table" and type(value.name) == "string"
	end

	-- Resolve a keyword by alias-index (if available), then by name within the parent,
	-- otherwise create it. Same-language `aliases` are kept only in the in-memory alias
	-- index for run-scoped dedup; they are NOT persisted to LR's synonym field, since
	-- LLMs unreliably distinguish true synonyms from hypernyms/co-occurring concepts
	-- and polluted synonyms cascade into the dedup tool's exact-match pass.
	-- Bilingual translations (passed via `lrSynonyms`) DO land in the LR synonym field
	-- so cross-language search works; existing keywords get an additive merge.
	local aliasIndex = sessionCache and sessionCache._aliasIndex or nil
	local function resolveAndAttachKeyword(candidateName, candidateAliases, currentParent, lrSynonyms)
		if type(candidateName) ~= "string" or candidateName == "" then
			return nil
		end

		lrSynonyms = sanitizeSynonyms(lrSynonyms)
		local nameLower = string.lower(Util.trim(candidateName))
		local filteredLrSynonyms = {}
		local lrSynSeen = { [nameLower] = true }
		for _, syn in ipairs(lrSynonyms) do
			local key = string.lower(syn)
			if not lrSynSeen[key] then
				lrSynSeen[key] = true
				table.insert(filteredLrSynonyms, syn)
			end
		end

		local resolved = findKeywordByAliases(aliasIndex, candidateName, candidateAliases)
		if not resolved then
			resolved = findKeywordByNameInParent(photo, catalog, sessionCache, currentParent, candidateName)
		end
		if resolved then
			mergeKeywordSynonyms(resolved, filteredLrSynonyms)
		else
			resolved =
				createKeywordSafely(catalog, candidateName, filteredLrSynonyms, true, currentParent, sessionCache)
		end

		-- Register the new keyword (and its aliases / bilingual synonyms) in the alias
		-- index so the next candidate in the same run can dedupe against it.
		if resolved and aliasIndex then
			if nameLower ~= "" and not aliasIndex[nameLower] then
				aliasIndex[nameLower] = resolved
			end
			local function indexAll(list)
				if type(list) ~= "table" then
					return
				end
				for _, entry in ipairs(list) do
					if type(entry) == "string" then
						local key = string.lower(Util.trim(entry))
						if key ~= "" and not aliasIndex[key] then
							aliasIndex[key] = resolved
						end
					end
				end
			end
			indexAll(candidateAliases)
			indexAll(filteredLrSynonyms)
		end

		if resolved then
			local okAdd, errAdd = LrTasks.pcall(function()
				photo:addKeyword(resolved)
			end)
			if not okAdd then
				log:error("Failed to add keyword '" .. tostring(candidateName) .. "' to photo: " .. tostring(errAdd))
				return nil
			end
		end
		return resolved
	end

	local addKeywords = {}
	local reservedTopLevel = currentTopLevelKeyword or prefs.topLevelKeyword
	for key, value in pairs(keywordSubTable) do
		local keyword
		if type(key) == "string" and key ~= "" and key ~= "None" and key ~= "none" and prefs.useKeywordHierarchy then
			keyword = createKeywordSafely(catalog, key, {}, false, parent, sessionCache)
		elseif type(key) == "number" and value then
			local keywordName, keywordSynonyms, keywordAliases, keywordSynonymAliases = parseKeywordLeaf(value)
			if keywordName and keywordName ~= "" and keywordName ~= "None" and keywordName ~= "none" then
				if not Util.table_contains(addKeywords, keywordName) then
					if
						keywordName == "Ollama"
						or keywordName == "LMStudio"
						or keywordName == "Google Gemini"
						or keywordName == "ChatGPT"
						or keywordName == reservedTopLevel
					then
						log:trace("Skipping keyword: " .. tostring(keywordName) .. " as it is reserved.")
					else
						local currentParent = prefs.useKeywordHierarchy and parent or nil

						-- Bilingual translations + their same-language aliases all land in the
						-- LR synonym field of the primary keyword. The primary's own aliases
						-- (`keywordAliases`, same language as the primary) are kept out of LR
						-- synonyms — they only feed the run-scoped alias index for dedup.
						local lrSynonyms = {}
						for _, t in ipairs(keywordSynonyms) do
							table.insert(lrSynonyms, t)
						end
						for _, sa in ipairs(keywordSynonymAliases) do
							table.insert(lrSynonyms, sa)
						end

						local primary = resolveAndAttachKeyword(keywordName, keywordAliases, currentParent, lrSynonyms)
						if primary then
							table.insert(addKeywords, keywordName)
							-- Use the primary as the parent for nested categories below.
							keyword = primary
						end
					end
				end
			end
		end
		if type(value) == "table" and not isKeywordLeafObject(value) then
			MetadataManager.addKeywordRecursively(
				photo,
				catalog,
				value,
				keyword,
				existingKeywordNames,
				currentTopLevelKeyword,
				sessionCache
			)
		end
	end
end

function MetadataManager.showValidationDialog(ctx, photo, response)
	local f = LrView.osFactory()
	local bind = LrView.bind

	local title = response.metadata.title
	local caption = response.metadata.caption
	local altText = response.metadata.alt_text
	local keywords = response.metadata.keywords

	local propertyTable = LrBinding.makePropertyTable(ctx)
	propertyTable.skipFromHere = false

	local kwVal, kwMeta, orderedIds = Util.extractAllKeywords(keywords or {})
	propertyTable.keywordsMeta = kwMeta

	-- Initialize flat properties with full paths for bindings
	for _, id in ipairs(orderedIds) do
		local fullPath = kwVal[id] or ""
		local prefix = kwMeta[id].path
		if prefix and prefix ~= "" then
			fullPath = prefix .. " > " .. fullPath
		end
		propertyTable["keywordsSel_" .. id] = true
		propertyTable["keywordsVal_" .. id] = fullPath
	end

	propertyTable.title = title or ""
	propertyTable.caption = caption or ""
	propertyTable.altText = altText or ""

	propertyTable.saveKeywords = keywords ~= nil and type(keywords) == "table"
	-- By default, save if data is present, regardless of pre-flight options
	propertyTable.saveTitle = title ~= nil and title ~= ""
	propertyTable.saveCaption = caption ~= nil and caption ~= ""
	propertyTable.saveAltText = altText ~= nil and altText ~= ""

	local keywordRows = {
		spacing = 2,
	}

	for _, id in ipairs(orderedIds) do
		table.insert(
			keywordRows,
			f:row({
				f:checkbox({
					value = bind("keywordsSel_" .. id),
					visible = bind("saveKeywords"),
				}),
				f:edit_field({
					value = bind("keywordsVal_" .. id),
					width_in_chars = 45, -- Enough for long paths
					immediate = true,
					enabled = bind("saveKeywords"),
				}),
			})
		)
	end

	local dialogView = f:row({
		bind_to_object = propertyTable,
		spacing = 20,
		f:column({
			width = 250,
			f:static_text({
				title = photo:getFormattedMetadata("fileName"),
				font = "<system/bold>",
				wrap = true,
				width = 250,
			}),
			f:catalog_photo({
				photo = photo,
				width = 250,
				height = 250,
			}),
			f:spacer({ height = 10 }),
			f:checkbox({
				value = bind("skipFromHere"),
				title = LOC("$$$/LrGeniusAI/MetadataManager/SkipRemaining=Save following without reviewing."),
			}),
		}),
		f:column({
			f:group_box({
				title = LOC("$$$/LrGeniusAI/Keywords=Keywords"),
				fill_horizontal = 1,
				f:row({
					f:push_button({
						title = LOC("$$$/LrGeniusAI/MetadataManager/SelectAll=Select All"),
						action = function()
							for _, id in ipairs(orderedIds) do
								propertyTable["keywordsSel_" .. id] = true
							end
						end,
					}),
					f:push_button({
						title = LOC("$$$/LrGeniusAI/MetadataManager/DeselectAll=Deselect All"),
						action = function()
							for _, id in ipairs(orderedIds) do
								propertyTable["keywordsSel_" .. id] = false
							end
						end,
					}),
					f:spacer({ fill_horizontal = 1 }),
					f:checkbox({
						value = bind("saveKeywords"),
						title = LOC("$$$/lrc-ai-assistant/AnalyzeImageTask/SaveKeywords=Save keywords"),
					}),
				}),
				f:scrolled_view({
					height = 250,
					width = 560,
					f:column(keywordRows),
				}),
			}),
			f:group_box({
				title = LOC("$$$/LrGeniusAI/Metadata=Metadata"),
				fill_horizontal = 1,
				f:row({
					f:checkbox({
						value = bind("saveTitle"),
						title = LOC("$$$/lrc-ai-assistant/AnalyzeImageTask/SaveTitle=Save title"),
					}),
					f:edit_field({
						value = bind("title"),
						fill_horizontal = 1,
						height_in_lines = 1,
						enabled = bind("saveTitle"),
					}),
				}),
				f:row({
					f:checkbox({
						value = bind("saveCaption"),
						title = LOC("$$$/lrc-ai-assistant/AnalyzeImageTask/SaveCaption=Save caption"),
					}),
					f:edit_field({
						value = bind("caption"),
						fill_horizontal = 1,
						height_in_lines = 5,
						enabled = bind("saveCaption"),
					}),
				}),
				f:row({
					f:checkbox({
						value = bind("saveAltText"),
						title = LOC("$$$/lrc-ai-assistant/AnalyzeImageTask/SaveAltText=Save alt text"),
					}),
					f:edit_field({
						value = bind("altText"),
						fill_horizontal = 1,
						height_in_lines = 3,
						enabled = bind("saveAltText"),
					}),
				}),
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/lrc-ai-assistant/AnalyzeImageTask/ReviewWindowTitle=Review results")
			.. (photo and (": " .. photo:getFormattedMetadata("fileName")) or ""),
		otherVerb = LOC("$$$/lrc-ai-assistant/AnalyzeImageTask/discard=Discard"),
		contents = dialogView,
	})

	local results = {}
	local validatedKeywords = {}
	if propertyTable.saveKeywords then
		-- Construct a new hierarchical table from the full path strings
		local pathsWithMeta = {}
		for _, id in ipairs(orderedIds) do
			if propertyTable["keywordsSel_" .. id] then
				local meta = kwMeta[id] or {}
				table.insert(pathsWithMeta, {
					path = propertyTable["keywordsVal_" .. id],
					synonyms = meta.synonyms or {},
					aliases = meta.aliases or {},
					synonymAliases = meta.synonymAliases or {},
				})
			end
		end
		validatedKeywords = Util.buildHierarchyFromPaths(pathsWithMeta)
	end

	results.keywords = validatedKeywords
	results.saveKeywords = propertyTable.saveKeywords
	results.title = propertyTable.title
	results.saveTitle = propertyTable.saveTitle
	results.caption = propertyTable.caption
	results.saveCaption = propertyTable.saveCaption
	results.altText = propertyTable.altText
	results.saveAltText = propertyTable.saveAltText
	results.skipFromHere = propertyTable.skipFromHere

	return result, results
end

---
-- Collects up to `limit` unique keyword names from the full catalog keyword tree.
-- Returns a flat sorted list of strings suitable for sending to the backend as
-- catalog vocabulary context.
-- @param catalog LrCatalog
-- @param limit number Max names to return (default 300)
-- @return table Flat list of keyword name strings
function MetadataManager.collectCatalogKeywordNames(catalog, limit)
	limit = limit or 300
	local names = {}
	local seen = {}
	local count = 0

	local function walk(keywords)
		if count >= limit or type(keywords) ~= "table" then
			return
		end
		for _, kw in ipairs(keywords) do
			if count >= limit then
				break
			end
			local okName, name = LrTasks.pcall(function()
				return kw:getName()
			end)
			if okName and type(name) == "string" then
				local key = string.lower(Util.trim(name))
				if key ~= "" and not seen[key] then
					seen[key] = true
					table.insert(names, Util.trim(name))
					count = count + 1
				end
			end
			if type(kw.getChildren) == "function" then
				local okCh, children = LrTasks.pcall(function()
					return kw:getChildren() or {}
				end)
				if okCh then
					walk(children)
				end
			end
		end
	end

	local okTop, topKeywords = LrTasks.pcall(function()
		return catalog:getKeywords() or {}
	end)
	if okTop then
		walk(topKeywords)
	end
	table.sort(names, function(a, b)
		return a:lower() < b:lower()
	end)
	return names
end

---
-- Get the keyword hierarchy from the Lightroom catalog.
-- Only keywords with children will be returned.
-- @return A table representing the keyword hierarchy.
function MetadataManager.getCatalogKeywordHierarchy()
	local catalog = LrApplication.activeCatalog()
	local topKeywords = catalog:getKeywords()
	local hierarchy = {}

	local function traverseKeywords(keywords, parentHierarchy)
		for _, keyword in ipairs(keywords) do
			-- if not Util.table_contains(prefs.knownTopLevelKeywords, keyword) and not Util.table_contains(keyword:getSynonyms(), Defaults.topLevelKeywordSynonym) then
			local children = keyword:getChildren()
			if #children > 0 then
				local keywordEntry = {}
				parentHierarchy[keyword:getName()] = keywordEntry
				traverseKeywords(children, keywordEntry)
			end
			-- end
		end
	end

	traverseKeywords(topKeywords, hierarchy)

	-- log:trace("Keyword hierarchy: " .. Util.dumpTable(hierarchy))
	return hierarchy
end

---
-- Get the keyword hierarchy for a specific photo.
-- Returns a multidimensional table containing all the photo's keywords organized under their parent keywords.
-- Leaf keywords (last level) are stored as strings in a numeric array.
-- @param photo The LrPhoto object.
-- @return A table representing the keyword hierarchy for this photo.
function MetadataManager.getPhotoKeywordHierarchy(photo)
	local keywords = photo:getRawMetadata("keywords")
	if not keywords or #keywords == 0 then
		return {}
	end

	local hierarchy = {}
	local processedKeywords = {}

	-- Helper function to build the path from keyword to root
	local function getKeywordPath(keyword)
		local path = {}
		local current = keyword
		while current do
			if not Util.table_contains(prefs.knownTopLevelKeywords, current) then
				table.insert(path, 1, current)
			end
			current = current:getParent()
		end
		return path
	end

	-- Helper function to insert a keyword into the hierarchy following its path
	local function insertKeywordIntoHierarchy(path)
		local currentLevel = hierarchy
		for i, keyword in ipairs(path) do
			local keywordName = keyword:getName()

			if i == #path then
				-- Last level: add keyword name as string in numeric array
				if currentLevel[keywordName] == nil then
					currentLevel[keywordName] = {}
				end
				-- Only add if it doesn't already exist in the array
				local alreadyExists = false
				for _, existingKeyword in ipairs(currentLevel) do
					if existingKeyword == keywordName then
						alreadyExists = true
						break
					end
				end
				if not alreadyExists then
					table.insert(currentLevel, keywordName)
				end
			else
				-- Intermediate level: create nested table
				if currentLevel[keywordName] == nil then
					currentLevel[keywordName] = {}
				end
				currentLevel = currentLevel[keywordName]
			end
		end
	end

	-- Process each keyword and build the hierarchy
	for _, keyword in ipairs(keywords) do
		local keywordName = keyword:getName()

		-- Only process each keyword once
		if not processedKeywords[keywordName] then
			processedKeywords[keywordName] = true
			local path = getKeywordPath(keyword)
			insertKeywordIntoHierarchy(path)
		end
	end

	-- log:trace("Photo keyword hierarchy: " .. Util.dumpTable(hierarchy))
	return hierarchy
end
