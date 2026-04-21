-- MetadataManager.lua
-- Handles reading and writing metadata from/to the Lightroom catalog.

MetadataManager = {}
local createKeywordSafely

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
    log:trace("Applying metadata to photo: " .. photo:getFormattedMetadata('fileName'))
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
        local existingTitle = photo:getFormattedMetadata('title') or ""
        local existingCaption = photo:getFormattedMetadata('caption') or ""
        local existingAltText = photo:getFormattedMetadata('altTextAccessibility') or ""
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
    catalog:withWriteAccessDo(LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/saveTitleCaption=Save AI generated title and caption", function()
        if saveCaption and caption and caption ~= "" then
            photo:setRawMetadata('caption', caption)
        end
        if saveTitle and title and title ~= "" then
            photo:setRawMetadata('title', title)
        end
        if saveAltText and altText and altText ~= "" then
            photo:setRawMetadata('altTextAccessibility', altText)
        end
    end, Defaults.catalogWriteAccessOptions)

    -- Save keywords (sessionCache avoids LrKeyword:getChildren() when the SDK errors there)
    log:trace("Saving keywords to catalog")
    if saveKeywords and keywords ~= nil and type(keywords) == 'table' and prefs.generateKeywords then
        local keywordSessionCache = {}
        local topKeyword = nil
        if prefs.useKeywordHierarchy and options.useTopLevelKeyword then
            catalog:withWriteAccessDo("$$$/lrc-ai-assistant/AnalyzeImageTask/saveTopKeyword=Save AI generated keywords", function()
                topKeyword = createKeywordSafely(catalog, options.topLevelKeyword or "LrGeniusAI", { Defaults.topLevelKeywordSynonym }, false, nil, keywordSessionCache)
                if topKeyword then
                    local okAdd, errAdd = LrTasks.pcall(function()
                        photo:addKeyword(topKeyword) -- Add top-level keyword to photo. To see the number of tagged photos in keyword list (Gerald Uhl)
                    end)
                    if not okAdd then
                        log:error("Failed to add top-level keyword to photo: " .. tostring(errAdd))
                    end
                end
            end)
            -- Keep track of used top-level keywords
            if not Util.table_contains(prefs.knownTopLevelKeywords, options.topLevelKeyword) then
                table.insert(prefs.knownTopLevelKeywords, options.topLevelKeyword)
            end
        end
        local existingKeywordNames = nil
        local currentTopLevelKeyword = options.useTopLevelKeyword and (options.topLevelKeyword or "LrGeniusAI") or nil
        catalog:withWriteAccessDo("$$$/lrc-ai-assistant/AnalyzeImageTask/saveTopKeyword=Save AI generated keywords", function()
            MetadataManager.addKeywordRecursively(photo, catalog, keywords, topKeyword, existingKeywordNames, currentTopLevelKeyword, keywordSessionCache)
        end, Defaults.catalogWriteAccessOptions)
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
local function findKeywordByNameInParent(photo, catalog, sessionCache, parent, keywordName)
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
                log:trace("findKeywordByNameInParent: getChildren/getKeywords failed (SDK bug), using createKeyword fallback: " .. errStr)
            end
        else
            log:trace("findKeywordByNameInParent: getChildren/getKeywords failed (SDK bug), using createKeyword fallback: " .. errStr)
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
-- Adds incoming synonyms to an existing Lightroom keyword (additive-only).
-- Existing synonyms are preserved; duplicates are removed case-insensitively.
-- @param keywordObj LrKeyword
-- @param incomingSynonyms table
local function mergeKeywordSynonyms(keywordObj, incomingSynonyms)
    if not keywordObj or type(incomingSynonyms) ~= "table" or #incomingSynonyms == 0 then
        return
    end
    if not keywordObj.getSynonyms then
        return
    end

    local okName, keywordName = LrTasks.pcall(function() return keywordObj:getName() or "" end)
    if not okName then
        log:trace("mergeKeywordSynonyms: getName failed, skipping synonyms for this node: " .. tostring(keywordName))
        return 
    end

    local okSyn, existing = LrTasks.pcall(function() return keywordObj:getSynonyms() or {} end)
    if not okSyn then
        log:trace("mergeKeywordSynonyms: getSynonyms failed, skipping synonyms for this node: " .. tostring(existing))
        return
    end

    local merged = {}
    local seen = {}

    local function addSynonymIfValid(value)
        if type(value) ~= "string" then
            return
        end
        local synonym = Util.trim(value)
        local lowered = string.lower(synonym)
        if synonym == "" or lowered == string.lower(keywordName) or seen[lowered] then
            return
        end
        seen[lowered] = true
        table.insert(merged, synonym)
    end

    for _, synonym in ipairs(existing) do
        addSynonymIfValid(synonym)
    end

    local addedIncoming = false
    for _, synonym in ipairs(incomingSynonyms) do
        local beforeCount = #merged
        addSynonymIfValid(synonym)
        if #merged > beforeCount then
            addedIncoming = true
        end
    end

    if not addedIncoming then
        return
    end

    if not keywordObj.setSynonyms then
        log:warn("Cannot merge synonyms for keyword '" .. tostring(keywordName) .. "': setSynonyms API unavailable")
        return
    end

    local ok, err = LrTasks.pcall(function()
        keywordObj:setSynonyms(merged)
    end)
    if not ok then
        log:warn("Failed to merge synonyms for keyword '" .. tostring(keywordName) .. "': " .. tostring(err))
    end
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
-- Recursively adds keywords to a photo, creating parent keywords as needed.
-- @param photo The LrPhoto object.
-- @param catalog The LrCatalog object.
-- @param keywordSubTable A table of keywords, possibly nested.
-- @param parent The parent LrKeyword object for the current level.
-- @param existingKeywordNames Optional set of keyword names already on the photo (append mode).
-- @param currentTopLevelKeyword Optional top-level keyword for this task (avoids prefs race in parallel jobs).
-- @param sessionCache Optional table: parent -> keyword name -> LrKeyword (same pass as applyMetadata).
--
function MetadataManager.addKeywordRecursively(photo, catalog, keywordSubTable, parent, existingKeywordNames, currentTopLevelKeyword, sessionCache)
    local function parseKeywordLeaf(leafValue)
        if type(leafValue) == "string" then
            local keywordName = Util.trim(leafValue)
            return keywordName, {}
        end
        if type(leafValue) == "table" and type(leafValue.name) == "string" then
            local keywordName = Util.trim(leafValue.name)
            local synonyms = {}
            local seenSynonyms = {}
            if type(leafValue.synonyms) == "table" then
                for _, synonym in ipairs(leafValue.synonyms) do
                    if type(synonym) == "string" then
                        local synonymText = Util.trim(synonym)
                        local normalized = string.lower(synonymText)
                        if synonymText ~= "" and normalized ~= string.lower(keywordName) and not seenSynonyms[normalized] then
                            table.insert(synonyms, synonymText)
                            seenSynonyms[normalized] = true
                        end
                    end
                end
            end
            return keywordName, synonyms
        end
        return nil, {}
    end

    local function isKeywordLeafObject(value)
        return type(value) == "table" and type(value.name) == "string"
    end

    local addKeywords = {}
    local reservedTopLevel = currentTopLevelKeyword or prefs.topLevelKeyword
    for key, value in pairs(keywordSubTable) do
        local keyword
        if type(key) == 'string' and key ~= "" and key ~= "None" and key ~= "none" and prefs.useKeywordHierarchy then
            keyword = createKeywordSafely(catalog, key, {}, false, parent, sessionCache)
        elseif type(key) == 'number' and value then
            local keywordName, keywordSynonyms = parseKeywordLeaf(value)
            if not keywordName or keywordName == "" or keywordName == "None" or keywordName == "none" then
                -- Skip invalid keyword leafs
            elseif not Util.table_contains(addKeywords, keywordName) then
                if keywordName == "Ollama" or keywordName == "LMStudio" or keywordName == "Google Gemini" or keywordName == "ChatGPT" or keywordName == reservedTopLevel then
                    log:trace("Skipping keyword: " .. tostring(keywordName) .. " as it is reserved.")
                else
                    local currentParent = prefs.useKeywordHierarchy and parent or nil
                    keyword = findKeywordByNameInParent(photo, catalog, sessionCache, currentParent, keywordName)
                    if keyword then
                        mergeKeywordSynonyms(keyword, keywordSynonyms)
                    else
                        keyword = createKeywordSafely(catalog, keywordName, keywordSynonyms, true, currentParent, sessionCache)
                        mergeKeywordSynonyms(keyword, keywordSynonyms)
                    end
                    if keyword then
                        local okAdd, errAdd = LrTasks.pcall(function()
                            photo:addKeyword(keyword)
                        end)
                        if okAdd then
                            table.insert(addKeywords, keywordName)
                        else
                            log:error("Failed to add keyword '" .. tostring(keywordName) .. "' to photo: " .. tostring(errAdd))
                        end
                    end
                end
            end
        end
        if type(value) == 'table' and not isKeywordLeafObject(value) then
            MetadataManager.addKeywordRecursively(photo, catalog, value, keyword, existingKeywordNames, currentTopLevelKeyword, sessionCache)
        end
    end
end




function MetadataManager.showValidationDialog(ctx, photo, response, options)
    local f = LrView.osFactory()
    local bind = LrView.bind
    local share = LrView.share

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
        propertyTable['keywordsSel_' .. id] = true
        propertyTable['keywordsVal_' .. id] = fullPath
    end
    
    propertyTable.title = title or ""
    propertyTable.caption = caption or ""
    propertyTable.altText = altText or ""

    propertyTable.saveKeywords = keywords ~= nil and type(keywords) == 'table'
    -- By default, save if data is present, regardless of pre-flight options
    propertyTable.saveTitle = title ~= nil and title ~= ""
    propertyTable.saveCaption = caption ~= nil and caption ~= ""
    propertyTable.saveAltText = altText ~= nil and altText ~= ""

    local keywordRows = {
        spacing = 2,
    }

    for _, id in ipairs(orderedIds) do
        table.insert(keywordRows, f:row {
            f:checkbox { 
                value = bind('keywordsSel_' .. id), 
                visible = bind 'saveKeywords' 
            },
            f:edit_field {
                value = bind('keywordsVal_' .. id),
                width_in_chars = 45, -- Enough for long paths
                immediate = true,
                enabled = bind 'saveKeywords'
            },
        })
    end

    local dialogView = f:row {
        bind_to_object = propertyTable,
        spacing = 20,
        f:column {
            width = 250,
            f:static_text {
                title = photo:getFormattedMetadata('fileName'),
                font = "<system/bold>",
                wrap = true,
                width = 250,
            },
            f:catalog_photo {
                photo = photo,
                width = 250,
                height = 250,
            },
            f:spacer { height = 10 },
            f:checkbox {
                value = bind 'skipFromHere',
                title = LOC "$$$/LrGeniusAI/MetadataManager/SkipRemaining=Save following without reviewing.",
            },
        },
        f:column {
            f:group_box {
                title = LOC "$$$/LrGeniusAI/Keywords=Keywords",
                fill_horizontal = 1,
                f:row {
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/MetadataManager/SelectAll=Select All",
                        action = function()
                            for _, id in ipairs(orderedIds) do
                                propertyTable['keywordsSel_' .. id] = true
                            end
                        end
                    },
                    f:push_button {
                        title = LOC "$$$/LrGeniusAI/MetadataManager/DeselectAll=Deselect All",
                        action = function()
                            for _, id in ipairs(orderedIds) do
                                propertyTable['keywordsSel_' .. id] = false
                            end
                        end
                    },
                    f:spacer { fill_horizontal = 1 },
                    f:checkbox {
                        value = bind 'saveKeywords',
                        title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/SaveKeywords=Save keywords",
                    },
                },
                f:scrolled_view {
                    height = 250,
                    width = 560,
                    f:column(keywordRows)
                },
            },
            f:group_box {
                title = LOC "$$$/LrGeniusAI/Metadata=Metadata",
                fill_horizontal = 1,
                f:row {
                    f:checkbox {
                        value = bind 'saveTitle',
                        title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/SaveTitle=Save title",
                    },
                    f:edit_field {
                        value = bind 'title',
                        fill_horizontal = 1,
                        height_in_lines = 1,
                        enabled = bind 'saveTitle',
                    },
                },
                f:row {
                    f:checkbox {
                        value = bind 'saveCaption',
                        title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/SaveCaption=Save caption",
                    },
                    f:edit_field {
                        value = bind 'caption',
                        fill_horizontal = 1,
                        height_in_lines = 5,
                        enabled = bind 'saveCaption',
                    },
                },
                f:row {
                    f:checkbox {
                        value = bind 'saveAltText',
                        title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/SaveAltText=Save alt text",
                    },
                    f:edit_field {
                        value = bind 'altText',
                        fill_horizontal = 1,
                        height_in_lines = 3,
                        enabled = bind 'saveAltText',
                    },
                },
            },
        },
    }

    local result = LrDialogs.presentModalDialog({
        title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/ReviewWindowTitle=Review results" .. (photo and (": " .. photo:getFormattedMetadata('fileName')) or ""),
        otherVerb = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/discard=Discard",
        contents = dialogView,
    })

    local results = {}
    local validatedKeywords = {}
    if propertyTable.saveKeywords then
        -- Construct a new hierarchical table from the full path strings
        local pathsWithMeta = {}
        for _, id in ipairs(orderedIds) do
            if propertyTable['keywordsSel_' .. id] then
                table.insert(pathsWithMeta, {
                    path = propertyTable['keywordsVal_' .. id],
                    synonyms = kwMeta[id] and kwMeta[id].synonyms or {}
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
    local keywords = photo:getRawMetadata('keywords')
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
