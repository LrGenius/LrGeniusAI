-- MetadataManager.lua
-- Handles reading and writing metadata from/to the Lightroom catalog.

MetadataManager = {}
local createKeywordSafely
local findKeywordByNameInParent
local mergeKeywordSynonyms

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
-- Appends generated text below what the field already holds, unless it is
-- already there.
--
-- Without the containment check, a delta run stacks duplicates: when a photo is
-- not regenerated, the backend returns the *stored* description — the same text
-- that was written to the catalog last time — and appending that to itself
-- doubles the field on every run.
--
-- Exposed for the headless tests in `plugin/spec`; not part of the module's
-- contract for callers running inside Lightroom.
--
-- @param existing string|nil The value currently in the catalog field.
-- @param incoming string|nil The newly resolved value.
-- @return string|nil The value to write, or `incoming` unchanged when there is
--         nothing to append to.
--
function MetadataManager.appendText(existing, incoming)
	existing = existing or ""
	if existing == "" or incoming == nil or incoming == "" then
		return incoming
	end
	local existingTrimmed = LrStringUtils.trimWhitespace(existing)
	local incomingTrimmed = LrStringUtils.trimWhitespace(incoming)
	if incomingTrimmed == "" or existingTrimmed:find(incomingTrimmed, 1, true) then
		return existing
	end
	return existing .. "\n\n" .. incoming
end

---
-- Applies the AI-generated metadata to the photo.
-- @param photo The LrPhoto object.
-- @param aiResponse The parsed JSON response from the AI.
-- @param validatedData The data from the review dialog, indicating what to save.
-- @param options table|nil Apply flags. `options.keywordSessionCache` may hold a table
--        shared across all photos of a run so the keyword cache and the alias index are
--        built once rather than per photo; pass a fresh table per run, not per photo.
--
function MetadataManager.applyMetadata(photo, response, validatedData, options)
	log:trace("Applying metadata to photo: " .. photo:getFormattedMetadata("fileName"))
	local catalog = LrApplication.activeCatalog()
	options = options or {}

	local title = response.metadata.title
	local caption = response.metadata.caption
	local altText = response.metadata.alt_text
	local keywords = response.metadata.keywords

	-- The apply flags used to be consulted only inside the `validatedData`
	-- branch below, so without the review dialog — the normal case, see
	-- `TaskRetrieveMetadata.lua` — every field was written regardless of what
	-- the user had unchecked. They belong in the initial value; the review
	-- dialog then narrows further, it never widens.
	local saveTitle = options.applyTitle ~= false
	local saveCaption = options.applyCaption ~= false
	local saveAltText = options.applyAltText ~= false
	local saveKeywords = options.applyKeywords ~= false

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
		title = MetadataManager.appendText(photo:getFormattedMetadata("title"), title)
		caption = MetadataManager.appendText(photo:getFormattedMetadata("caption"), caption)
		altText = MetadataManager.appendText(photo:getFormattedMetadata("altTextAccessibility"), altText)
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
	-- Gated on `options.applyKeywords` (folded into `saveKeywords` above), not
	-- on `prefs.generateKeywords`. That preference is written by the Analyze &
	-- Index dialog alone, so unchecking keywords there used to silently
	-- suppress them in "Retrieve Metadata from Backend" as well, whatever that
	-- dialog's own checkbox said.
	if saveKeywords and keywords ~= nil and type(keywords) == "table" then
		-- Callers processing a batch should pass `options.keywordSessionCache` so the
		-- keyword lookup cache and the alias index are built once for the whole run
		-- instead of once per photo (both are O(catalog keywords) to construct).
		local keywordSessionCache = options.keywordSessionCache or {}

		-- Build alias-dedup index when alias mode is on. Scope follows the user's
		-- top-level-keyword preference so we don't merge into hand-curated branches.
		-- Skipped when a shared cache already carries an index from an earlier photo.
		if options.generateAliases and not keywordSessionCache._aliasIndex then
			local indexScope = nil
			if options.useTopLevelKeyword and options.topLevelKeyword and options.topLevelKeyword ~= "" then
				indexScope = findKeywordByNameInParent(nil, catalog, keywordSessionCache, nil, options.topLevelKeyword)
			end
			keywordSessionCache._aliasIndex = MetadataManager.buildAliasIndex(catalog, indexScope)
			local nameCount, synonymCount = 0, 0
			for _ in pairs(keywordSessionCache._aliasIndex.byName) do
				nameCount = nameCount + 1
			end
			for _ in pairs(keywordSessionCache._aliasIndex.bySynonym) do
				synonymCount = synonymCount + 1
			end
			log:trace(
				"Alias index built with " .. tostring(nameCount) .. " names, " .. tostring(synonymCount) .. " synonyms"
			)
		end

		-- The top-level keyword is the container for every generated keyword and is
		-- independent of the hierarchy setting: hierarchy only controls whether the LLM
		-- groups keywords into categories underneath it. With hierarchy off the keywords
		-- become one flat level of children here rather than moving to catalog root.
		local topKeyword = nil
		if options.useTopLevelKeyword then
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
					-- createKeyword ignores its synonyms argument when returnExisting=true
					-- matches a keyword that already exists, so a top-level keyword created
					-- before this marker existed (or by hand) would never receive it.
					mergeKeywordSynonyms(topKeyword, { Defaults.topLevelKeywordSynonym })
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
			if
				options.topLevelKeyword
				and not Util.table_contains(prefs.knownTopLevelKeywords, options.topLevelKeyword)
			then
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
-- Must be called inside a catalog write-access gate (uses LrKeyword:setAttributes).
mergeKeywordSynonyms = function(keywordObj, incomingSynonyms)
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
-- Builds a two-tier lookup index by walking the catalog keyword tree once per
-- analysis run. Used for alias-based de-duplication so a newly generated keyword
-- can be matched against an existing keyword that already covers it.
--
--   byName    : lower(keyword name) -> LrKeyword   — authoritative
--   bySynonym : lower(LR synonym)   -> LrKeyword | false  — fallback only
--
-- Keyword names always win. LR synonyms are a strict fallback because that field
-- can hold junk from older plugin versions (hypernyms / co-occurring terms), and
-- matching on it too eagerly would re-route fresh keywords into the wrong bucket.
-- A synonym claimed by two different keywords is stored as `false` and never
-- matches, which removes the ambiguous cases cheaply.
-- @param catalog LrCatalog
-- @param scope LrKeyword|nil If provided, only keywords under this subtree are indexed.
-- @return table { byName = {...}, bySynonym = {...} }
function MetadataManager.buildAliasIndex(catalog, scope)
	local index = { byName = {}, bySynonym = {} }
	if not catalog then
		return index
	end

	local function indexKeyword(kw)
		if not kw or type(kw.getName) ~= "function" then
			return
		end
		local okName, name = LrTasks.pcall(function()
			return kw:getName()
		end)
		local nameKey
		if okName and type(name) == "string" then
			nameKey = string.lower(Util.trim(name))
			if nameKey ~= "" and not index.byName[nameKey] then
				index.byName[nameKey] = kw
			end
		end

		if type(kw.getSynonyms) ~= "function" then
			return
		end
		local okSyn, synonyms = LrTasks.pcall(function()
			return kw:getSynonyms() or {}
		end)
		if not okSyn or type(synonyms) ~= "table" then
			return
		end
		for _, synonym in ipairs(synonyms) do
			if type(synonym) == "string" then
				local key = string.lower(Util.trim(synonym))
				if key ~= "" and key ~= nameKey then
					local existing = index.bySynonym[key]
					if existing == nil then
						index.bySynonym[key] = kw
					elseif existing ~= false and existing ~= kw then
						-- Two keywords claim this synonym — too ambiguous to merge on.
						index.bySynonym[key] = false
					end
				end
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
-- Looks up a candidate keyword in the two-tier alias index. Every term (the
-- candidate name, then each alias) is tried against keyword names first; only
-- when all of them miss do we fall back to the LR synonym tier. That ordering
-- means an existing keyword name always beats a synonym claim.
-- @return LrKeyword|nil
local function findKeywordByAliases(aliasIndex, candidateName, candidateAliases)
	if type(aliasIndex) ~= "table" or type(candidateName) ~= "string" then
		return nil
	end
	local nameKey = string.lower(Util.trim(candidateName))
	if nameKey == "" then
		return nil
	end

	local terms = { nameKey }
	if type(candidateAliases) == "table" then
		for _, alias in ipairs(candidateAliases) do
			if type(alias) == "string" then
				local key = string.lower(Util.trim(alias))
				if key ~= "" then
					table.insert(terms, key)
				end
			end
		end
	end

	for _, tier in ipairs({ aliasIndex.byName, aliasIndex.bySynonym }) do
		if type(tier) == "table" then
			for _, term in ipairs(terms) do
				local hit = tier[term]
				if hit then -- `false` (ambiguous synonym) is skipped here
					return hit
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
	-- otherwise create it.
	--
	-- De-clutter and bilingual keywords interact here, and de-clutter wins: exactly one
	-- keyword ends up on the photo, chosen by the alias index. Bilingual then makes sure
	-- nothing becomes unfindable — every alternate term for that concept is merged into
	-- the winner's LR synonym field:
	--   * bilingual translations and their `synonym_aliases` (passed via `lrSynonyms`)
	--   * the candidate's own name, when de-clutter routed it to a differently-named
	--     keyword ("Automobile" -> existing "Car" writes "Automobile" as a synonym of
	--     "Car", so searching either term still finds the photo)
	-- Same-language `aliases` stay out of the synonym field: LLMs unreliably distinguish
	-- true synonyms from hypernyms/co-occurring concepts, so they only feed the in-memory
	-- index for run-scoped dedup.
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

		-- lower-cased name of the keyword that actually won, or nil if unreadable
		local resolvedKey
		if resolved then
			local okName, resolvedName = LrTasks.pcall(function()
				return resolved:getName() or ""
			end)
			if okName then
				resolvedKey = string.lower(Util.trim(resolvedName))
			end
			-- De-clutter routed the generated name to a differently-named keyword: keep
			-- the generated name searchable by recording it as a synonym of the winner.
			if resolvedKey and resolvedKey ~= nameLower then
				table.insert(filteredLrSynonyms, Util.trim(candidateName))
			end
			mergeKeywordSynonyms(resolved, filteredLrSynonyms)
		else
			resolved =
				createKeywordSafely(catalog, candidateName, filteredLrSynonyms, true, currentParent, sessionCache)
			if resolved then
				resolvedKey = nameLower
			end
		end

		-- Register the winner in the alias index so later candidates in this run dedupe
		-- against it. Only an actual keyword name goes into the authoritative name tier;
		-- aliases, translations and de-cluttered-away names go into the fallback tier.
		if resolved and aliasIndex then
			if resolvedKey and resolvedKey ~= "" and not aliasIndex.byName[resolvedKey] then
				aliasIndex.byName[resolvedKey] = resolved
			end
			local function indexAsSynonym(list)
				if type(list) ~= "table" then
					return
				end
				for _, entry in ipairs(list) do
					if type(entry) == "string" then
						local key = string.lower(Util.trim(entry))
						-- `== nil`, not falsy: an existing `false` marks a synonym claimed by
						-- two catalog keywords, and that guard must not be cleared here.
						if key ~= "" and key ~= resolvedKey and aliasIndex.bySynonym[key] == nil then
							aliasIndex.bySynonym[key] = resolved
						end
					end
				end
			end
			indexAsSynonym({ candidateName })
			indexAsSynonym(candidateAliases)
			indexAsSynonym(filteredLrSynonyms)
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
						-- `parent` is already the correct target in both modes: the enclosing
						-- category with hierarchy on, the top-level keyword (or nil) with
						-- hierarchy off — the recursion below keeps it pinned there.
						local currentParent = parent

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
			-- Hierarchy off: never deepen the tree. Keep handing the same parent down so a
			-- nested response still lands as one flat level under the top-level keyword.
			local childParent = prefs.useKeywordHierarchy and keyword or parent
			MetadataManager.addKeywordRecursively(
				photo,
				catalog,
				value,
				childParent,
				existingKeywordNames,
				currentTopLevelKeyword,
				sessionCache
			)
		end
	end
end

function MetadataManager.showValidationDialog(ctx, photo, response, options)
	local f = LrView.osFactory()
	local bind = LrView.bind

	local title = response.metadata.title
	local caption = response.metadata.caption
	local altText = response.metadata.alt_text
	local keywords = response.metadata.keywords

	local propertyTable = LrBinding.makePropertyTable(ctx)
	propertyTable.skipFromHere = false

	-- ── Keyword extraction ────────────────────────────────────────────────
	-- The generated keywords as-is. De-clutter (keyword reuse) happens later, at
	-- apply time in addKeywordRecursively, so there is nothing to preview here.
	local kwVal, kwMeta, orderedIds = Util.extractAllKeywords(keywords or {})

	-- ── Species ──────────────────────────────────────────────────────────
	-- Shown read-only, unlike everything else here: a taxonomic identification
	-- comes from a classifier over a fixed vocabulary, so there is nothing
	-- sensible to reword. There *is* something to decide, though — before #327
	-- the species keywords and links were written whichever button the user
	-- pressed, so a discarded photo kept half its results.
	local speciesSummary = MetadataManager.speciesSummary(response.species)

	-- ── Property table initialisation ────────────────────────────────────
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
	propertyTable.saveTitle = title ~= nil and title ~= ""
	propertyTable.saveCaption = caption ~= nil and caption ~= ""
	propertyTable.saveAltText = altText ~= nil and altText ~= ""
	propertyTable.saveSpecies = speciesSummary ~= nil

	-- ── Keyword rows ──────────────────────────────────────────────────────
	local keywordRows = { spacing = 2 }

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
					width_in_chars = 45,
					immediate = true,
					enabled = bind("saveKeywords"),
				}),
			})
		)
	end

	-- ── Right panel contents ────────────────────────────────────────────
	local rightColumn = {
		f:group_box({
			title = LOC("$$$/LrGeniusAI/Keywords=Keywords"),
			fill_horizontal = 1,
			f:row({
				f:spacer({ fill_horizontal = 1 }),
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
	}

	-- Appended rather than written into the table above, because most photos
	-- have no organism in them and an empty "Species" box on every one of
	-- them would be worse than no box at all.
	if speciesSummary then
		local speciesLines = {
			f:static_text({ title = speciesSummary.name, font = "<system/bold>", width = 500 }),
			f:static_text({ title = speciesSummary.rank, width = 500 }),
		}
		if speciesSummary.taxonomy ~= "" then
			table.insert(speciesLines, f:static_text({ title = speciesSummary.taxonomy, width = 500 }))
		end
		table.insert(
			rightColumn,
			f:group_box({
				title = "Species",
				fill_horizontal = 1,
				f:row({
					f:checkbox({
						value = bind("saveSpecies"),
						title = "Save species",
					}),
					f:column(speciesLines),
				}),
			})
		)
	end

	-- ── Dialog layout ─────────────────────────────────────────────────────
	local dialogView = f:row({
		bind_to_object = propertyTable,
		spacing = 20,

		-- Left panel: photo thumbnail + skip checkbox
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

		-- Right panel: keywords, metadata, and species when there is one.
		f:column(rightColumn),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/lrc-ai-assistant/AnalyzeImageTask/ReviewWindowTitle=Review results")
			.. (photo and (": " .. photo:getFormattedMetadata("fileName")) or ""),
		otherVerb = LOC("$$$/lrc-ai-assistant/AnalyzeImageTask/discard=Discard"),
		contents = dialogView,
	})

	-- ── Result extraction ─────────────────────────────────────────────────
	local results = {}
	local validatedKeywords = {}
	if propertyTable.saveKeywords then
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
	-- False both when the user unticked it and when there was no
	-- identification to offer, so a caller can simply ask "should I write the
	-- species?" without re-deriving whether the box was even shown.
	results.saveSpecies = propertyTable.saveSpecies
	results.skipFromHere = propertyTable.skipFromHere

	return result, results
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

-- Photo counts per keyword, keyed by catalog path. Counting means one catalog
-- query per keyword, so it is done once per Lightroom session. Staleness is
-- wanted here: keywords the plugin itself creates mid-run must not change the
-- vocabulary sent with the next photo, or every request invalidates the
-- backend's cached prompt prefix.
local keywordUsageCache = {}

---
-- Counts how many photos carry each keyword in the catalog.
--
-- @param catalog The LrCatalog to walk.
-- @return table Array of { name = string, count = number }, unused keywords omitted.
--
local function collectKeywordUsage(catalog)
	local entries = {}
	local visited = 0

	local function traverse(keywords)
		for _, keyword in ipairs(keywords) do
			local okName, name = LrTasks.pcall(function()
				return keyword:getName()
			end)
			if okName and type(name) == "string" and name ~= "" then
				local okPhotos, photos = LrTasks.pcall(function()
					return keyword:getPhotos()
				end)
				local count = (okPhotos and type(photos) == "table") and #photos or 0
				-- An unused keyword is not vocabulary — it is usually a container
				-- or a leftover, and the cap is better spent on live terms.
				if count > 0 then
					table.insert(entries, { name = name, count = count })
				end
			end

			visited = visited + 1
			if visited % 200 == 0 and LrTasks.canYield() then
				LrTasks.yield()
			end

			-- getChildren() is known to throw on some catalogs (see
			-- findKeywordOnPhotoForParent); a failure just prunes that subtree.
			local okChildren, children = LrTasks.pcall(function()
				return keyword:getChildren()
			end)
			if okChildren and type(children) == "table" and #children > 0 then
				traverse(children)
			end
		end
	end

	local okTop, topKeywords = LrTasks.pcall(function()
		return catalog:getKeywords()
	end)
	if okTop and type(topKeywords) == "table" then
		traverse(topKeywords)
	end
	return entries
end

---
-- Collect the catalog keyword vocabulary to send to the LLM as `catalog_keywords`.
--
-- Selected by usage frequency, with the category names merged in and pinned so
-- they always survive the cap — see Util.rankKeywordsByUsage.
--
-- Must be called from an async task (it reads the catalog).
--
-- @param limit number|nil Maximum keywords to return (default Defaults.catalogKeywordLimit).
-- @param categories table|nil The keyword_categories being sent with the same request.
-- @return table|nil Array of keyword names, or nil when the catalog has none.
--
function MetadataManager.collectCatalogKeywordNames(limit, categories)
	limit = tonumber(limit) or Defaults.catalogKeywordLimit

	local catalog = LrApplication.activeCatalog()
	local cacheKey = catalog:getPath() or "activeCatalog"
	local entries = keywordUsageCache[cacheKey]
	if not entries then
		local started = LrDate.currentTime()
		entries = collectKeywordUsage(catalog)
		keywordUsageCache[cacheKey] = entries
		log:info(
			string.format(
				"Collected usage counts for %d catalog keywords in %.1fs",
				#entries,
				LrDate.currentTime() - started
			)
		)
	end

	local names = Util.rankKeywordsByUsage(entries, limit, {
		pinned = Util.flattenKeywordCategoryNames(categories),
	})
	if #names == 0 then
		return nil
	end
	log:trace("Sending " .. #names .. " catalog keywords as LLM vocabulary")
	return names
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

---
-- One-line-per-fact description of a species identification, for the review
-- dialog.
--
-- Pure string work, so it is testable without a catalog (see
-- `plugin/spec/metadata_manager_spec.lua`). Returning nil when nothing was
-- identified is what keeps the section out of the dialog entirely, rather
-- than showing an empty box on every photo that has no organism in it.
--
-- @param species table|nil Backend block: rank, taxonomy, scientific_name,
--   common_name, confidence.
-- @return table|nil `{ name = string, rank = string, taxonomy = string }`.
function MetadataManager.speciesSummary(species)
	if type(species) ~= "table" then
		return nil
	end
	local rank = species.rank
	if type(rank) ~= "string" or rank == "" or rank == "none" then
		return nil
	end
	local scientific = type(species.scientific_name) == "string" and Util.trim(species.scientific_name) or ""
	local common = type(species.common_name) == "string" and Util.trim(species.common_name) or ""
	local name
	if common ~= "" and scientific ~= "" then
		name = common .. " (" .. scientific .. ")"
	elseif common ~= "" then
		name = common
	elseif scientific ~= "" then
		name = scientific
	else
		-- A rank with neither name behind it is not something to show a user.
		return nil
	end

	local rankLine = rank
	local confidence = tonumber(species.confidence)
	if confidence then
		rankLine = rankLine .. string.format(", %d%% confidence", math.floor(confidence * 100 + 0.5))
	end

	local taxonomy = type(species.taxonomy) == "string" and Util.trim(species.taxonomy) or ""
	return { name = name, rank = rankLine, taxonomy = taxonomy }
end

---
-- Turns a backend species block into the ordered chain of keyword names to
-- create, coarsest first.
--
-- Split out from `MetadataManager.applySpecies` because it is the only part
-- with real logic and the only part testable without a live catalog (see
-- `plugin/spec/metadata_manager_spec.lua`).
--
-- The backend reports the deepest rank it is confident about, so the chain is
-- naturally short for an uncertain call ("Animalia > Arthropoda > Insecta")
-- and full-depth for a confident one. The leaf carries the human-readable name
-- when there is one, with the scientific name as an LR synonym — that way the
-- keyword list reads "Great Tit" rather than "major", and a search for
-- "Parus major" still finds it.
--
-- @param species table|nil Backend block: taxonomy, scientific_name, common_name, rank.
-- @return table|nil Array of `{ name = <string>, synonyms = <table> }`, or nil when
--         there is nothing identified.
function MetadataManager.speciesKeywordChain(species)
	if type(species) ~= "table" then
		return nil
	end
	local rank = species.rank
	if rank == nil or rank == "" or rank == "none" then
		return nil
	end
	local taxonomy = species.taxonomy
	if type(taxonomy) ~= "string" or taxonomy == "" then
		return nil
	end

	local parts = {}
	for part in string.gmatch(taxonomy, "([^>]+)") do
		local trimmed = Util.trim(part)
		if trimmed ~= "" then
			table.insert(parts, trimmed)
		end
	end
	if #parts == 0 then
		return nil
	end

	local chain = {}
	for i, part in ipairs(parts) do
		local isLeaf = i == #parts
		local name = part
		local synonyms = {}
		if isLeaf then
			-- At species rank the taxonomy's last element is the bare epithet
			-- ("major"), which is meaningless on its own — the binomial and the
			-- common name both live in their own fields.
			local scientific = species.scientific_name
			if type(scientific) == "string" and scientific ~= "" then
				name = scientific
			end
			local common = species.common_name
			if type(common) == "string" and common ~= "" then
				table.insert(synonyms, name)
				name = common
			end
		end
		table.insert(chain, { name = name, synonyms = synonyms })
	end
	return chain
end

---
-- Writes a species identification to the catalog.
--
-- Two independent outputs, because they answer different needs:
--   * the plugin metadata fields, always — searchable, filterable, and
--     contained inside the catalog;
--   * a keyword branch under `Defaults.defaultSpeciesKeyword`, only when
--     `options.applySpeciesKeywords` — portable, exports with the file.
--
-- The keyword branch deliberately does not go through
-- `addKeywordRecursively`: that path runs every leaf through the alias index
-- so LLM output can be de-cluttered onto existing keywords. Taxonomic names
-- are canonical and must not be re-routed onto whatever they happen to
-- resemble.
--
-- @param photo LrPhoto
-- @param species table|nil Backend block from `/get`'s metadata.
-- @param options table|nil `applySpeciesKeywords`, `keywordSessionCache`,
--   `skipLinks`.
function MetadataManager.applySpecies(photo, species, options)
	if type(species) ~= "table" then
		return
	end
	options = options or {}
	local catalog = LrApplication.activeCatalog()

	-- Resolved before the write gate, never inside it: this can touch the
	-- backend, and holding the catalog's private write access across a network
	-- round trip would block every other write in Lightroom for its duration.
	local links = nil
	if not options.skipLinks then
		links = SpeciesLinks.forSpecies(species)
	end

	-- Written unconditionally, empty values included: a re-run that downgrades
	-- a confident species to "none" has to clear the old answer, not leave an
	-- identification the backend no longer stands behind.
	catalog:withPrivateWriteAccessDo(function()
		photo:setPropertyForPlugin(_PLUGIN, "speciesRank", tostring(species.rank or "none"))
		photo:setPropertyForPlugin(_PLUGIN, "speciesTaxonomy", tostring(species.taxonomy or ""))
		photo:setPropertyForPlugin(_PLUGIN, "speciesScientificName", tostring(species.scientific_name or ""))
		photo:setPropertyForPlugin(_PLUGIN, "speciesCommonName", tostring(species.common_name or ""))
		-- Stored as a string like every other numeric AI field (see the cull*
		-- fields) because the metadata schema declares them all as strings.
		local confidence = tonumber(species.confidence)
		photo:setPropertyForPlugin(_PLUGIN, "speciesConfidence", confidence and string.format("%.2f", confidence) or "")
		-- Cleared along with everything else when there is no identification,
		-- so the panel never offers a link to the previous answer's species.
		photo:setPropertyForPlugin(_PLUGIN, "speciesInatUrl", links and links.inaturalist or "")
		photo:setPropertyForPlugin(_PLUGIN, "speciesWikipediaUrl", links and links.wikipedia or "")
	end, Defaults.catalogWriteAccessOptions)

	if not options.applySpeciesKeywords then
		return
	end
	local chain = MetadataManager.speciesKeywordChain(species)
	if not chain then
		return
	end

	local sessionCache = options.keywordSessionCache
	catalog:withWriteAccessDo(LOC("$$$/LrGeniusAI/Species/SaveKeywords=Save AI identified species"), function()
		local parent = createKeywordSafely(catalog, Defaults.defaultSpeciesKeyword, {}, false, nil, sessionCache)
		if not parent then
			log:error("Could not create the species root keyword; skipping the taxonomy branch")
			return
		end
		for i, entry in ipairs(chain) do
			local isLeaf = i == #chain
			-- Only the leaf is exported. A JPEG tagged "Animalia,
			-- Chordata, Aves, Great Tit" in IPTC would be noise in every
			-- downstream tool.
			local keyword = createKeywordSafely(catalog, entry.name, entry.synonyms, isLeaf, parent, sessionCache)
			if not keyword then
				log:warn("Could not create species keyword '" .. tostring(entry.name) .. "'")
				return
			end
			-- createKeyword drops its synonyms argument when returnExisting
			-- matches, so a rank keyword created by an earlier run without
			-- the binomial would never gain it.
			if #entry.synonyms > 0 then
				mergeKeywordSynonyms(keyword, entry.synonyms)
			end
			if isLeaf then
				local okAdd, errAdd = LrTasks.pcall(function()
					photo:addKeyword(keyword)
				end)
				if not okAdd then
					log:error("Failed to add species keyword to photo: " .. tostring(errAdd))
				end
			end
			parent = keyword
		end
	end, Defaults.catalogWriteAccessOptions)
end
