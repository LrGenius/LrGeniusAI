-- Helper functions

Util = {}

local DEFAULT_PARTIAL_HASH_WINDOW_BYTES = 4 * 1024 * 1024
local STABLE_ID_ALGO = "stable_meta_v1"
local LEGACY_HASH_ALGO = "md5_partial"

-- Utility function to check if table contains a value
function Util.table_contains(tbl, x)
    for _, v in pairs(tbl) do
        if v == x then
            return true
        end
    end
    return false
end

-- Utility function to dump tables as JSON scrambling the API key and removing base64 strings.
local function dumpHelper(val, indent, seen)
    indent = indent or ""
    seen = seen or {}
    local val_type = type(val)

    if val_type == "string" then
        return '"' .. tostring(val):gsub('"', '\\"') .. '"'
    elseif val_type == "number" or val_type == "boolean" or val_type == "nil" then
        return tostring(val)
    elseif val_type == "table" then
        if seen[val] then
            return "{ ...cycle... }"
        end
        seen[val] = true

        if next(val) == nil then return "{}" end -- Handle empty table

        local parts = {}
        local is_array = true
        local i = 1
        for k in pairs(val) do
            if k ~= i then
                is_array = false
                break
            end
            i = i + 1
        end

        local next_indent = indent .. "  "
        if is_array then
            for _, v in ipairs(val) do
                table.insert(parts, next_indent .. dumpHelper(v, next_indent, Util.deepcopy(seen)))
            end
            return "{\n" .. table.concat(parts, ",\n") .. "\n" .. indent .. "}"
        else -- It's a dictionary-like table
            local sorted_keys = {}
            for k in pairs(val) do table.insert(sorted_keys, k) end
            -- Sort keys, converting to string for comparison to handle mixed key types (numbers and strings)
            table.sort(sorted_keys, function(a, b)
                return tostring(a) < tostring(b)
            end)

            for _, k in ipairs(sorted_keys) do
                local v = val[k]
                local key_str = (type(k) == "string" and not k:match("^[A-Za-z_][A-Za-z0-9_]*$")) and ('["' .. k .. '"]') or tostring(k)
                table.insert(parts, next_indent .. key_str .. " = " .. dumpHelper(v, next_indent, Util.deepcopy(seen)))
            end
            return "{\n" .. table.concat(parts, ",\n") .. "\n" .. indent .. "}"
        end
    else
        return tostring(val)
    end
end

function Util.dumpTable(t)
    local s = dumpHelper(t)
    -- Redact base64 data for security
    local result = s:gsub('(data = )"([A-Za-z0-9+/=]+)"', '%1"base64 removed"')
    result = result:gsub('(url = "data:image/jpeg;base64,)([A-Za-z0-9+/]+=?=?)"', '%1base64 removed"')
    -- Redact common API key fields by name (prefs / options)
    result = result:gsub('(api_key%s*=%s*)"([^"]*)"', '%1"<redacted>"')
    result = result:gsub('(chatgptApiKey%s*=%s*)"([^"]*)"', '%1"<redacted>"')
    result = result:gsub('(geminiApiKey%s*=%s*)"([^"]*)"', '%1"<redacted>"')
    return result
end

local function trim(s)
    return s:match("^%s*(.-)%s*$")
end

function Util.trim(s)
    return trim(s)
end

function Util.nilOrEmpty(val)
    if type(val) == 'string' then
        return val == nil or trim(val) == ''
    else
        return val == nil
    end
end

---
-- Returns a stable unique identifier for the given catalog, for cross-catalog backend tracking.
-- Stored in catalog plugin properties; generated once (MD5 of path + timestamp) and reused.
-- @param catalog LrCatalog|nil Optional; defaults to LrApplication.activeCatalog().
-- @return string catalog_id (e.g. "cat_" .. 32 hex chars), or nil, error on failure.
--
function Util.getCatalogIdentifier(catalog)
    catalog = catalog or LrApplication.activeCatalog()
    if not catalog then
        return nil, "No catalog"
    end
    local existing = catalog:getPropertyForPlugin(_PLUGIN, "catalogIdentifier")
    if not Util.nilOrEmpty(existing) then
        return existing, nil
    end
    local path = catalog:getPath() or ""
    local seed = path .. tostring(LrDate.currentTime())
    local digest = LrMD5.digest(seed)
    if Util.nilOrEmpty(digest) then
        return nil, "Could not generate catalog identifier"
    end
    local catalogId = "cat_" .. digest
    catalog:withPrivateWriteAccessDo(function()
        catalog:setPropertyForPlugin(_PLUGIN, "catalogIdentifier", catalogId)
    end)
    return catalogId, nil
end

function Util.string_split(s, delimiter)
    local t = {}
    for str in string.gmatch(s, "([^" .. delimiter .. "]+)") do
        table.insert(t, trim(str))
    end
    return t
end


function Util.encodePhotoToBase64(filePath)
    local file = io.open(filePath, "rb")
    if not file then
        return nil
    end

    local data = file:read("*all")
    file:close()

    local base64 = LrStringUtils.encodeBase64(data)
    return base64
end

function Util.getDefaultPartialHashWindowBytes()
    return DEFAULT_PARTIAL_HASH_WINDOW_BYTES
end

local function safeGetRawMetadata(photo, key)
    local ok, value = LrTasks.pcall(function()
        return photo:getRawMetadata(key)
    end)
    if ok then return value end
    return nil
end

local function safeGetFormattedMetadata(photo, key)
    local ok, value = LrTasks.pcall(function()
        return photo:getFormattedMetadata(key)
    end)
    if ok then return value end
    return nil
end

function Util.computeStableMetadataPhotoId(photo)
    if not photo then
        return nil, "Photo is nil"
    end

    local fileName = safeGetFormattedMetadata(photo, "fileName") or ""
    local dateTime = safeGetRawMetadata(photo, "dateTime") or ""
    local width = safeGetRawMetadata(photo, "width") or ""
    local height = safeGetRawMetadata(photo, "height") or ""
    local fileFormat = safeGetRawMetadata(photo, "fileFormat") or ""
    local cameraModel = safeGetFormattedMetadata(photo, "cameraModel") or ""
    local lens = safeGetFormattedMetadata(photo, "lens") or ""
    local focalLength = safeGetFormattedMetadata(photo, "focalLength") or ""
    local aperture = safeGetFormattedMetadata(photo, "aperture") or ""
    local shutterSpeed = safeGetFormattedMetadata(photo, "shutterSpeed") or ""
    local isoSpeed = safeGetFormattedMetadata(photo, "isoSpeedRating") or ""

    local payload = table.concat({
        tostring(fileName),
        tostring(dateTime),
        tostring(width),
        tostring(height),
        tostring(fileFormat),
        tostring(cameraModel),
        tostring(lens),
        tostring(focalLength),
        tostring(aperture),
        tostring(shutterSpeed),
        tostring(isoSpeed),
    }, "|")

    if Util.nilOrEmpty(payload) or payload == string.rep("|", 10) then
        return nil, "Insufficient metadata for stable photo ID"
    end

    local digest = LrMD5.digest(payload)
    if Util.nilOrEmpty(digest) then
        return nil, "Stable metadata digest failed"
    end
    return "meta1:" .. digest, nil
end

local function getFileAttributes(filePath)
    if Util.nilOrEmpty(filePath) then
        return nil, "File path missing"
    end

    if not LrFileUtils.exists(filePath) then
        return nil, "File does not exist"
    end
    if not LrFileUtils.isReadable(filePath) then
        return nil, "File is not readable"
    end

    local attributes = LrFileUtils.fileAttributes(filePath) or {}
    local fileSize = tonumber(attributes.fileSize)
    if not fileSize then
        return nil, "Could not read file size"
    end

    return {
        fileSize = fileSize,
        fileModificationDate = tonumber(attributes.fileModificationDate) or 0,
    }, nil
end

function Util.computePartialFileMd5(filePath, windowBytes)
    if type(LrMD5) ~= "table" or type(LrMD5.digest) ~= "function" then
        return nil, "LrMD5.digest is unavailable"
    end

    local startedAt = LrDate.currentTime()
    local attributes, attrErr = getFileAttributes(filePath)
    if not attributes then
        log:error("computePartialFileMd5: file attribute error for " .. tostring(filePath) .. ": " .. tostring(attrErr))
        return nil, attrErr
    end

    local chunkSize = math.max(1, tonumber(windowBytes) or DEFAULT_PARTIAL_HASH_WINDOW_BYTES)
    local fh = io.open(filePath, "rb")
    if not fh then
        log:error("computePartialFileMd5: could not open file for binary read: " .. tostring(filePath))
        return nil, "Could not open file for binary read"
    end

    local firstLen = math.min(attributes.fileSize, chunkSize)
    local firstChunk = fh:read(firstLen) or ""

    local lastChunk = ""
    if attributes.fileSize > firstLen then
        local lastOffset = math.max(0, attributes.fileSize - chunkSize)
        fh:seek("set", lastOffset)
        lastChunk = fh:read(math.min(chunkSize, attributes.fileSize)) or ""
    end
    fh:close()

    local md5Input = tostring(attributes.fileSize) .. ":" .. firstChunk .. ":" .. lastChunk
    local digest = LrMD5.digest(md5Input)
    if Util.nilOrEmpty(digest) then
        log:error("computePartialFileMd5: digest failed for " .. tostring(filePath))
        return nil, "MD5 digest failed"
    end

    local elapsedMs = math.floor((LrDate.currentTime() - startedAt) * 1000)
    log:trace(
        "computePartialFileMd5: file=" .. tostring(filePath) ..
        " size=" .. tostring(attributes.fileSize) ..
        " chunkSize=" .. tostring(chunkSize) ..
        " firstLen=" .. tostring(firstLen) ..
        " lastLen=" .. tostring(string.len(lastChunk)) ..
        " elapsedMs=" .. tostring(elapsedMs)
    )

    return digest, {
        fileSize = attributes.fileSize,
        fileModificationDate = attributes.fileModificationDate,
        windowBytes = chunkSize,
    }
end

function Util.buildGlobalPhotoId(filePath, windowBytes)
    local digest, metadataOrErr = Util.computePartialFileMd5(filePath, windowBytes)
    if not digest then
        return nil, metadataOrErr
    end

    if type(metadataOrErr) ~= "table" then
        return nil, "Invalid hash metadata"
    end
    local metadata = metadataOrErr
    local fileSize = tostring(metadata.fileSize or 0)
    local mtime = tostring(math.floor(tonumber(metadata.fileModificationDate) or 0))
    local globalPhotoId = "md5p:" .. fileSize .. ":" .. mtime .. ":" .. digest
    return globalPhotoId, metadata
end

function Util.getGlobalPhotoIdForPhoto(photo, options)
    options = options or {}
    if not photo then
        return nil, "Photo is nil"
    end

    local originalFilePath = photo:getRawMetadata("path")
    local attributes, attrErr = getFileAttributes(originalFilePath)
    if not attributes then
        log:error("getGlobalPhotoIdForPhoto: file attributes unavailable for photo path=" .. tostring(originalFilePath) .. " err=" .. tostring(attrErr))
        return nil, attrErr
    end

    local cachedId = photo:getPropertyForPlugin(_PLUGIN, "globalPhotoId")
    local cachedAlgorithm = photo:getPropertyForPlugin(_PLUGIN, "globalPhotoIdAlgorithm")
    local cachedSize = tonumber(photo:getPropertyForPlugin(_PLUGIN, "globalPhotoIdFileSize") or "")
    local cachedMtime = tonumber(photo:getPropertyForPlugin(_PLUGIN, "globalPhotoIdFileModificationDate") or "")

    if not options.forceRecompute and not Util.nilOrEmpty(cachedId) then
        if cachedAlgorithm == STABLE_ID_ALGO then
            -- log:trace("getGlobalPhotoIdForPhoto: cache hit for " .. tostring(originalFilePath))
            return cachedId, nil
        end
        if cachedAlgorithm == LEGACY_HASH_ALGO
            and cachedSize == tonumber(attributes.fileSize)
            and math.floor(cachedMtime or 0) == math.floor(tonumber(attributes.fileModificationDate) or 0) then
            -- log:trace("getGlobalPhotoIdForPhoto: cache hit for legacy hash " .. tostring(originalFilePath))
            return cachedId, nil
        end
    end

    local rebuildStartedAt = LrDate.currentTime()
    local globalPhotoId, idErr = Util.computeStableMetadataPhotoId(photo)
    local metadata = {
        fileSize = attributes.fileSize,
        fileModificationDate = attributes.fileModificationDate,
        algorithm = STABLE_ID_ALGO,
    }

    if not globalPhotoId then
        log:warn("getGlobalPhotoIdForPhoto: stable metadata id failed, falling back to partial hash for " .. tostring(originalFilePath) .. " err=" .. tostring(idErr))
        local fallbackId, metadataOrErr = Util.buildGlobalPhotoId(originalFilePath, options.windowBytes)
        if not fallbackId then
            log:error("getGlobalPhotoIdForPhoto: failed for " .. tostring(originalFilePath) .. " err=" .. tostring(metadataOrErr))
            return nil, metadataOrErr
        end
        if type(metadataOrErr) ~= "table" then
            return nil, "Invalid photo metadata"
        end
        globalPhotoId = fallbackId
        metadata = metadataOrErr
        metadata.algorithm = LEGACY_HASH_ALGO
    end

    local catalog = LrApplication.activeCatalog()
    catalog:withPrivateWriteAccessDo(function()
        photo:setPropertyForPlugin(_PLUGIN, "globalPhotoId", globalPhotoId)
        photo:setPropertyForPlugin(_PLUGIN, "globalPhotoIdFileSize", tostring(metadata.fileSize or ""))
        photo:setPropertyForPlugin(_PLUGIN, "globalPhotoIdFileModificationDate", tostring(metadata.fileModificationDate or ""))
        photo:setPropertyForPlugin(_PLUGIN, "globalPhotoIdAlgorithm", tostring(metadata.algorithm or STABLE_ID_ALGO))
    end)

    local rebuildElapsedMs = math.floor((LrDate.currentTime() - rebuildStartedAt) * 1000)
    log:trace(
        "getGlobalPhotoIdForPhoto: cache miss -> generated id for " .. tostring(originalFilePath) ..
        " elapsedMs=" .. tostring(rebuildElapsedMs) ..
        " idPrefix=" .. tostring(string.sub(globalPhotoId, 1, 24))
    )

    return globalPhotoId, nil
end

function Util.getStringsFromRelativePath(absolutePath)
    local catalog = LrApplication.activeCatalog()
    local rootFolders = catalog:getFolders()

    for _, folder in ipairs(rootFolders) do
        local rootFolder = folder:getPath()
        log:trace("Root folder: " .. rootFolder)
        local relativePath = LrPathUtils.parent(LrPathUtils.makeRelative(absolutePath, rootFolder))
        if relativePath ~= nil and string.len(relativePath) > 0 and string.len(relativePath) < string.len(absolutePath) then
            log:trace("Relative path: " .. relativePath)
            relativePath = string.gsub(relativePath, "[/\\\\]", " ")
            relativePath = string.gsub(relativePath, "[^%a%säöüÄÖÜ]", "")
            relativePath = string.gsub(relativePath, "[^%w%s]", "")
            log:trace("Processed relative path: " .. relativePath)
            return relativePath
        end
    end
end

function Util.getLogfilePath()
    local filename = "LrGeniusAI.log"
    local macPath14 = LrPathUtils.getStandardFilePath('home') .. "/Library/Logs/Adobe/Lightroom/LrClassicLogs/"
    local winPath14 = LrPathUtils.getStandardFilePath('home') .. "\\AppData\\Local\\Adobe\\Lightroom\\Logs\\LrClassicLogs\\"
    local macPathOld = LrPathUtils.getStandardFilePath('documents') .. "/LrClassicLogs/"
    local winPathOld = LrPathUtils.getStandardFilePath('documents') .. "\\LrClassicLogs\\"

    local lightroomVersion = LrApplication.versionTable()

    if lightroomVersion.major >= 14 then
        if MAC_ENV then
            return macPath14 .. filename
        else
            return winPath14 .. filename
        end
    else
        if MAC_ENV then
            return macPathOld .. filename
        else
            return winPathOld .. filename
        end
    end
end

function Util.table_size(table)
    local count = 0
    for _ in pairs(table) do
        count = count + 1
    end
    return count
end

function Util.copyLogfilesToDesktop()

    local folder = LrPathUtils.child(LrPathUtils.getStandardFilePath('desktop'), "LrGenius_" .. LrDate.timeToIsoDate(LrDate.currentTime()))
    if not LrFileUtils.exists(folder) then
        log:trace("Removing pre-existing report folder: " .. folder)
        LrFileUtils.moveToTrash(folder)
    end
    log:trace("Creating report folder: " .. folder)
    LrFileUtils.createDirectory(folder)

    local filePath = LrPathUtils.child(folder, 'LrGeniusAI.log')
    local logFilePath = Util.getLogfilePath()
    if LrFileUtils.exists(logFilePath) then
        LrFileUtils.copy(logFilePath, filePath)
    else
        ErrorHandler.showError(LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/logfileNotFound=Logfile not found", logFilePath)
    end

    local ollamaLogfilePath = Util.getOllamaLogfilePath()
    if LrFileUtils.exists(ollamaLogfilePath) then
        LrFileUtils.copy(ollamaLogfilePath, LrPathUtils.child(folder, 'ollama.log'))
    else
        log:trace("Ollama log file not found at: " .. ollamaLogfilePath)
    end

    if prefs.enableOpenClip then
        local lrgeniusClipLogfilePath = LrPathUtils.child(LrPathUtils.parent(LrApplication.activeCatalog():getPath()), "lrgenius-server.log")
        if LrFileUtils.exists(lrgeniusClipLogfilePath) then
            LrFileUtils.copy(lrgeniusClipLogfilePath, LrPathUtils.child(folder, 'lrgenius-server.log'))
        else
            log:trace("lrgenius-server log file not found at: " .. lrgeniusClipLogfilePath)
        end
    end

    if LrFileUtils.exists(filePath) then
        LrShell.revealInShell(filePath)
    else
        ErrorHandler.showError(LOC "$$$/lrc-ai-assistant/PluginInfoDialogSections/logfileCopyFailed=Logfile copy failed", filePath)
    end
end

function Util.getOllamaLogfilePath()
    local macPath = LrPathUtils.getStandardFilePath('home') .. "/.ollama/logs/server.log"
    local winPath = LrPathUtils.getStandardFilePath('home') .. "\\AppData\\Local\\ollama\\server.log"

    if MAC_ENV then
        log:trace("Using macOS path for Ollama log: " .. macPath)
        return macPath
    else
        log:trace("Using Windows path for Ollama log: " .. winPath)
        return winPath
    end
end

function Util.deepcopy(o, seen)

    seen = seen or {}
    if o == nil then return nil end
    if seen[o] then return seen[o] end

    local no
    if type(o) == 'table' then
        no = {}
        seen[o] = no

        for k, v in next, o, nil do
            no[Util.deepcopy(k, seen)] = Util.deepcopy(v, seen)
        end
    setmetatable(no, Util.deepcopy(getmetatable(o), seen))
    else
        no = o
    end
    return no

end



---
-- Returns true if a table is a keyword leaf object.
-- Supported shape: { name = "keyword", synonyms = { ... } }
local function isKeywordLeafObject(value)
    return type(value) == "table" and type(value.name) == "string"
end

local function sanitizeKeywordLeaf(value)
    if type(value) == "string" then
        local keyword = Util.trim(value)
        if keyword == "" then
            return nil, {}
        end
        return keyword, {}
    end

    if isKeywordLeafObject(value) then
        local keyword = Util.trim(value.name)
        if keyword == "" then
            return nil, {}
        end

        local cleanedSynonyms = {}
        local seen = {}
        if type(value.synonyms) == "table" then
            for _, synonym in ipairs(value.synonyms) do
                if type(synonym) == "string" then
                    local synonymText = Util.trim(synonym)
                    local normalized = string.lower(synonymText)
                    if synonymText ~= "" and normalized ~= string.lower(keyword) and not seen[normalized] then
                        table.insert(cleanedSynonyms, synonymText)
                        seen[normalized] = true
                    end
                end
            end
        end

        return keyword, cleanedSynonyms
    end

    return nil, {}
end

local function iterateDeterministic(tbl, callback)
    local stringKeys = {}
    local numberKeys = {}
    for key in pairs(tbl) do
        if type(key) == "number" then
            table.insert(numberKeys, key)
        elseif type(key) == "string" then
            table.insert(stringKeys, key)
        end
    end

    table.sort(stringKeys, function(a, b) return a < b end)
    table.sort(numberKeys, function(a, b) return a < b end)

    for _, key in ipairs(stringKeys) do
        callback(key, tbl[key])
    end
    for _, key in ipairs(numberKeys) do
        callback(key, tbl[key])
    end
end

---
-- Extracts all keyword leaf names from the hierarchical table.
-- Keeps an optional metadata map with synonyms for structured leaves.
--
-- @param hierarchicalTable The original table with categories.
-- @return table keywordsVal, table keywordsMeta
--
function Util.extractAllKeywords(hierarchicalTable)
    if hierarchicalTable == nil or type(hierarchicalTable) ~= "table" then
        return {}, {}
    end

    local result = {}
    local meta = {}
    local keywordCounter = 0

    local function recurse(tbl)
        iterateDeterministic(tbl, function(_, value)
            if type(value) == "table" and not isKeywordLeafObject(value) then
                recurse(value)
                return
            end

            local keywordName, synonyms = sanitizeKeywordLeaf(value)
            if keywordName and keywordName ~= "" then
                keywordCounter = keywordCounter + 1
                local keywordId = "kw_" .. tostring(keywordCounter)
                result[keywordId] = keywordName
                meta[keywordId] = { synonyms = synonyms }
            end
        end)
    end

    recurse(hierarchicalTable)

    log:trace("Extracted keywords: " .. Util.dumpTable(result))

    return result, meta
end

---
-- Recursively rebuilds the hierarchical table structure based on a
-- list of selected keywords.
--
-- @param originalTable The original multidimensional table, used as a structural template.
-- @param keywordsVal A table mapping keyword keys to their values.
-- @param keywordsSel A table indicating which keywords are selected (key = true).
-- @param keywordsMeta Optional metadata table with synonyms for each keyword key.
-- @return A new hierarchical table containing only the selected keywords.
--
function Util.rebuildTableFromKeywords(originalTable, keywordsVal, keywordsSel, keywordsMeta)
    local keywordCounter = 0

    local function buildKeywordLeaf(keywordId, fallbackValue, fallbackSynonyms)
        if not keywordsSel[keywordId] then
            return nil
        end
        local newKeyword = keywordsVal[keywordId] or fallbackValue
        if not newKeyword or Util.trim(newKeyword) == "" then
            return nil
        end
        newKeyword = Util.trim(newKeyword)
        local meta = keywordsMeta and keywordsMeta[keywordId] or nil
        local synonyms = (meta and meta.synonyms) or fallbackSynonyms or {}
        if synonyms and #synonyms > 0 then
            return {
                name = newKeyword,
                synonyms = Util.deepcopy(synonyms),
            }
        end
        return newKeyword
    end

    local function recurse(tbl)
        local newTbl = {}
        iterateDeterministic(tbl, function(key, value)
            if type(value) == "table" and not isKeywordLeafObject(value) then
                local child = recurse(value)
                if next(child) ~= nil then
                    newTbl[key] = child
                end
                return
            end

            keywordCounter = keywordCounter + 1
            local keywordId = "kw_" .. tostring(keywordCounter)

            if type(value) == "string" then
                local leafValue = buildKeywordLeaf(keywordId, value, {})
                if leafValue ~= nil then
                    newTbl[#newTbl + 1] = leafValue
                end
            elseif isKeywordLeafObject(value) then
                local leafValue = buildKeywordLeaf(keywordId, value.name, value.synonyms or {})
                if leafValue ~= nil then
                    newTbl[#newTbl + 1] = leafValue
                end
            end
        end)
        return newTbl
    end

    return recurse(originalTable)
end

--- 
-- Converts an LrKeyword object to a string representing its full hierarchy.
-- Format: Parent-Keyword>Parent-Keyword>...>Keyword
-- @param keyword The LrKeyword object.
-- @return A string with the full keyword path.
--
function Util.keywordToHierarchyString(keyword)
    local parts = {}
    local current = keyword
    while current do
        table.insert(parts, 1, current:getName())
        current = current:getParent()
    end
    return table.concat(parts, ">")
end


---
-- Converts a hierarchy string (Parent-Keyword>...>Keyword) into a hierarchy of LrKeyword objects.
-- If the hierarchy does not exist, it will be created.
-- The parent keywords are created with includeOnExport = false, the deepest keyword with includeOnExport = true.
-- Returns the deepest LrKeyword object.
-- @param hierarchyString The string to convert.
-- @return The deepest LrKeyword object representing the hierarchy.
--

function Util.hierarchyStringToOrCreateKeyword(hierarchyString)
    local catalog = LrApplication.activeCatalog()
    local keywordNames = Util.string_split(hierarchyString, ">")
    local parent = nil
    local keywordObj = nil

    catalog:withWriteAccessDo("CreateKeywordHierarchy", function()
        for i, name in ipairs(keywordNames) do
            local includeOnExport = (i == #keywordNames)
            keywordObj = catalog:createKeyword(name, nil, includeOnExport, parent, true)
            parent = keywordObj
        end
    end, Defaults.catalogWriteAccessOptions)

    return keywordObj
end

---
-- Converts a multidimensional Lua table of keywords and parent keywords
-- into a string of keywords separated by ';'.
-- Each keyword is represented in the format Parent>Parent>...>Keyword.
-- Example input:
-- {
--   Location = { Europe = { City = { "Berlin", "Hamburg" } }, Country = { "Germany" } },
--   Plants = { Type = { "Tree", "Bush" }, "Oak" }
-- }
-- Output: "Location>Europe>City>Berlin;Location>Europe>City>Hamburg;Location>Country>Germany;Plants>Type>Tree;Plants>Type>Bush;Plants>Oak"
--
-- @param keywordTable The multidimensional table.
-- @return A string with all keywords in hierarchy format, separated by ';'.
--
function Util.keywordTableToHierarchyStringList(keywordTable)
    local result = {}

    local function recurse(tbl, path)
        for k, v in pairs(tbl) do
            if type(v) == "table" then
                -- If the key is a number, treat v as a leaf keyword
                if type(k) == "number" then
                    table.insert(result, table.concat(path, ">") .. ">" .. v)
                else
                    -- Otherwise, k is a parent keyword
                    local newPath = Util.deepcopy(path) or {}
                    table.insert(newPath, k)
                    recurse(v, newPath)
                end
            else
                -- v is a leaf keyword, k is parent or index
                if type(k) == "number" then
                    table.insert(result, table.concat(path, ">") .. ">" .. v)
                else
                    table.insert(result, table.concat(path, ">") .. ">" .. k .. ">" .. v)
                end
            end
        end
    end

    recurse(keywordTable, {})
    return table.concat(result, ";")
end

function Util.keywordTableToStringList(keywordTable)
    local result = {}

    local function recurse(tbl, path)
        for k, v in pairs(tbl) do
            if type(v) == "string" then
                table.insert(result, v)
            elseif type(v) == "table" then
                recurse(v, path)
            end
        end
    end

    recurse(keywordTable, {})
    log:trace(table.concat(result, ";"))
    return table.concat(result, ";")
end

function Util.get_keys(t)
  local keys={}
  for key,_ in pairs(t) do
    table.insert(keys, key)
  end
  return keys
end

function Util.waitForServerDialog()
    if SearchIndexAPI.pingServer() then
        local compatible, versionMessage = SearchIndexAPI.ensureVersionCompatibility()
        if compatible then
            return true
        end

        -- If the backend is local and currently running an older process, we can restart it
        -- once and then re-check compatibility (covers "stale backend still running").
        if SearchIndexAPI.isBackendOnLocalhost() then
            log:trace("Backend version mismatch detected; attempting local backend restart once.")

            -- Best-effort: try graceful shutdown first; structured lifecycle will escalate if needed.
            LrTasks.pcall(function()
                SearchIndexAPI.shutdownServer({
                    graceSeconds = 5,
                    forceWaitSeconds = 5,
                    pollIntervalSeconds = 0.5,
                    shutdownRequestTimeoutSeconds = 5,
                })
            end)

            LrTasks.sleep(1)
            LrTasks.pcall(function()
                SearchIndexAPI.startServer({ readyTimeoutSeconds = 60 })
            end)

            if SearchIndexAPI.pingServer() then
                local compatible2, versionMessage2 = SearchIndexAPI.ensureVersionCompatibility()
                if compatible2 then
                    return true
                end
                versionMessage = versionMessage2 or versionMessage
            end
        end

        LrDialogs.message(
            "Plugin/Backend version mismatch",
            versionMessage or "Version check failed.",
            "critical"
        )
        return false
    end

    local result = false

    LrFunctionContext.callWithContext("WaitForServerContext", function(waitContext)
        local canceled = false

        local progressScope = LrDialogs.showModalProgressDialog({
            title = LOC "$$$/lrc-ai-assistant/WaitForServer/title=LrGeniusAI",
            caption = LOC "$$$/lrc-ai-assistant/WaitForServer/caption=Waiting for LrGeniusAI database to load...",
            cannotCancel = false,
            functionContext = waitContext
        })

        local elapsedTime = 0
        local timeout = 60 -- 60 seconds timeout
        local restartAttempted = false
        while not progressScope:isCanceled() and elapsedTime < timeout do
            if SearchIndexAPI.pingServer() then
                local compatible, versionMessage = SearchIndexAPI.ensureVersionCompatibility()
                progressScope:done()
                if compatible then
                    result = true
                    return
                end

                -- If we found a mismatch after the server started (likely a stale backend),
                -- restart it once and re-check.
                if SearchIndexAPI.isBackendOnLocalhost() and not restartAttempted then
                    restartAttempted = true
                    log:trace("Backend version mismatch detected after ping; restarting local backend once.")

                    LrTasks.pcall(function()
                        SearchIndexAPI.shutdownServer({
                            graceSeconds = 5,
                            forceWaitSeconds = 5,
                            pollIntervalSeconds = 0.5,
                            shutdownRequestTimeoutSeconds = 5,
                        })
                    end)
                    LrTasks.sleep(1)
                    LrTasks.pcall(function()
                        SearchIndexAPI.startServer({ readyTimeoutSeconds = 60 })
                    end)

                    -- Re-check compatibility immediately (without restarting the modal loop).
                    if SearchIndexAPI.pingServer() then
                        local compatible2, versionMessage2 = SearchIndexAPI.ensureVersionCompatibility()
                        if compatible2 then
                            result = true
                            return
                        end
                        versionMessage = versionMessage2 or versionMessage
                    end
                end

                LrDialogs.message(
                    "Plugin/Backend version mismatch",
                    versionMessage or "Version check failed.",
                    "critical"
                )
                result = false
                return
            end
            LrTasks.sleep(0.5) -- Poll every 500ms
            elapsedTime = elapsedTime + 0.5
            progressScope:setPortionComplete(elapsedTime, timeout)
        end

        if elapsedTime >= timeout then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/common/ServerTimeout/Title=LrGeniusAI Database Timeout",
                LOC "$$$/LrGeniusAI/common/ServerTimeout/Message=Timed out waiting for the LrGeniusAI server to respond. Please contact LrGeniusAI support for assistance."
            )
        end
    end)

    return result
end

---
-- Adds a photo to the "Rejected AI Descriptions" collection (under set "LrGeniusAI").
-- Finds or creates the set and collection by name, then adds the photo.
-- @param photo LrPhoto
-- @param writeOptions optional; e.g. Defaults.catalogWriteAccessOptions
--
function Util.addPhotoToRejectedDescriptionsCollection(photo, writeOptions)
    if not photo then return end
    writeOptions = writeOptions or { timeout = 60 }
    local catalog = LrApplication.activeCatalog()
    local setName = LOC "$$$/LrGeniusAI/Rejected/CollectionSetName=LrGeniusAI"
    local collName = LOC "$$$/LrGeniusAI/Rejected/CollectionName=Rejected AI Descriptions"

    local collectionSet, collection

    local function findSetAndCollection()
        local children = catalog:getChildCollections()
        if children then
            for _, child in ipairs(children) do
                if child:type() == "LrCollectionSet" and child:getName() == setName then
                    collectionSet = child
                    break
                end
            end
        end
        if not collectionSet then
            collectionSet = catalog:createCollectionSet(setName, nil, true)
        end
        if collectionSet then
            local collChildren = collectionSet:getChildCollections()
            if collChildren then
                for _, c in ipairs(collChildren) do
                    if c:type() == "LrCollection" and c:getName() == collName then
                        collection = c
                        break
                    end
                end
            end
            if not collection then
                collection = catalog:createCollection(collName, collectionSet, false)
            end
        end
    end

    catalog:withWriteAccessDo(LOC "$$$/LrGeniusAI/Rejected/AddToCollection=Add to Rejected AI Descriptions", function()
        findSetAndCollection()
        if collection then
            collection:addPhotos({ photo })
        end
    end, writeOptions)
end

return Util
