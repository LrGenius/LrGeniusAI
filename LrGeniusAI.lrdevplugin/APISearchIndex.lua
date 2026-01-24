-- lrgenius-server API Wrapper
-- Provides functions to interact with the Python-based search index server.

SearchIndexAPI = {}

local BASE_URL = "http://127.0.0.1:19819"
local ENDPOINTS = {
    INDEX = "/index",
    INDEX_BY_REFERENCE = "/index_by_reference",
    INDEX_BASE64 = "/index_base64",
    SEARCH = "/search",
    STATS = "/stats",
    MODELS = "/models",
    GET_IDS = "/get/ids",
    REMOVE = "/remove",
    PING = "/ping",
    SHUTDOWN = "/shutdown",
    IMPORT_METADATA = "/import/metadata",
    START_CLIP_DOWNLOAD = "/clip/download/start",
    STATUS_CLIP_DOWNLOAD = "/clip/download/status",
    CLIP_STATUS = "/clip/status",
}

local EXPORT_SETTINGS = {
        LR_export_destinationType = 'specificFolder',
        LR_export_useSubfolder = false,
        LR_format = 'JPEG',
        LR_jpeg_quality = tonumber(prefs.exportQuality) or 60,
        LR_minimizeEmbeddedMetadata = true,
        LR_outputSharpeningOn = false,
        LR_size_doConstrain = true,
        LR_size_maxHeight = tonumber(prefs.exportSize) or 1024,
        LR_size_resizeType = 'longEdge',
        LR_size_units = 'pixels',
        LR_collisionHandling = 'rename',
        LR_includeVideoFiles = false,
        LR_removeLocationMetadata = true,
        LR_embeddedMetadataOption = "all",
    }


-- Forward declarations for private helper functions
local _request

---
-- Exports a photo to a temporary location for processing.
-- @param photo The Lightroom photo object to export.
-- @return string|nil The path to the exported JPEG file, or nil on failure.
--
function SearchIndexAPI.exportPhotoForIndexing(photo)

    if photo == nil then
        log:error("exportPhotoForIndexing: photo is nil. Probably it got deleted in the meantime.")
        return nil
    end

    local tempDir = LrPathUtils.getStandardFilePath('temp')
    local photoName = LrPathUtils.leafName(photo:getFormattedMetadata('fileName'))
    local catalog = LrApplication.activeCatalog()

    EXPORT_SETTINGS.LR_export_destinationPathPrefix = tempDir
   
    local exportSession = LrExportSession({
        photosToExport = { photo },
        exportSettings = EXPORT_SETTINGS
    })

    for _, rendition in exportSession:renditions() do
        local success, path = rendition:waitForRender()
        log:trace("Export completed for photo: " .. photoName .. " Success: " .. tostring(success) .. " Path: " .. tostring(path))
        if success then -- Export successful
            return path
        else
            -- Error during export
            log:error("Failed to export photo for indexing. " .. (path or 'unknown error'))
            return nil
        end
    end
end

function SearchIndexAPI.exportPhotosForIndexing(photos)
    if not photos or #photos == 0 then return {} end

    local tempDir = LrPathUtils.getStandardFilePath('temp')

    EXPORT_SETTINGS.LR_export_destinationPathPrefix = tempDir

    local exportSession = LrExportSession({
        photosToExport = photos,
        exportSettings = EXPORT_SETTINGS
    })

    local photoPaths = {}
    local photoIndex = 1
    for _, rendition in exportSession:renditions() do
        local success, path = rendition:waitForRender()
        local photo = photos[photoIndex]
        if photo ~= nil then
            local photoName = LrPathUtils.leafName(photo:getFormattedMetadata('fileName'))
            log:trace("Export completed for photo: " .. photoName .. " Success: " .. tostring(success) .. " Path: " .. tostring(path))
            if success then
                photoPaths[photo] = path
            else
                log:error("Failed to export photo for indexing. " .. (path or 'unknown error'))
                photoPaths[photo] = nil
            end
        else
            log:error("Photo is nil in exportPhotosForIndexing, probably it got deleted in the meantime.")
        end
        photoIndex = photoIndex + 1
    end
    return photoPaths
end


---
-- Unified function to analyze and index photos with metadata, quality scores, and embeddings.
-- Replaces the old separate analyze and index workflows.
-- @param uuid string The UUID of the photo.
-- @param filename string The filename of the photo.
-- @param jpeg string The JPEG data of the photo.
-- @param options table Optional parameters for the analysis:
--   - tasks table: Array of tasks to perform (default: {"embeddings", "metadata", "quality"})
--   - provider string: AI provider to use (default: "qwen")
--   - language string: Language for generated content (default: "English")
--   - temperature number: Temperature for AI generation (default: 0.2)
--   - generate_keywords boolean: Generate keywords (default: true)
--   - generate_caption boolean: Generate caption (default: true)
--   - generate_title boolean: Generate title (default: true)
--   - generate_alt_text boolean: Generate alt text (default: false)
--   - submit_gps boolean: Submit GPS coordinates (default: false)
--   - gps_coordinates table: GPS coordinates {latitude, longitude}
--   - submit_keywords boolean: Submit existing keywords (default: false)
--   - existing_keywords table: Array of existing keywords
--   - submit_folder_names boolean: Submit folder names (default: false)
--   - folder_names string: Folder path
--   - user_context string: Additional context for the photo
-- @return boolean success, table|string response - Returns success status and response data or error message
---


function SearchIndexAPI.analyzeAndIndexPhoto(uuid, filepath, options)
    if filepath == nil then 
        log:error("JPEG is nil")
        return false, "No image data provided"
    end

    local filename = LrPathUtils.leafName(filepath)

    options = options or {}
    
    local url = BASE_URL .. ENDPOINTS.INDEX_BY_REFERENCE

    local body = {
        path = filepath,
        images = {      
            { path = filepath, uuid = uuid },
        },
        
        -- Tasks to perform (default: all three)
        tasks = options.tasks or {"embeddings", "metadata", "quality"},
        
        -- AI Provider settings
        provider = options.provider,
        model = options.model,
        api_key = options.api_key,
        language = options.language or prefs.generateLanguage or "English",
        temperature = options.temperature or prefs.temperature or 0.2,
        replace_ss = options.replace_ss or false,
        
        -- Metadata generation options
        generate_keywords = options.generate_keywords,
        generate_caption = options.generate_caption,
        generate_title = options.generate_title,
        generate_alt_text = options.generate_alt_text,
        
        -- Context options
        submit_gps = options.submit_gps,
        submit_keywords = options.submit_keywords,
        submit_folder_names = options.submit_folder_names,
        user_context = options.user_context,
        gps_coordinates = options.gps_coordinates,
        existing_keywords = options.existing_keywords,
        folder_names = options.folder_names,
        prompt = options.prompt,
        keyword_categories = JSON:encode(options.keyword_categories or {}),
        date_time = options.date_time
        
    }

    -- Regeneration control: if false, server will only fill missing fields
    body.regenerate_metadata = options.regenerate_metadata ~= false -- default true if nil
    
    log:trace("Analyzing and indexing photo: " .. filename .. " with tasks: " .. table.concat(body.tasks, ", "))

    local response, err = _request('POST', url, body, 720)

    if not response then
        log:error("Failed to analyze/index photo: " .. tostring(err))
        return false, err or "Unknown error"
    end

    -- Check response status
    if response.status == "processed" then
        local success_count = response.success_count or 0
        local failure_count = response.failure_count or 0
        
        if success_count > 0 then
            log:trace("Successfully processed photo: " .. filename)
            return true, response
        else
            log:error("Photo processing failed: " .. filename)
            return false, response.error or "Processing failed"
        end
    else
        log:error("Unexpected response status: " .. tostring(response.status))
        return false, "Unexpected response status"
    end
end




---
-- Builds a URL with optional query parameters.
--
local function buildUrlWithParams(baseUrl, params)
    local queryParts = {}
    for key, value in pairs(params) do
        if value ~= nil then
            table.insert(queryParts, key .. "=" .. tostring(value))
        end
    end
    
    if #queryParts > 0 then
        return baseUrl .. "?" .. table.concat(queryParts, "&")
    else
        return baseUrl
    end
end

function SearchIndexAPI.searchIndex(searchTerm, qualitySort, photosToSearch)
    local params = {
        term = searchTerm,
        quality_sort = qualitySort,
    }

    local url = BASE_URL .. ENDPOINTS.SEARCH

    if photosToSearch and #photosToSearch > 0 then
        -- Perform a scoped search via POST
        local uuids = {}
        for _, photo in ipairs(photosToSearch) do
            table.insert(uuids, photo:getRawMetadata("uuid"))
        end

        local body = {
            term = searchTerm,
            uuids = uuids
        }
        local postUrl = buildUrlWithParams(url, params)

        log:trace("Searching index via POST (scoped): " .. postUrl)
        return _request('POST', postUrl, body)
    else
        -- Perform a global search via GET
        local getUrl = buildUrlWithParams(url, params)
        log:trace("Searching index via GET (global): " .. getUrl)
        return _request('GET', getUrl)
    end
end

function SearchIndexAPI.getStats()
    return _request('GET', BASE_URL .. ENDPOINTS.STATS)
end

function SearchIndexAPI.getAllIndexedPhotoUUIDs(requireEmbeddings)
    local url = BASE_URL .. ENDPOINTS.GET_IDS
    -- If requireEmbeddings is true, only get UUIDs with real embeddings
    if requireEmbeddings then
        url = url .. "?has_embedding=true"
    end
    return _request('GET', url)
end

---
-- Retrieves metadata and quality scores for a photo by UUID.
-- @param uuid The UUID of the photo to retrieve.
-- @return table|nil Response containing metadata and quality fields, or nil on error.
-- Response structure:
--   {
--     status = "success",
--     uuid = "...",
--     metadata = { title = "...", caption = "...", keywords = {...}, alt_text = "..." },
--     quality = { overall_score = 0.8, composition_score = 0.9, ... }
--   }
--
function SearchIndexAPI.getPhotoData(uuid)
    if not uuid then
        log:error("getPhotoData: UUID is required")
        return nil
    end
    
    local url = BASE_URL .. "/get"
    local body = { uuid = uuid }
    
    log:trace("Retrieving photo data for UUID: " .. uuid)
    
    local result, err = _request('POST', url, body)
    if err then
        log:error("Failed to retrieve photo data: " .. err)
        return nil
    end
    
    if result and result.status == "success" then
        log:trace("Successfully retrieved photo data for UUID: " .. uuid)
        return result
    else
        log:warn("Photo data not found for UUID: " .. uuid)
        return nil
    end
end

function SearchIndexAPI.removeUUID(uuid)
    local url = BASE_URL .. ENDPOINTS.REMOVE
    local body = { uuid = uuid }
    log:trace("Removing UUID: " .. uuid)

    local result, err = _request('POST', url, body)
    if not err then
        return true
    else
        ErrorHandler.handleError("Remove UUID failed", err)
        return false
    end
end

function SearchIndexAPI.removeMissingFromIndex()
    local indexedUUIDs = SearchIndexAPI.getAllIndexedPhotoUUIDs()

    if indexedUUIDs == nil then
        log:warn("Failed to retrieve indexed UUIDs")
        return false
    end

    local catalog = LrApplication.activeCatalog()

    local progressScope = LrProgressScope({
        title = LOC "$$$/LrGeniusAI/SearchIndexAPI/cleaningIndex=Cleaning search index",
        functionContext = nil,
    })

    local total = #indexedUUIDs
    local missingPhotosUUIDs = {}
    for _, uuid in ipairs(indexedUUIDs) do
        progressScope:setPortionComplete(_ - 1, total)
        progressScope:setCaption(LOC "$$$/LrGeniusAI/SearchIndexAPI/cleaningIndexProgress=Cleaning index. Photo ^1/^2", tostring(_), tostring(total))
        if progressScope:isCanceled() then break end

        local photo = catalog:findPhotoByUuid(uuid)
        if photo == nil then
            missingPhotosUUIDs[#missingPhotosUUIDs + 1] = uuid
            log:trace("Photo with UUID " .. uuid .. " not found in catalog, removing from index")
            SearchIndexAPI.removeUUID(uuid)
        end
    end
    progressScope:done()
end

---
-- Analyzes and indexes selected photos with LLM processing (metadata, quality, embeddings).
-- Uses JPEG export instead of thumbnails for better reliability.
-- @param selectedPhotos table Array of LrPhoto objects to process.
-- @param progressScope LrProgressScope Progress scope for UI updates.
-- @param options table Processing options (tasks, provider, language, temperature, etc.).
-- @return string status Status: "success", "canceled", "somefailed", or "allfailed".
-- @return number processed Number of photos processed.
-- @return number failed Number of photos that failed.
-- @return table responses Array of response data from the server for each photo.
--
function SearchIndexAPI.analyzeAndIndexSelectedPhotos(selectedPhotos, progressScope, options)
    local numPhotos = #selectedPhotos
    if numPhotos == 0 then
        return "success", 0, 0, {}
    end

    if not SearchIndexAPI.pingServer() then
        return "allfailed", numPhotos, numPhotos, {}
    end

    options = options or {}
    
    progressScope:setCaption(LOC "$$$/LrGeniusAI/AnalyzeAndIndex/ProgressTitle=Processing photos...")
    progressScope:setPortionComplete(0, numPhotos)

    local photoToProcessStack = {}
    for _, photo in ipairs(selectedPhotos) do
        table.insert(photoToProcessStack, photo)
    end

    local maxWorkers = 1 -- tonumber(prefs.indexingParallelTasks) or 2
    local stats = { processed = 0, success = 0, failed = 0 }
    local processedPhotos = {}
    local responses = {}
    local activeWorkers = 0
    local keepRunning = true
    local catalog = LrApplication.activeCatalog()
    
    local analyzeWorker = function()
        while #photoToProcessStack > 0 do
            if progressScope:isCanceled() then break end
            if not keepRunning then break end
            
            local photo = table.remove(photoToProcessStack, 1)
            if photo ~= nil then
                
                local uuid = photo:getRawMetadata("uuid")
                local filename = photo:getFormattedMetadata("fileName")
                
                -- Export photo as JPEG
                local exportedPhotoPath = SearchIndexAPI.exportPhotoForIndexing(photo)
                
                if exportedPhotoPath ~= nil then

                    -- Prepare analysis options with photo-specific context
                    local photoOptions = {}
                    for k, v in pairs(options) do
                        photoOptions[k] = v
                    end

                    log:trace("Options for photo " .. filename .. ": " .. Util.dumpTable(photoOptions))
                    
                    -- Add GPS if enabled
                    if options.submit_gps then
                        local gps = photo:getRawMetadata('gps')
                        if gps then
                            photoOptions.gps_coordinates = gps
                        end
                    end
                    
                    -- Add existing keywords if enabled
                    if options.submit_keywords then
                        local keywords = photo:getFormattedMetadata("keywordTagsForExport")
                        if keywords then
                            photoOptions.existing_keywords = keywords
                        end
                    end
                    
                    -- Add folder names if enabled
                    if options.submit_folder_names then
                        local originalFilePath = photo:getRawMetadata("path")
                        if originalFilePath then
                            photoOptions.folder_names = Util.getStringsFromRelativePath(originalFilePath)
                        end
                    end


                    if options.submit_date_time then
                        local datetime = photo:getRawMetadata("dateTime")
                        if datetime ~= nil and type(datetime) == "number" then
                            photoOptions.date_time = LrDate.timeToW3CDate(datetime)
                        end
                    end


                    photoOptions.user_context = catalog:getPropertyForPlugin(_PLUGIN, 'photoContext') or ""

                    -- Call unified API to index/analyze
                    local success, indexResponse = SearchIndexAPI.analyzeAndIndexPhoto(uuid, exportedPhotoPath, photoOptions)
                    if success then
                        stats.success = stats.success + 1
                    else
                        stats.failed = stats.failed + 1
                        log:error("Failed to analyze/index photo: " .. filename .. " Error: " .. (indexResponse or "Unknown"))
                    end
                    -- Cleanup temp filename
                    LrFileUtils.delete(exportedPhotoPath)
                else
                    stats.failed = stats.failed + 1
                    log:error("Failed to read exported photo: " .. filename)
                end
                

                
                stats.processed = stats.processed + 1
                table.insert(processedPhotos, photo)
                progressScope:setPortionComplete(stats.processed, numPhotos)
                progressScope:setCaption(
                    LOC("$$$/LrGeniusAI/AnalyzeAndIndex/ProcessingPhoto=Processing ^1 successful (^2 total/^3 failed)",
                        stats.success, numPhotos, stats.failed)
                )
            else
                log:error("Photo is nil in analyze worker, probably it got deleted in the meantime.")
            end
        end
        log:trace("Analyze worker thread finished.")
        activeWorkers = activeWorkers - 1
    end

    -- Start worker threads
    for i = 1, maxWorkers do
        LrTasks.startAsyncTask(analyzeWorker)
        log:trace("Started analyze worker #" .. tostring(i))
        activeWorkers = activeWorkers + 1
    end

    -- Monitor workers and server availability
    local notReached = 0
    while activeWorkers > 0 do
        if progressScope:isCanceled() then break end
        if MAC_ENV then
            LrTasks.yield()
        else
            LrTasks.sleep(0.1)
        end
    end

    -- Wait for workers to stop in case of server failure
    if not keepRunning then
        while activeWorkers > 0 do
            if MAC_ENV then
                LrTasks.yield()
            else
                LrTasks.sleep(0.5)
            end
        end
    end

    progressScope:done()

    if progressScope:isCanceled() then
        return "canceled", stats.processed, stats.failed, processedPhotos
    end

    local status
    if stats.failed == 0 then
        status = "success"
    elseif stats.failed >= stats.processed and stats.processed > 0 then
        status = "allfailed"
    else
        status = "somefailed"
    end
    
    return status, stats.processed, stats.failed, processedPhotos
end



function SearchIndexAPI.importMetadataFromCatalog(photosToProcess, progressScope)
    local numPhotos = #photosToProcess
    if numPhotos == 0 then
        return "success", 0, 0
    end

    if not SearchIndexAPI.pingServer() then
        return "allfailed", numPhotos, numPhotos
    end

    progressScope:setCaption(LOC "$$$/LrGeniusAI/ImportMetadata/ProgressTitle=Importing metadata for photos...")
    progressScope:setPortionComplete(0, numPhotos)

    local stats = { processed = 0, success = 0, failed = 0 }
    local batchSize = 50 -- Send metadata in batches
    local metadataBatch = {}

    for i, photo in ipairs(photosToProcess) do
        if photo ~= nil then 
            if progressScope:isCanceled() then
                break
            end

            local metadata = {
                uuid = photo:getRawMetadata("uuid"),
                caption = photo:getFormattedMetadata("caption"),
                title = photo:getFormattedMetadata("title"),
                keywords = MetadataManager.getPhotoKeywordHierarchy(photo),
                alt_text = photo:getFormattedMetadata("altTextAccessibility")
            }
            table.insert(metadataBatch, metadata)

            if #metadataBatch >= batchSize or i == numPhotos then
                local response = _request('POST', BASE_URL .. ENDPOINTS.IMPORT_METADATA, { metadata_items = metadataBatch })
                if response ~= nil and response.status == "processed" then
                    stats.success = stats.success + #metadataBatch
                else
                    stats.failed = stats.failed + #metadataBatch
                    log:error("Failed to import metadata batch: " .. (response and response.error or "Unknown error"))
                end
                metadataBatch = {} -- Clear the batch
            end

            stats.processed = stats.processed + 1
            progressScope:setPortionComplete(stats.processed, numPhotos)
            progressScope:setCaption(
                LOC("$$$/LrGeniusAI/ImportMetadata/Processing=Importing metadata... ^1/^2 (^3 failed)",
                    stats.processed, numPhotos, stats.failed)
            )
        else
            log:error("Photo is nil in importMetadataFromCatalog, probably it got deleted in the meantime.")
        end
    end

    progressScope:done()

    if progressScope:isCanceled() then
        return "canceled", stats.processed, stats.failed
    end

    local status
    if stats.failed == 0 then
        status = "success"
    elseif stats.failed >= stats.processed and stats.processed > 0 then
        status = "allfailed"
    else
        status = "somefailed"
    end

    return status, stats.processed, stats.failed
end



function SearchIndexAPI.pingServer()
    local url = BASE_URL .. "/ping"
    local result, hdrs = LrHttp.get(url)
    if hdrs.status == 200 and result == "pong" then
        return true
    else
        return false
    end
end

function SearchIndexAPI.shutdownServer()
    if not SearchIndexAPI.pingServer() then
        log:trace("Search index server is not running")
        return true
    end

    local url = BASE_URL .. ENDPOINTS.SHUTDOWN
    log:trace("Shutting down server")
    
    _request('POST', url)
end

function SearchIndexAPI.killServer()
    if not SearchIndexAPI.pingServer() then
        log:trace("Search index server is not running")
        return true
    end

    log:trace("Attempting to shut down search index server gracefully")
    SearchIndexAPI.shutdownServer()

    local pidFilePath = LrPathUtils.child(LrPathUtils.parent(LrApplication.activeCatalog():getPath()), "lrgenius-server.pid")

    local pidFile = io.open(pidFilePath, "r")
    if not pidFile then
        log:error("Error: Could not open PID file at " .. pidFilePath)
        return false
    end

    local pid = pidFile:read("*l")
    pidFile:close()

    if not pid then
        log:error("Error: Could not read PID from the file.")
        return false
    end
    
    local pid_number = tonumber(pid)
    if not pid_number then
        log:error("Error: The content of the PID file is not a valid number.")
        return false
    end

    log:trace("Attempting to kill process with PID: " .. pid)

    local command
    if WIN_ENV then
        command = "taskkill /PID " .. pid
    elseif MAC_ENV then
        command = "kill " .. pid
    end

    LrTasks.startAsyncTask(function()
        local success = LrTasks.execute(command)

        if success == 0 then
            log:trace("Successfully killed the process.")
        else
            log:error("Error: Failed to kill the process. Command returned " .. tostring(success))
        end
        return success == 0
    end)
end


function SearchIndexAPI.startServer()
    if SearchIndexAPI.pingServer() then
        log:trace("Search index server is already running")
        return true
    end

    local serverDir = LrPathUtils.child(LrPathUtils.parent(_PLUGIN.path), "lrgenius-server")
    local serverBinary = LrPathUtils.child(serverDir, "lrgenius-server")
    if WIN_ENV then
        serverBinary = serverBinary .. ".exe"
    end

    if not LrFileUtils.exists(serverBinary) then
        log:error(serverBinary .. " not found. Not trying to start server")
        return
    end

    LrTasks.startAsyncTask(function()
        local startServerCmd = nil
        
        if WIN_ENV then
            -- Set KMP_DUPLICATE_LIB_OK environment variable to fix OpenMP library conflict in PyInstaller builds
            local envCmd = "set KMP_DUPLICATE_LIB_OK=TRUE &&"
            startServerCmd = "start /b /d \"" .. serverDir .. "\" \"\" cmd /c \"" .. envCmd .. " lrgenius-server.exe"
            startServerCmd = startServerCmd .. " --db-path \"" .. LrPathUtils.child(LrPathUtils.parent(LrApplication.activeCatalog():getPath()), "lrgenius.db") .. "\""
            startServerCmd = startServerCmd .. "\""
        else 
            -- Set environment variable for Mac as well
            local envPrefix = "KMP_DUPLICATE_LIB_OK=TRUE "
            startServerCmd = serverBinary
            startServerCmd = envPrefix .. "\"" .. startServerCmd .. "\" --db-path \"" .. LrPathUtils.child(LrPathUtils.parent(LrApplication.activeCatalog():getPath()), "lrgenius.db") .. "\""
        end
        log:trace("Trying to start search index server with command: " .. startServerCmd)
        local result = LrTasks.execute(startServerCmd)
        log:trace("Search index server start command result: " .. tostring(result))
    end)

    LrTasks.startAsyncTask(function()
        LrTasks.sleep(20)
        if SearchIndexAPI.pingServer() then
            log:trace("Search index server is running")
            return true
        else
            LrTasks.sleep(20)
            if SearchIndexAPI.pingServer() then
                log:trace("Search index server is running")
                return true
            end
            return false
        end
    end)
end

_request = function(method, url, body, timeout)
    local result, hdrs
    local bodyString = (body and type(body) == 'table') and JSON:encode(body) or nil

    if method == 'GET' then
        result, hdrs = LrHttp.get(url, timeout)
    elseif method == 'POST' then
        result, hdrs = LrHttp.post(url, bodyString or "", { { field = "Content-Type", value = "application/json" } }, 'POST', timeout)
    elseif method == 'PUT' then
        result, hdrs = LrHttp.post(url, bodyString or "", { { field = "Content-Type", value = "application/json" } }, 'PUT', timeout)
    elseif method == 'DELETE' then
        result, hdrs = LrHttp.post(url, bodyString or "", { { field = "Content-Type", value = "application/json" } }, 'DELETE', timeout)
    else
        local err = "Unsupported HTTP method: " .. method
        log:error(err)
        return nil, err
    end

    if hdrs ~= nil and hdrs.status ~= nil and hdrs.status >= 200 and hdrs.status < 300 then
        if result and #result > 0 then
            return JSON:decode(result)
        end
        return {} -- Return an empty table for successful but empty responses
    else
        local err_msg = "API request failed. HTTP status: " .. tostring(hdrs and hdrs.status or 'unknown')
        if result and #result > 0 then
            local decoded_err = JSON:decode(result)
            if decoded_err and decoded_err.error then
                err_msg = err_msg .. " - " .. decoded_err.error
            else
                err_msg = err_msg .. " Response: " .. result
            end
        end
        log:error(err_msg)
        return nil, err_msg
    end
end


function SearchIndexAPI.getMissingPhotosFromIndex(requireEmbeddings)
    -- If requireEmbeddings is true, we only get photos that have real embeddings
    -- (excluding metadata-only entries with dummy embeddings)
    local indexedUUIDs, err = SearchIndexAPI.getAllIndexedPhotoUUIDs(requireEmbeddings)
    if err then
        ErrorHandler.handleError("Failed to retrieve indexed photos", err)
        return false, {}
    end

    local allPhotos = PhotoSelector.getPhotosInScope('all')

    if allPhotos == nil then
        ErrorHandler.handleError("No photos found in catalog", "Something went wrong")
        return false, {}
    end

    local photosToProcess = {}
    
    for i, photo in ipairs(allPhotos) do
        local uuid = photo:getRawMetadata("uuid")
        if not Util.table_contains(indexedUUIDs, uuid) then
            table.insert(photosToProcess, photo)
        end
    end

    return true, photosToProcess
end


function SearchIndexAPI.saveThumbnail(uuid, faceIndex, base64Data)
    local tempDir = LrPathUtils.getStandardFilePath('temp')
    local tempFile = LrPathUtils.child(tempDir, uuid .. "_" .. faceIndex ..  ".jpg")
    local f = io.open(tempFile, "wb")
    if f then
        f:write(LrStringUtils.decodeBase64(base64Data))
        f:close()
        log:trace("Saved face thumbnail to: " .. tempFile)
        return tempFile
    end
    return nil
end
---
-- Retrieves all available multimodal models from all providers.
-- Always filters to vision-capable models only.
-- Dynamically checks Ollama and LM Studio availability on each call.
-- @param openaiApiKey string|nil OpenAI API key for listing ChatGPT models
-- @param geminiApiKey string|nil Gemini API key for listing Gemini models
-- @return table|nil Response from server with format: { models = { qwen = {...}, ollama = {...}, ... } }
function SearchIndexAPI.getModels(openaiApiKey, geminiApiKey)
    local url = BASE_URL .. ENDPOINTS.MODELS
    local body = { 
        openai_apikey = openaiApiKey, 
        gemini_apikey = geminiApiKey 
    }
    local result, err = _request('POST', url, body)
    if err then
        log:error("getModels failed: " .. err)
        return nil
    end
    return result
end



function SearchIndexAPI.startClipDownload()

    if SearchIndexAPI.isClipReady() then
        log:trace("CLIP model is already cached")
        return
    end

    local status, err = _request('GET', BASE_URL .. ENDPOINTS.STATUS_CLIP_DOWNLOAD)
    if not err and status ~= nil and status.status == "downloading" then
        log:trace("CLIP model download is already in progress")
        return
    end

    local progressScope = LrProgressScope({
        title = LOC "$$$/LrGeniusAI/ClipDownload/ProgressTitle=Downloading CLIP AI model for advanced search",
        functionContext = nil,
    })

    local url = BASE_URL .. ENDPOINTS.START_CLIP_DOWNLOAD
    local body = {}

    local res, err = _request('POST', url, body)

    if err then
        log:error("startClipDownload failed: " .. err)
        return nil, err
    end

    LrTasks.startAsyncTask(function()
        while true do
            local status, err = _request('GET', BASE_URL .. ENDPOINTS.STATUS_CLIP_DOWNLOAD)
            if err then
                ErrorHandler.handleError("Error downloading CLIP model", err)
                if progressScope ~= nil then
                    progressScope:setCaption(LOC "$$$/LrGeniusAI/ClipDownload/Error=Error downloading CLIP model: ^1", err)
                    progressScope:done()
                end
                break
            end

            if status ~= nil then
                if progressScope ~= nil then
                    progressScope:setCaption(LOC "$$$/LrGeniusAI/ClipDownload/Downloading=Downloading CLIP model...")
                end
                if status.status == "downloading" then
                    progressScope:setPortionComplete(status.progress, status.total)
                elseif status.status == "completed" then
                    log:trace("CLIP model download completed")
                    progressScope:done()
                    break
                end
            end

            LrTasks.sleep(2)
        end
    end)
end


function SearchIndexAPI.isClipReady()
    local url = BASE_URL .. ENDPOINTS.CLIP_STATUS
    local res, err = _request('GET', url)
    if err then
        log:error("isClipReady failed: " .. err)
        return false, err
    end
    if res ~= nil then
        if res.clip == "ready" then
            log:trace("CLIP model is ready")
            return true, res.message
        else
            log:trace("CLIP model is not ready")
            return false, res.message
        end
    end
    log:error("isClipReady: Unknown error")
    return false, "Unknown error"
end