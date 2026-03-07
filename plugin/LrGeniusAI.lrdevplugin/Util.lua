-- Helper functions

Util = {}

local DEFAULT_PARTIAL_HASH_WINDOW_BYTES = 4 * 1024 * 1024

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

    local attributes, attrErr = getFileAttributes(filePath)
    if not attributes then
        return nil, attrErr
    end

    local chunkSize = math.max(1, tonumber(windowBytes) or DEFAULT_PARTIAL_HASH_WINDOW_BYTES)
    local fh = io.open(filePath, "rb")
    if not fh then
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
        return nil, "MD5 digest failed"
    end

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
        return nil, attrErr
    end

    local cachedId = photo:getPropertyForPlugin(_PLUGIN, "globalPhotoId")
    local cachedSize = tonumber(photo:getPropertyForPlugin(_PLUGIN, "globalPhotoIdFileSize") or "")
    local cachedMtime = tonumber(photo:getPropertyForPlugin(_PLUGIN, "globalPhotoIdFileModificationDate") or "")

    if not options.forceRecompute and not Util.nilOrEmpty(cachedId)
        and cachedSize == attributes.fileSize
        and cachedMtime == attributes.fileModificationDate then
        return cachedId, nil
    end

    local globalPhotoId, metadataOrErr = Util.buildGlobalPhotoId(originalFilePath, options.windowBytes)
    if not globalPhotoId then
        return nil, metadataOrErr
    end

    if type(metadataOrErr) ~= "table" then
        return nil, "Invalid photo metadata"
    end
    local metadata = metadataOrErr
    local catalog = LrApplication.activeCatalog()
    catalog:withPrivateWriteAccessDo(function()
        photo:setPropertyForPlugin(_PLUGIN, "globalPhotoId", globalPhotoId)
        photo:setPropertyForPlugin(_PLUGIN, "globalPhotoIdFileSize", metadata.fileSize)
        photo:setPropertyForPlugin(_PLUGIN, "globalPhotoIdFileModificationDate", metadata.fileModificationDate)
        photo:setPropertyForPlugin(_PLUGIN, "globalPhotoIdAlgorithm", "md5_partial")
    end)

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
-- Extracts all keywords (strings) from the inner arrays of the
-- hierarchical table and returns them as a single, flat list.
--
-- @param hierarchicalTable The original table with categories.
-- @return A flat table (array) containing all keywords.
--
function Util.extractAllKeywords(hierarchicalTable)
    if hierarchicalTable == nil or type(hierarchicalTable) ~= "table" then
        return {}
    end

    local flatList = {}

    local function recurse(tbl)
        for _, v in pairs(tbl) do
            if type(v) == "table" then
                recurse(v)
            elseif type(v) == "string" then
                table.insert(flatList, Util.trim(v))
            end
        end
    end

    recurse(hierarchicalTable)

    log:trace("Extracted keywords: " .. Util.dumpTable(flatList))

    local result = {}

    for _, keyword in ipairs(flatList) do
        local keywordWoDots = string.gsub(keyword, "%.", "_;_")
        -- Ensure that each keyword is trimmed of whitespace
        result[keywordWoDots] = keyword
    end

    return result
end

---
-- Recursively rebuilds the hierarchical table structure based on a
-- list of selected keywords.
--
-- @param originalTable The original multidimensional table, used as a structural template.
-- @param keywordsVal A table mapping keyword keys to their values.
-- @param keywordsSel A table indicating which keywords are selected (key = true).
-- @return A new hierarchical table containing only the selected keywords.
--
function Util.rebuildTableFromKeywords(originalTable, keywordsVal, keywordsSel)
    local function recurse(tbl)
        local newTbl = {}
        for key, value in pairs(tbl) do
            if type(value) == "table" then
                local child = recurse(value)
                -- Only add non-empty child tables
                if next(child) ~= nil then
                    newTbl[key] = child
                end
            elseif type(value) == "string" then
                local keywordWoDots = string.gsub(value, "%.", "_;_")
                if keywordsSel[keywordWoDots] then
                    newTbl[#newTbl + 1] = keywordsVal[keywordWoDots]
                end
            end
        end
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
        return true
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
        while not progressScope:isCanceled() and elapsedTime < timeout do
            if SearchIndexAPI.pingServer() then
                progressScope:done()
                result = true
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

return Util
