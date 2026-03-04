-- MetadataManager.lua
-- Handles reading and writing metadata from/to the Lightroom catalog.

MetadataManager = {}

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

    -- Save keywords
    log:trace("Saving keywords to catalog")
    if saveKeywords and keywords ~= nil and type(keywords) == 'table' and prefs.generateKeywords then
        local topKeyword = nil
        if prefs.useKeywordHierarchy and options.useTopLevelKeyword then
            catalog:withWriteAccessDo("$$$/lrc-ai-assistant/AnalyzeImageTask/saveTopKeyword=Save AI generated keywords", function()
                topKeyword = catalog:createKeyword(options.topLevelKeyword or "LrGeniusAI", { Defaults.topLevelKeywordSynonym }, false, nil, true)
                photo:addKeyword(topKeyword) -- Add top-level keyword to photo. To see the number of tagged photos in keyword list (Gerald Uhl)
            end)
            -- Keep track of used top-level keywords
            if not Util.table_contains(prefs.knownTopLevelKeywords, options.topLevelKeyword) then
                table.insert(prefs.knownTopLevelKeywords, options.topLevelKeyword)
            end
        end
        catalog:withWriteAccessDo("$$$/lrc-ai-assistant/AnalyzeImageTask/saveTopKeyword=Save AI generated keywords", function()
            MetadataManager.addKeywordRecursively(photo, keywords, topKeyword)
        end, Defaults.catalogWriteAccessOptions)
    end

    -- Save quality scores
    log:trace("Saving quality scores to catalog")
    if response.quality and type(response.quality) == 'table' and options.applyQuality then
        catalog:withPrivateWriteAccessDo(function()
            log:trace("Saving quality scores to catalog")
            for key, value in pairs(response.quality) do
                --log:trace("Setting property for key: " .. key .. " value: " .. tostring(value))
                photo:setPropertyForPlugin(_PLUGIN, key, tostring(value))
            end
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
-- Recursively adds keywords to a photo, creating parent keywords as needed.
-- @param photo The LrPhoto object.
-- @param keywordSubTable A table of keywords, possibly nested.
-- @param parent The parent LrKeyword object for the current level.
--
function MetadataManager.addKeywordRecursively(photo, keywordSubTable, parent)
    local addKeywords = {}
    for key, value in pairs(keywordSubTable) do
        -- log:trace("Processing keyword key: " .. tostring(key) .. " value: " .. tostring(value))
        local keyword
        if type(key) == 'string' and key ~= "" and key ~= "None" and key ~= "none" and prefs.useKeywordHierarchy then
            keyword = photo.catalog:createKeyword(key, {}, false, parent, true)
        elseif type(key) == 'number' and value and value ~= "" and value ~= "None" and value ~= "none" then
            local currentParent = prefs.useKeywordHierarchy and parent or nil
            if not Util.table_contains(addKeywords, value) then
                if value == "Ollama" or value == "LMStudio" or value == "Google Gemini" or value == "ChatGPT" or value == prefs.topLevelKeyword then
                    log:trace("Skipping keyword: " .. tostring(value) .. " as it is reserved.")
                else
                    keyword = photo.catalog:createKeyword(value, {}, true, currentParent, true)
                    photo:addKeyword(keyword)
                    table.insert(addKeywords, value)
                end
            end
        end
        if type(value) == 'table' then
            MetadataManager.addKeywordRecursively(photo, value, keyword)
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
    propertyTable.keywordsVal = Util.extractAllKeywords(keywords or {})
    propertyTable.keywordsSel = {}
    propertyTable.title = title or ""
    propertyTable.caption = caption or ""
    propertyTable.altText = altText or ""

    propertyTable.saveKeywords = keywords ~= nil and type(keywords) == 'table' and options.applyKeywords ~= false
    propertyTable.saveTitle = title ~= nil and title ~= "" and options.applyTitle ~= false
    propertyTable.saveCaption = caption ~= nil and caption ~= "" and options.applyCaption ~= false
    propertyTable.saveAltText = altText ~= nil and altText ~= "" and options.applyAltText ~= false
    -- propertyTable.keywordWidth = 50

    local keywordRows = {}
    local keywordLabels = {}

    local keywordCount = 0
    for _, keyword in pairs(propertyTable.keywordsVal) do
        if propertyTable.keywordsSel[_] == nil then -- Prevent duplicates
            propertyTable.keywordsSel[_] = true
            keywordCount = keywordCount + 1
            table.insert(keywordLabels, f:checkbox { value = bind('keywordsSel.' .. _), visible = bind 'saveKeywords' })
            table.insert(keywordLabels, f:edit_field { value = bind('keywordsVal.' .. _), width_in_chars = 15, immediate = true, enabled = bind 'saveKeywords' })
        end
    end

    local rowCount = #keywordLabels / 10 + 1

    for i = 1, rowCount do
        local row = {}
        for j = 1, 10 do
            local index = (i - 1) * 10 + j
            if index <= #keywordLabels then
                table.insert(row, keywordLabels[index])
            end
        end
        table.insert(keywordRows, f:row(row))
    end

    keywordRows.horizontal_scroller = true
    keywordRows.vertical_scroller = true
    keywordRows.height = 250
    keywordRows.width = 1100

    local dialogView = f:column {
        bind_to_object = propertyTable,
        f:row {
            f:static_text {
                title = photo:getFormattedMetadata('fileName'),
                font = "<system/bold>",
            },
            f:catalog_photo {
                photo = photo,
                width = 150,
            },
        },
        f:row {
            margin_vertical = 10,
            f:checkbox {
                value = bind 'saveKeywords',
                width = share 'checkboxWidth',
            },
            f:static_text {
                title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/SaveKeywords=Save keywords",
                width = share 'labelWidth',
            },
            f:scrolled_view(keywordRows),
        },
        f:row {
            margin_vertical = 10,
            f:checkbox {
                value = bind 'saveTitle',
                width = share 'checkboxWidth',
            },
            f:static_text {
                title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/SaveTitle=Save title",
                width = share 'labelWidth',
            },
            f:edit_field {
                value = bind 'title',
                -- width_in_chars = 40,
                fill_horizontal = 1,
                height_in_lines = 1,
                enabled = bind 'saveTitle',  -- Enable only if the checkbox is checked
            },
        },
        f:row {
            margin_vertical = 10,
            f:checkbox {
                value = bind 'saveCaption',
                width = share 'checkboxWidth',
            },
            f:static_text {
                title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/SaveCaption=Save caption",
                width = share 'labelWidth',
            },
            f:edit_field {
                value = bind 'caption',
                fill_horizontal = 1,
                height_in_lines = 10,
                enabled = bind 'saveCaption',  -- Enable only if the checkbox is checked
            },
        },
        f:row {
            margin_vertical = 10,
            f:checkbox {
                value = bind 'saveAltText',
                width = share 'checkboxWidth',
            },
            f:static_text {
                title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/SaveAltText=Save alt text",
                width = share 'labelWidth',
            },
            f:edit_field {
                value = bind 'altText',
                fill_horizontal = 1,
                height_in_lines = 10,
                enabled = bind 'saveAltText',  -- Enable only if the checkbox is checked
            },
        },
        f:row {
            margin_vertical = 10,
            f:checkbox {
                value = bind 'skipFromHere'
            },
            f:static_text {
                title = LOC "$$$/lrc-ai-assistant/AnalyzeImageTask/SkipFromHere=Save following without reviewing.",
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
        validatedKeywords = Util.rebuildTableFromKeywords(keywords, propertyTable.keywordsVal, propertyTable.keywordsSel)
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
