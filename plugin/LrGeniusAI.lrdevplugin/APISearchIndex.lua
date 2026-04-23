-- lrgenius-server API Wrapper
-- Provides functions to interact with the Python-based search index server.

SearchIndexAPI = {}

local function getBaseUrl()
	local url = (prefs and prefs.backendServerUrl) and prefs.backendServerUrl or ""
	url = url:gsub("^%s*(.-)%s*$", "%1") -- trim whitespace
	if url == "" then
		return "http://127.0.0.1:19819"
	end
	-- Ensure URL has protocol
	if not url:match("^https?://") then
		url = "http://" .. url
	end
	-- Remove trailing slash for consistency
	url = url:gsub("/+$", "")
	return url
end

function SearchIndexAPI.isLocalBackend()
	local url = getBaseUrl()
	return url:match("^https?://127%.0%.0%.1:") or url:match("^https?://localhost:")
end

local ENDPOINTS = {
	INDEX = "/index",
	EDIT = "/edit",
	INDEX_BY_REFERENCE = "/index_by_reference",
	INDEX_BASE64 = "/index_base64",
	EDIT_BASE64 = "/edit_base64",
	GROUP_SIMILAR = "/group_similar",
	CULL = "/cull",
	FIND_SIMILAR = "/find_similar",
	SEARCH = "/search",
	STATS = "/db/stats",
	MODELS = "/models",
	GET_IDS = "/get/ids",
	REMOVE = "/remove",
	REMOVE_METADATA = "/remove/metadata",
	PING = "/ping",
	VERSION = "/version",
	VERSION_CHECK = "/version/check",
	SHUTDOWN = "/shutdown",
	UNLOAD = "/unload",
	IMPORT_METADATA = "/import/metadata",
	START_CLIP_DOWNLOAD = "/clip/download/start",
	STATUS_CLIP_DOWNLOAD = "/clip/download/status",
	CLIP_STATUS = "/clip/status",
	CHECK_UNPROCESSED = "/index/check-unprocessed",
	FACES_CLUSTER = "/faces/cluster",
	FACES_PERSONS = "/faces/persons",
	FACES_PERSON_PHOTOS = "/faces/persons", -- suffix /<id>/photos
	FACES_DETECT = "/faces/detect",
	FACES_QUERY = "/faces/query",
	MIGRATE_PHOTO_IDS = "/db/migrate-photo-ids",
	DB_BACKUP = "/db/backup",
	SYNC_CLEANUP = "/sync/cleanup",
	SYNC_CLAIM = "/sync/claim",
	TRAINING_ADD = "/training/add",
	TRAINING_LIST = "/training/list",
	TRAINING_COUNT = "/training/count",
	TRAINING_DELETE = "/training", -- DELETE /training/<photo_id>
	TRAINING_CLEAR = "/training", -- DELETE /training (all)
	TRAINING_STATS = "/training/stats",
	STYLE_EDIT = "/style_edit",
	LOGS = "/logs",
	LOGS_RAW = "/logs/raw",
	INITIALIZE = "/initialize",
	RESTART = "/restart",
	HEALTH = "/health",
}

local EXPORT_SETTINGS = {
	LR_export_destinationType = "specificFolder",
	LR_export_useSubfolder = false,
	LR_format = "JPEG",
	LR_jpeg_quality = tonumber(prefs.exportQuality) or 60,
	LR_minimizeEmbeddedMetadata = false,
	LR_outputSharpeningOn = false,
	LR_size_doConstrain = true,
	LR_size_maxHeight = tonumber(prefs.exportSize) or 1024,
	LR_size_resizeType = "longEdge",
	LR_size_units = "pixels",
	LR_collisionHandling = "rename",
	LR_includeVideoFiles = false,
	LR_removeLocationMetadata = true,
	LR_embeddedMetadataOption = "all",
}

-- Forward declarations for private helper functions
local _request
local _requestMultipart

-- Returns a string safe for logging; never passes a table to tostring (avoids "table: 0x...").
local function httpStatusForLog(status, hdrs)
	if type(status) == "number" then
		return tostring(status)
	end
	if type(hdrs) == "number" then
		return tostring(hdrs)
	end
	if type(hdrs) == "table" then
		local s = hdrs.status or hdrs.statusCode
		if type(s) == "number" then
			return tostring(s)
		end
		if type(s) == "string" then
			return s
		end
	end
	return "unknown"
end

-- Catalog DB migrations: one-time backend operations per catalog (e.g. claim_photos after cross-catalog soft state).
-- Each entry: { id = "unique_id", run = function(progressScope) return ok, err [, userMessage] end }. progressScope is optional (nil for migrations that don't need it). Optional userMessage is shown via LrDialogs when present.
local CATALOG_DB_MIGRATIONS = {
	{
		id = "claim_photos_v1",
		run = function(progressScope)
			local ok, err, result = SearchIndexAPI.claimPhotosForCatalog(progressScope)
			local msg = (ok and result and (result.claimed or 0) >= 0)
					and (LOC(
						"$$$/LrGeniusAI/SearchIndexAPI/PhotosClaimedCount=^1 photos claimed for this catalog.",
						tostring(result.claimed or 0)
					))
				or nil
			return ok, err, msg
		end,
	},
	-- Add future migrations here, e.g. { id = "some_breaking_change_v1", run = function(progressScope) ... return ok, err [, userMessage] end },
}

local MIGRATION_IN_PROGRESS_PREFIX = "in_progress"
-- A live migration task re-writes its marker every MIGRATION_HEARTBEAT_INTERVAL_SECONDS seconds.
-- Anything older than STALE_IN_PROGRESS_SECONDS is assumed orphaned (crashed/killed task).
-- Keep the stale threshold several multiples of the heartbeat so a brief scheduler hiccup
-- doesn't cause a parallel migration to start.
local MIGRATION_HEARTBEAT_INTERVAL_SECONDS = 30
local STALE_IN_PROGRESS_SECONDS = 120
-- LrC caches this module per plugin-session; any `in_progress` marker whose timestamp predates
-- SESSION_START_TIME was written by a prior process and its owning task no longer exists.
local SESSION_START_TIME = LrDate.currentTime()
-- In-session guard: short-circuits re-entry within the same plugin session, regardless of
-- what's persisted in the plugin property. Survives nothing; exists only to defend against
-- logic bugs where the property check doesn't fire fast enough.
local _migrationTaskRunning = false

local function formatInProgressMarker()
	return MIGRATION_IN_PROGRESS_PREFIX .. ":" .. tostring(math.floor(LrDate.currentTime()))
end

-- Rewrites the in_progress:<ts> marker with a current timestamp so the stale-detection check
-- doesn't evict a long-running live migration. No-op if the marker is no longer present
-- (e.g., the migration task just finished and stripped it).
local function updateInProgressHeartbeat(catalog)
	catalog:withPrivateWriteAccessDo(function()
		local cur = catalog:getPropertyForPlugin(_PLUGIN, "catalogDbMigrations") or ""
		local fresh = formatInProgressMarker()
		local updated, n = cur:gsub(MIGRATION_IN_PROGRESS_PREFIX .. ":%d+", fresh, 1)
		if n > 0 and updated ~= cur then
			catalog:setPropertyForPlugin(_PLUGIN, "catalogDbMigrations", updated)
		end
	end)
end

local function shouldUseGlobalPhotoId()
	return prefs and prefs.useGlobalPhotoId ~= false
end

local function parseCompletedMigrations(raw)
	local completed = {}
	local inProgress = false
	local inProgressSince = nil
	if raw and raw ~= "" then
		for part in string.gmatch(raw, "([^,]+)") do
			part = part:match("^%s*(.-)%s*$") or part
			if part == MIGRATION_IN_PROGRESS_PREFIX then
				-- Legacy unversioned marker (pre-timestamp plugin version): treat as stale.
				inProgress = true
				inProgressSince = inProgressSince or 0
			else
				local ts = part:match("^" .. MIGRATION_IN_PROGRESS_PREFIX .. ":(%d+)$")
				if ts then
					inProgress = true
					inProgressSince = tonumber(ts) or 0
				else
					completed[part] = true
				end
			end
		end
	end
	return completed, inProgress, inProgressSince
end

local function isInProgressStale(inProgressSince)
	if not inProgressSince then
		return false
	end
	-- 0 is reserved for legacy unversioned markers — always stale.
	if inProgressSince == 0 then
		return true
	end
	if inProgressSince < SESSION_START_TIME then
		return true
	end
	if (LrDate.currentTime() - inProgressSince) > STALE_IN_PROGRESS_SECONDS then
		return true
	end
	return false
end

local function stripInProgressMarkers(raw)
	if not raw or raw == "" then
		return ""
	end
	local cleaned = raw:gsub(MIGRATION_IN_PROGRESS_PREFIX .. ":%d+", "")
		:gsub(MIGRATION_IN_PROGRESS_PREFIX, "")
		:gsub(",+", ",")
		:gsub("^,", "")
		:gsub(",$", "")
		:gsub("^%s*(.-)%s*$", "%1")
	return cleaned
end

--- Ensures all registered catalog DB migrations have been run for the active catalog. Runs pending ones in background; uses catalog plugin property catalogDbMigrations so each migration runs once per catalog.
local function ensureDbMigrationsDone()
	local catalog = LrApplication.activeCatalog()
	if not catalog then
		return
	end
	if _migrationTaskRunning then
		return
	end
	local raw = catalog:getPropertyForPlugin(_PLUGIN, "catalogDbMigrations") or ""
	local completed, inProgress, inProgressSince = parseCompletedMigrations(raw)

	-- Recover from crashed/killed prior migrations that left the marker poisoned.
	if inProgress and isInProgressStale(inProgressSince) then
		local age = inProgressSince and (LrDate.currentTime() - inProgressSince) or -1
		log:warn(
			"Clearing stale catalogDbMigrations in_progress marker (age="
				.. tostring(math.floor(age))
				.. "s, pre_session="
				.. tostring(inProgressSince and inProgressSince < SESSION_START_TIME)
				.. ")"
		)
		catalog:withPrivateWriteAccessDo(function()
			local cur = catalog:getPropertyForPlugin(_PLUGIN, "catalogDbMigrations") or ""
			catalog:setPropertyForPlugin(_PLUGIN, "catalogDbMigrations", stripInProgressMarkers(cur))
		end)
		raw = catalog:getPropertyForPlugin(_PLUGIN, "catalogDbMigrations") or ""
		completed, inProgress = parseCompletedMigrations(raw)
	end

	if inProgress then
		return
	end
	local pending = {}
	for _, m in ipairs(CATALOG_DB_MIGRATIONS) do
		if not completed[m.id] then
			pending[#pending + 1] = m
		end
	end
	if #pending == 0 then
		return
	end
	catalog:withPrivateWriteAccessDo(function()
		local marker = formatInProgressMarker()
		local newRaw = (raw == "" or raw:match("%S") == nil) and marker or (raw .. "," .. marker)
		catalog:setPropertyForPlugin(_PLUGIN, "catalogDbMigrations", newRaw)
	end)
	_migrationTaskRunning = true
	local heartbeatStop = false

	-- Heartbeat task: periodically refreshes the in_progress:<ts> timestamp so a long-running
	-- migration isn't mistaken for a crashed one. Sleeps in 1-second chunks so it can exit
	-- quickly once the main task finishes (avoids a race where it re-writes the marker after
	-- cleanup has already stripped it).
	LrTasks.startAsyncTask(function()
		while not heartbeatStop do
			for _ = 1, MIGRATION_HEARTBEAT_INTERVAL_SECONDS do
				if heartbeatStop then
					return
				end
				LrTasks.sleep(1)
			end
			if heartbeatStop then
				return
			end
			updateInProgressHeartbeat(catalog)
		end
	end)

	LrTasks.startAsyncTask(function()
		local runOk, runErr = LrTasks.pcall(function()
			local done = raw
			for _, m in ipairs(pending) do
				local progressScope
				if m.id == "claim_photos_v1" then
					progressScope = LrProgressScope({
						title = LOC("$$$/LrGeniusAI/SearchIndexAPI/claimingPhotos=Claiming photos for this catalog..."),
						functionContext = nil,
					})
				end
				local ok, err, userMessage
				if type(m.run) == "function" then
					local status, a, b, c
					if type(LrTasks) == "table" and type(LrTasks.pcall) == "function" then
						status, a, b, c = LrTasks.pcall(function()
							return m.run(progressScope)
						end)
					else
						status, a, b, c = LrTasks.pcall(function()
							return m.run(progressScope)
						end)
					end
					if status then
						ok, err, userMessage = a, b, c
					else
						ok, err, userMessage = false, tostring(a), nil
					end
				end
				if ok then
					done = (done == "" or done:match("%S") == nil) and m.id or (done .. "," .. m.id)
					catalog:withPrivateWriteAccessDo(function()
						catalog:setPropertyForPlugin(_PLUGIN, "catalogDbMigrations", done)
					end)
					log:info("Catalog DB migration completed: " .. tostring(m.id))
					if userMessage and userMessage ~= "" then
						LrDialogs.message(
							LOC("$$$/LrGeniusAI/PluginInfo/ClaimPhotosTitle=Claim photos"),
							userMessage,
							"info"
						)
					end
				else
					log:warn("Catalog DB migration failed: " .. tostring(m.id) .. " - " .. tostring(err))
					if m.id == "claim_photos_v1" then
						LrDialogs.message(
							LOC("$$$/LrGeniusAI/PluginInfo/ClaimPhotosFailed=Claim photos failed"),
							tostring(err or LOC("$$$/LrGeniusAI/common/UnknownError=Unknown error"))
								.. "\n\n"
								.. LOC(
									"$$$/LrGeniusAI/SearchIndexAPI/ClaimPhotosRetryHint=You can try again from Plug-in Manager → LrGeniusAI → Backend Server → Claim photos for this catalog."
								),
							"critical"
						)
					end
				end
				if progressScope then
					progressScope:done()
				end
			end
			catalog:withPrivateWriteAccessDo(function()
				local current = catalog:getPropertyForPlugin(_PLUGIN, "catalogDbMigrations") or ""
				catalog:setPropertyForPlugin(_PLUGIN, "catalogDbMigrations", stripInProgressMarkers(current))
			end)
		end)
		heartbeatStop = true
		_migrationTaskRunning = false
		if not runOk then
			log:error("Catalog DB migration task crashed: " .. tostring(runErr))
		end
	end)
end

local function allCatalogDbMigrationsCompleted(completed)
	for _, m in ipairs(CATALOG_DB_MIGRATIONS) do
		if not completed[m.id] then
			return false
		end
	end
	return true
end

--- Waits for catalog-scoped DB migrations (tracked by `catalogDbMigrations`) to complete.
--- This is important because backend operations (e.g. photo claiming visibility) can race if we start
--- indexing before `claim_photos_v1` finishes.
--- @param timeoutSeconds number
--- @return boolean success (all migrations completed)
local function waitForCatalogDbMigrationsDone(timeoutSeconds)
	local catalog = LrApplication.activeCatalog()
	if not catalog then
		return false
	end

	timeoutSeconds = tonumber(timeoutSeconds) or 600
	local start = LrDate.currentTime()
	local sawInProgress = false

	while (LrDate.currentTime() - start) < timeoutSeconds do
		local raw = catalog:getPropertyForPlugin(_PLUGIN, "catalogDbMigrations") or ""
		local completed, inProgress, inProgressSince = parseCompletedMigrations(raw)

		if allCatalogDbMigrationsCompleted(completed) then
			return true
		end

		if inProgress then
			-- A stale marker means no task is actually running — don't block waiting for a ghost.
			if isInProgressStale(inProgressSince) then
				log:warn("waitForCatalogDbMigrationsDone: stale in_progress marker detected, aborting wait")
				return false
			end
			sawInProgress = true
		elseif sawInProgress then
			-- Previously observed in_progress, now gone but not all completed → migration failed.
			return false
		end

		LrTasks.sleep(0.5)
	end

	return false
end

--- Returns the stable catalog identifier for the active catalog (for backend catalog-scoped operations).
local function getCatalogIdValue()
	local id, err = Util.getCatalogIdentifier()
	if not id then
		log:warn("getCatalogId: " .. tostring(err))
		return nil
	end
	return id
end

local function getCatalogId()
	local id = getCatalogIdValue()
	if not id then
		return nil
	end
	ensureDbMigrationsDone()
	-- Block until the background catalog DB migrations (including photo claiming) finish.
	-- Prevents backend requests from failing when the catalog hasn't been fully "claimed" yet.
	local ok = waitForCatalogDbMigrationsDone(tonumber(prefs and prefs.dbMigrationWaitTimeoutSeconds) or 600)
	if not ok then
		log:warn("getCatalogId: timed out or failed waiting for catalogDbMigrations to complete")
	end
	return id
end

local function getPhotoIdForPhoto(photo)
	if not photo then
		return nil, "Photo is nil"
	end
	if shouldUseGlobalPhotoId() then
		return Util.getGlobalPhotoIdForPhoto(photo, {
			windowBytes = Util.getDefaultPartialHashWindowBytes(),
		})
	end
	local uuid = photo:getRawMetadata("uuid")
	if not uuid or uuid == "" then
		return nil, "Photo UUID is missing"
	end
	return uuid, nil
end

function SearchIndexAPI.getPhotoIdForPhoto(photo)
	return getPhotoIdForPhoto(photo)
end

function SearchIndexAPI.findPhotoByPhotoId(photoId)
	if not photoId or photoId == "" then
		return nil
	end

	local catalog = LrApplication.activeCatalog()
	if not shouldUseGlobalPhotoId() then
		return catalog:findPhotoByUuid(photoId)
	end

	for _, photo in ipairs(catalog:getAllPhotos()) do
		local cachedId = photo:getPropertyForPlugin(_PLUGIN, "globalPhotoId")
		if cachedId == photoId then
			return photo
		end
	end

	for _, photo in ipairs(catalog:getAllPhotos()) do
		local candidateId = getPhotoIdForPhoto(photo)
		if candidateId == photoId then
			return photo
		end
	end

	return nil
end

function SearchIndexAPI.findPhotosByPhotoIds(photoIds)
	local photos = {}
	if type(photoIds) ~= "table" or #photoIds == 0 then
		return photos
	end

	local catalog = LrApplication.activeCatalog()
	if not shouldUseGlobalPhotoId() then
		for _, photoId in ipairs(photoIds) do
			local photo = catalog:findPhotoByUuid(photoId)
			if photo then
				table.insert(photos, photo)
			else
				log:warn(
					"findPhotosByPhotoIds: Photo with UUID "
						.. tostring(photoId)
						.. " not found in catalog (non-global IDs)."
				)
			end
		end
		return photos
	end

	local idSet = {}
	for _, photoId in ipairs(photoIds) do
		idSet[photoId] = true
	end

	local photoById = {}
	local startedAt = LrDate.currentTime()
	local allPhotos = catalog:getAllPhotos()
	local allPhotosElapsed = math.floor((LrDate.currentTime() - startedAt) * 1000)
	log:trace(
		"findPhotosByPhotoIds: catalog:getAllPhotos() returned "
			.. tostring(#allPhotos)
			.. " photos in "
			.. tostring(allPhotosElapsed)
			.. "ms"
	)

	for _, photo in ipairs(allPhotos) do
		local cachedId = photo:getPropertyForPlugin(_PLUGIN, "globalPhotoId")
		if cachedId and idSet[cachedId] and not photoById[cachedId] then
			photoById[cachedId] = photo
		end
	end

	for _, photoId in ipairs(photoIds) do
		local photo = photoById[photoId]
		if photo then
			table.insert(photos, photo)
		else
			log:warn("findPhotosByPhotoIds: Photo with global ID " .. tostring(photoId) .. " not found in catalog.")
		end
	end

	return photos
end

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

	local tempDir = LrPathUtils.getStandardFilePath("temp")
	local photoName = LrPathUtils.leafName(photo:getFormattedMetadata("fileName"))

	EXPORT_SETTINGS.LR_export_destinationPathPrefix = tempDir

	local exportSession = LrExportSession({
		photosToExport = { photo },
		exportSettings = EXPORT_SETTINGS,
	})

	local resultPath = nil
	for _, rendition in exportSession:renditions() do
		local success, path = rendition:waitForRender()
		log:trace(
			"Export completed for photo: "
				.. photoName
				.. " Success: "
				.. tostring(success)
				.. " Path: "
				.. tostring(path)
		)
		if success then -- Export successful
			resultPath = path
		else
			-- Error during export
			log:error("Failed to export photo for indexing. " .. (path or "unknown error"))
			resultPath = nil
		end
	end
	return resultPath
end

function SearchIndexAPI.exportPhotosForIndexing(photos)
	if not photos or #photos == 0 then
		return {}
	end

	local tempDir = LrPathUtils.getStandardFilePath("temp")

	EXPORT_SETTINGS.LR_export_destinationPathPrefix = tempDir

	local exportSession = LrExportSession({
		photosToExport = photos,
		exportSettings = EXPORT_SETTINGS,
	})

	local photoPaths = {}
	local photoIndex = 1
	for _, rendition in exportSession:renditions() do
		local success, path = rendition:waitForRender()
		local photo = photos[photoIndex]
		if photo ~= nil then
			local photoName = LrPathUtils.leafName(photo:getFormattedMetadata("fileName"))
			log:trace(
				"Export completed for photo: "
					.. photoName
					.. " Success: "
					.. tostring(success)
					.. " Path: "
					.. tostring(path)
			)
			if success then
				photoPaths[photo] = path
			else
				log:error("Failed to export photo for indexing. " .. (path or "unknown error"))
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
-- Gets a JPEG thumbnail from Lightroom's preview system (must be called from LrTasks async context).
-- Uses photo:requestJpegThumbnail(width, height, callback) and waits for the callback with a timeout.
-- @param photo LrPhoto
-- @param minWidth number Minimum width (long edge); nil = smallest preview.
-- @param minHeight number Optional; if minWidth is set, controls height of returned pixels.
-- @param requestState table|nil Optional state/config with timeoutSeconds.
-- @return string|nil JPEG data string, or nil on failure.
-- @return string|nil Error message when JPEG is nil.
--
function SearchIndexAPI.getJpegThumbnailForPhoto(photo, minWidth, minHeight, requestState)
	if not photo then
		return nil, "Photo is nil"
	end
	local result = nil
	local errResult = nil
	local done = false
	local callbackCount = 0
	local timeoutSeconds = tonumber(requestState and requestState.timeoutSeconds)
		or tonumber(prefs and prefs.previewThumbnailTimeoutSeconds)
		or 12
	local deadline = LrDate.currentTime() + timeoutSeconds

	local callback = function(jpegData, err)
		callbackCount = callbackCount + 1

		-- Adobe reports that the callback may fire more than once. Prefer the
		-- first non-empty JPEG payload and otherwise keep waiting until timeout.
		if jpegData and type(jpegData) == "string" and #jpegData > 0 then
			result = jpegData
			errResult = nil
			done = true
			return
		end

		if err and err ~= "" then
			errResult = err
		elseif not errResult then
			errResult = "No thumbnail data"
		end
	end

	local requestObj = photo:requestJpegThumbnail(minWidth, minHeight, callback)
	if not requestObj then
		return nil, "requestJpegThumbnail failed to start"
	end

	while not done and LrDate.currentTime() < deadline do
		if MAC_ENV then
			LrTasks.yield()
		else
			LrTasks.sleep(0.05)
		end
	end

	if not done then
		return nil,
			string.format("Thumbnail request timed out after %.1fs (callbacks=%d)", timeoutSeconds, callbackCount)
	end
	if result and type(result) == "string" and #result > 0 then
		return result, nil
	end
	return nil, errResult or "No thumbnail data"
end

---
-- Analyzes and indexes a single photo using base64-encoded JPEG (e.g. from requestJpegThumbnail).
-- Uses the /index_base64 endpoint; same options as analyzeAndIndexPhoto.
-- @param photoId string
-- @param jpegData string Raw JPEG bytes.
-- @param filename string Display filename for logging.
-- @param options table Same as analyzeAndIndexPhoto.
-- @return boolean success, table|string response or error.
--
function SearchIndexAPI.analyzeAndIndexPhotoBase64(photoId, jpegData, filename, options)
	if not jpegData or type(jpegData) ~= "string" or #jpegData == 0 then
		log:error("analyzeAndIndexPhotoBase64: no JPEG data")
		return false, "No image data provided"
	end
	if not photoId or photoId == "" then
		log:error("Photo ID is missing")
		return false, "No photo ID provided"
	end

	options = options or {}
	local base64Image = LrStringUtils.encodeBase64(jpegData)
	local url = getBaseUrl() .. ENDPOINTS.INDEX_BASE64

	local body = {
		image = base64Image,
		photo_id = photoId,
		filename = filename or "photo.jpg",
		catalog_id = getCatalogId(),
		tasks = options.tasks or {},
		provider = options.provider,
		model = options.model,
		api_key = options.api_key,
		language = options.language or (prefs and prefs.generateLanguage) or "English",
		temperature = tostring(options.temperature or (prefs and prefs.temperature) or 0.2),
		replace_ss = tostring(options.replace_ss or false),
		generate_keywords = tostring(options.generate_keywords or false),
		generate_caption = tostring(options.generate_caption or false),
		generate_title = tostring(options.generate_title or false),
		generate_alt_text = tostring(options.generate_alt_text or false),
		submit_gps = tostring(options.submit_gps or false),
		submit_keywords = tostring(options.submit_keywords or false),
		submit_folder_names = tostring(options.submit_folder_names or false),
		user_context = options.user_context,
		gps_coordinates = options.gps_coordinates and JSON:encode(options.gps_coordinates) or nil,
		existing_keywords = options.existing_keywords and JSON:encode(options.existing_keywords) or nil,
		folder_names = options.folder_names,
		prompt = options.prompt,
		keyword_categories = options.keyword_categories and JSON:encode(options.keyword_categories) or "[]",
		bilingual_keywords = tostring(options.bilingual_keywords or false),
		keyword_secondary_language = options.keyword_secondary_language
			or (prefs and prefs.keywordSecondaryLanguage)
			or "English",
		date_time = options.date_time,
		ollama_base_url = options.ollama_base_url or (prefs and prefs.ollamaBaseUrl),
		lmstudio_base_url = options.lmstudio_base_url or (prefs and prefs.lmstudioBaseUrl),
		vertex_project_id = options.vertex_project_id,
		vertex_location = options.vertex_location,
		regenerate_metadata = tostring(options.regenerate_metadata ~= false),
	}

	log:trace("Analyzing and indexing photo (base64): " .. tostring(filename) .. " id " .. photoId)

	local response, err = _request("POST", url, body, 720)

	if not response then
		log:error("Failed to analyze/index photo (base64): " .. tostring(err))
		return false, err or "Unknown error"
	end
	if response.status == "processed" then
		local success_count = response.success_count or 0
		if success_count > 0 then
			log:trace("Successfully processed photo (base64): " .. tostring(filename))
			return true, response
		else
			log:error("Photo processing failed (base64): " .. tostring(filename))
			return false, response.error or "Processing failed"
		end
	end
	log:error("Unexpected response status (base64): " .. tostring(response.status))
	return false, "Unexpected response status"
end

---
-- Unified function to analyze and index photos with metadata and embeddings.
-- Replaces the old separate analyze and index workflows.
-- @param photoId string The ID of the photo.
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

function SearchIndexAPI.generateEditRecipePhoto(photoId, filepath, options)
	if filepath == nil then
		log:error("generateEditRecipePhoto: JPEG is nil")
		return false, "No image data provided"
	end
	if not photoId or photoId == "" then
		log:error("generateEditRecipePhoto: Photo ID is missing")
		return false, "No photo ID provided"
	end

	local filename = LrPathUtils.leafName(filepath)
	options = options or {}
	local url = getBaseUrl() .. ENDPOINTS.EDIT
	local mimeChunks = {}

	table.insert(mimeChunks, { name = "photo_id", value = photoId })
	local cid = getCatalogId()
	if cid then
		table.insert(mimeChunks, { name = "catalog_id", value = cid })
	end
	if options.provider then
		table.insert(mimeChunks, { name = "provider", value = options.provider })
	end
	if options.model then
		table.insert(mimeChunks, { name = "model", value = options.model })
	end
	if options.api_key then
		table.insert(mimeChunks, { name = "api_key", value = options.api_key })
	end

	table.insert(mimeChunks, { name = "language", value = options.language or prefs.generateLanguage or "English" })
	table.insert(
		mimeChunks,
		{ name = "temperature", value = tostring(options.temperature or prefs.temperature or 0.2) }
	)
	table.insert(mimeChunks, { name = "submit_gps", value = tostring(options.submit_gps or false) })
	table.insert(mimeChunks, { name = "submit_keywords", value = tostring(options.submit_keywords or false) })
	table.insert(mimeChunks, { name = "submit_folder_names", value = tostring(options.submit_folder_names or false) })
	table.insert(mimeChunks, { name = "include_masks", value = tostring(options.include_masks ~= false) })
	if options.user_context then
		table.insert(mimeChunks, { name = "user_context", value = options.user_context })
	end
	if options.edit_intent then
		table.insert(mimeChunks, { name = "edit_intent", value = options.edit_intent })
	end
	if options.gps_coordinates then
		table.insert(mimeChunks, { name = "gps_coordinates", value = JSON:encode(options.gps_coordinates) })
	end
	if options.existing_keywords then
		table.insert(mimeChunks, { name = "existing_keywords", value = JSON:encode(options.existing_keywords) })
	end
	if options.folder_names then
		table.insert(mimeChunks, { name = "folder_names", value = options.folder_names })
	end
	if options.prompt then
		table.insert(mimeChunks, { name = "prompt", value = options.prompt })
	end
	if options.date_time then
		table.insert(mimeChunks, { name = "date_time", value = options.date_time })
	end
	if options.ollama_base_url or (prefs and prefs.ollamaBaseUrl) then
		table.insert(mimeChunks, { name = "ollama_base_url", value = options.ollama_base_url or prefs.ollamaBaseUrl })
	end
	if options.lmstudio_base_url or (prefs and prefs.lmstudioBaseUrl) then
		table.insert(
			mimeChunks,
			{ name = "lmstudio_base_url", value = options.lmstudio_base_url or prefs.lmstudioBaseUrl }
		)
	end

	table.insert(mimeChunks, {
		name = "image",
		fileName = filename,
		filePath = filepath,
		contentType = "image/jpeg",
	})

	log:trace("Generating AI edit recipe for photo: " .. filename .. " with id " .. photoId)
	local response, err = _requestMultipart(url, mimeChunks, 720)
	if not response then
		log:error("Failed to generate AI edit recipe: " .. tostring(err))
		return false, err or "Unknown error"
	end
	if type(response) ~= "table" then
		log:error(
			"AI edit recipe response has unexpected type: "
				.. tostring(type(response))
				.. " value="
				.. tostring(response)
		)
		return false, "Invalid response type from /edit endpoint: " .. tostring(type(response))
	end
	if response.status == "success" then
		return true, response
	end
	log:error("Unexpected response status for AI edit recipe: " .. tostring(response.status))
	return false, response.error or "Unexpected response status"
end

function SearchIndexAPI.analyzeAndIndexPhoto(photoId, filepath, options)
	if filepath == nil then
		log:error("JPEG is nil")
		return false, "No image data provided"
	end
	if not photoId or photoId == "" then
		log:error("Photo ID is missing")
		return false, "No photo ID provided"
	end

	local filename = LrPathUtils.leafName(filepath)

	options = options or {}

	local url = getBaseUrl() .. ENDPOINTS.INDEX

	-- Prepare multipart content chunks
	local mimeChunks = {}

	-- Add form fields
	table.insert(mimeChunks, { name = "photo_id", value = photoId })
	local cid = getCatalogId()
	if cid then
		table.insert(mimeChunks, { name = "catalog_id", value = cid })
	end
	table.insert(mimeChunks, { name = "tasks", value = JSON:encode(options.tasks or {}) })

	if options.provider then
		table.insert(mimeChunks, { name = "provider", value = options.provider })
	end
	if options.model then
		table.insert(mimeChunks, { name = "model", value = options.model })
	end
	if options.api_key then
		table.insert(mimeChunks, { name = "api_key", value = options.api_key })
	end

	table.insert(mimeChunks, { name = "language", value = options.language or prefs.generateLanguage or "English" })
	table.insert(
		mimeChunks,
		{ name = "temperature", value = tostring(options.temperature or prefs.temperature or 0.2) }
	)
	table.insert(mimeChunks, { name = "replace_ss", value = tostring(options.replace_ss or false) })

	-- Metadata generation options
	table.insert(mimeChunks, { name = "generate_keywords", value = tostring(options.generate_keywords or false) })
	table.insert(mimeChunks, { name = "generate_caption", value = tostring(options.generate_caption or false) })
	table.insert(mimeChunks, { name = "generate_title", value = tostring(options.generate_title or false) })
	table.insert(mimeChunks, { name = "generate_alt_text", value = tostring(options.generate_alt_text or false) })

	-- Context options
	table.insert(mimeChunks, { name = "submit_gps", value = tostring(options.submit_gps or false) })
	table.insert(mimeChunks, { name = "submit_keywords", value = tostring(options.submit_keywords or false) })
	table.insert(mimeChunks, { name = "submit_folder_names", value = tostring(options.submit_folder_names or false) })

	if options.user_context then
		table.insert(mimeChunks, { name = "user_context", value = options.user_context })
	end
	if options.gps_coordinates then
		table.insert(mimeChunks, { name = "gps_coordinates", value = JSON:encode(options.gps_coordinates) })
	end
	if options.existing_keywords then
		table.insert(mimeChunks, { name = "existing_keywords", value = JSON:encode(options.existing_keywords) })
	end
	if options.folder_names then
		table.insert(mimeChunks, { name = "folder_names", value = options.folder_names })
	end
	if options.prompt then
		table.insert(mimeChunks, { name = "prompt", value = options.prompt })
	end

	table.insert(mimeChunks, { name = "keyword_categories", value = JSON:encode(options.keyword_categories or {}) })
	table.insert(mimeChunks, { name = "bilingual_keywords", value = tostring(options.bilingual_keywords or false) })
	table.insert(mimeChunks, {
		name = "keyword_secondary_language",
		value = options.keyword_secondary_language or (prefs and prefs.keywordSecondaryLanguage) or "English",
	})

	if options.date_time then
		table.insert(mimeChunks, { name = "date_time", value = options.date_time })
	end
	if options.ollama_base_url or (prefs and prefs.ollamaBaseUrl) then
		table.insert(mimeChunks, { name = "ollama_base_url", value = options.ollama_base_url or prefs.ollamaBaseUrl })
	end
	if options.lmstudio_base_url or (prefs and prefs.lmstudioBaseUrl) then
		table.insert(
			mimeChunks,
			{ name = "lmstudio_base_url", value = options.lmstudio_base_url or prefs.lmstudioBaseUrl }
		)
	end
	if options.vertex_project_id and options.vertex_project_id ~= "" then
		table.insert(mimeChunks, { name = "vertex_project_id", value = options.vertex_project_id })
	end
	if options.vertex_location and options.vertex_location ~= "" then
		table.insert(mimeChunks, { name = "vertex_location", value = options.vertex_location })
	end

	-- Regeneration control: if false, server will only fill missing fields
	table.insert(mimeChunks, { name = "regenerate_metadata", value = tostring(options.regenerate_metadata ~= false) })

	-- Add file
	table.insert(mimeChunks, {
		name = "image",
		fileName = filename,
		filePath = filepath,
		contentType = "image/jpeg",
	})

	log:trace(
		"Analyzing and indexing photo: "
			.. filename
			.. " with id "
			.. photoId
			.. " and tasks: "
			.. (options.tasks and table.concat(options.tasks, ", ") or "none")
	)

	local response, err = _requestMultipart(url, mimeChunks, 720)

	if not response then
		log:error("Failed to analyze/index photo: " .. tostring(err))
		return false, err or "Unknown error"
	end

	-- Check response status
	if response.status == "processed" then
		local success_count = response.success_count or 0

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

function SearchIndexAPI.searchIndex(searchTerm, qualitySort, photosToSearch, searchOptions)
	local params = {
		term = searchTerm,
		quality_sort = qualitySort,
	}
	local cid = getCatalogId()
	if cid then
		params.catalog_id = cid
	end

	local url = getBaseUrl() .. ENDPOINTS.SEARCH

	-- Build search_sources for API (snake_case). If searchOptions is nil, backend uses defaults.
	local search_sources = nil
	if searchOptions then
		search_sources = {
			semantic_siglip = searchOptions.semanticSiglip ~= false,
			semantic_vertex = searchOptions.semanticVertex ~= false,
			metadata = searchOptions.metadata ~= false,
			metadata_fields = searchOptions.metadataFields or { "flattened_keywords", "alt_text", "caption", "title" },
		}
	end

	if photosToSearch and #photosToSearch > 0 then
		-- Perform a scoped search via POST
		local photoIds = {}
		for _, photo in ipairs(photosToSearch) do
			local photoId, idErr = getPhotoIdForPhoto(photo)
			if photoId then
				table.insert(photoIds, photoId)
			else
				log:error("Skipping photo in scoped search due to missing photo ID: " .. tostring(idErr))
			end
		end

		local body = {
			term = searchTerm,
			photo_ids = photoIds,
			catalog_id = getCatalogId(),
		}
		if search_sources then
			body.search_sources = search_sources
		end
		local postUrl = buildUrlWithParams(url, params)

		log:trace("Searching index via POST (scoped): " .. postUrl)
		return _request("POST", postUrl, body)
	else
		-- Global search: use POST when search_sources are provided so we can send JSON body
		if search_sources then
			local body = { term = searchTerm, search_sources = search_sources, catalog_id = getCatalogId() }
			local postUrl = buildUrlWithParams(url, params)
			log:trace("Searching index via POST (global with search_sources): " .. postUrl)
			return _request("POST", postUrl, body)
		end
		local getUrl = buildUrlWithParams(url, params)
		log:trace("Searching index via GET (global): " .. getUrl)
		return _request("GET", getUrl)
	end
end

function SearchIndexAPI.getStats()
	local cid = getCatalogId()
	local url = getBaseUrl() .. ENDPOINTS.STATS
	if cid then
		url = url .. (url:find("?") and "&" or "?") .. "catalog_id=" .. cid
	end
	return _request("GET", url)
end

function SearchIndexAPI.getBackendVersion()
	return _request("GET", getBaseUrl() .. ENDPOINTS.VERSION)
end

function SearchIndexAPI.checkVersionCompatibility()
	local pluginVersion = tostring(Info.MAJOR) .. "." .. tostring(Info.MINOR) .. "." .. tostring(Info.REVISION)
	local pluginReleaseTag = "v" .. pluginVersion
	local body = {
		plugin_version = pluginVersion,
		plugin_release_tag = pluginReleaseTag,
		plugin_build = tonumber(Info.BUILD) or 0,
	}
	return _request("POST", getBaseUrl() .. ENDPOINTS.VERSION_CHECK, body)
end

function SearchIndexAPI.ensureVersionCompatibility()
	local result, err = SearchIndexAPI.checkVersionCompatibility()
	if err then
		return false, "Version check request failed: " .. tostring(err), nil
	end
	if type(result) ~= "table" then
		return false, "Version check failed: invalid response from backend.", nil
	end
	if result.compatible then
		return true, nil, result
	end

	local pluginTag = tostring(result.plugin_release_tag or ("v" .. tostring(result.plugin_version or "unknown")))
	local backendTag = tostring(result.backend_release_tag or ("v" .. tostring(result.backend_version or "unknown")))
	local reason = tostring(result.reason or "plugin and backend version differ")
	local message = "Plugin and backend versions are not compatible.\n"
		.. "Plugin: "
		.. pluginTag
		.. "\n"
		.. "Backend: "
		.. backendTag
		.. "\n"
		.. "Reason: "
		.. reason
	return false, message, result
end

function SearchIndexAPI.formatStats(stats)
	if type(stats) ~= "table" then
		return "No statistics available."
	end

	local photos = stats.photos or {}
	local faces = stats.faces or {}
	local persons = stats.persons or {}

	return table.concat({
		"Photos total: " .. tostring(photos.total or 0),
		"Photos with embeddings: " .. tostring(photos.with_embedding or 0),
		"Photos with title: " .. tostring(photos.with_title or 0),
		"Photos with caption: " .. tostring(photos.with_caption or 0),
		"Photos with keywords: " .. tostring(photos.with_keywords or 0),
		"Photos with Vertex AI: " .. tostring(photos.with_vertexai or 0),
		"Faces total: " .. tostring(faces.total or 0),
		"Persons total: " .. tostring(persons.total or 0),
	}, "\n")
end

function SearchIndexAPI.getAllIndexedPhotoIds(requireEmbeddings)
	local url = getBaseUrl() .. ENDPOINTS.GET_IDS
	local params = {}
	if requireEmbeddings then
		params.has_embedding = "true"
	end
	local cid = getCatalogId()
	if cid then
		params.catalog_id = cid
	end
	if next(params) then
		local sep = "?"
		for k, v in pairs(params) do
			url = url .. sep .. k .. "=" .. v
			sep = "&"
		end
	end
	return _request("GET", url)
end

function SearchIndexAPI.getAllIndexedPhotoUUIDs(requireEmbeddings)
	return SearchIndexAPI.getAllIndexedPhotoIds(requireEmbeddings)
end

---
-- Retrieves stored metadata for a photo by ID.
-- @param photoId The photo ID to retrieve.
-- @return table|nil Response containing metadata and quality fields, or nil on error.
-- Response structure:
--   {
--     status = "success",
--     photo_id = "...",
--     metadata = { title = "...", caption = "...", keywords = {...}, alt_text = "..." },
--   }
--
function SearchIndexAPI.getPhotoData(photoId)
	if not photoId then
		log:error("getPhotoData: photo_id is required")
		return nil
	end

	local url = getBaseUrl() .. "/get"
	local body = { photo_id = photoId }
	local cid = getCatalogId()
	if cid then
		body.catalog_id = cid
	end

	log:trace("Retrieving photo data for photo_id: " .. photoId)

	local result, err = _request("POST", url, body)
	if err then
		log:error("Failed to retrieve photo data: " .. err)
		return nil
	end

	if result and result.status == "success" then
		log:trace("Successfully retrieved photo data for photo_id: " .. photoId)
		return result
	else
		log:warn("Photo data not found for photo_id: " .. photoId)
		return nil
	end
end

function SearchIndexAPI.groupSimilarPhotos(photoIds, options)
	options = options or {}
	if type(photoIds) ~= "table" or #photoIds == 0 then
		return nil, "photo_ids required"
	end

	local body = {
		photo_ids = photoIds,
		phash_threshold = options.phash_threshold or "auto",
		clip_threshold = options.clip_threshold or "auto",
		time_delta_seconds = options.time_delta_seconds or 2,
		culling_preset = options.culling_preset or "default",
	}

	local result, err = _request("POST", getBaseUrl() .. ENDPOINTS.GROUP_SIMILAR, body, 300)
	if err then
		log:error("groupSimilarPhotos failed: " .. tostring(err))
		return nil, err
	end
	return result
end

function SearchIndexAPI.cullPhotos(photoIds, options)
	options = options or {}
	if type(photoIds) ~= "table" or #photoIds == 0 then
		return nil, "photo_ids required"
	end

	local body = {
		photo_ids = photoIds,
		phash_threshold = options.phash_threshold or "auto",
		clip_threshold = options.clip_threshold or "auto",
		time_delta_seconds = options.time_delta_seconds or 2,
		culling_preset = options.culling_preset or "default",
	}

	local result, err = _request("POST", getBaseUrl() .. ENDPOINTS.CULL, body, 300)
	if err then
		log:error("cullPhotos failed: " .. tostring(err))
		return nil, err
	end
	return result
end

---
-- Find photos similar to the given photo by perceptual hash (and optionally CLIP).
-- @param photoId string Reference photo ID (must be indexed with phash).
-- @param options table Optional: scope_photo_ids (table), max_results (number), phash_max_hamming (number), use_clip (boolean), catalog_id (string).
-- @return table|nil { results = { { photo_id, phash_distance, clip_distance }, ... } }, or nil, err
--
function SearchIndexAPI.findSimilarImages(photoId, options)
	if not photoId or type(photoId) ~= "string" or photoId:match("^%s*$") then
		return nil, "photo_id required"
	end
	options = options or {}
	local body = {
		photo_id = photoId,
		max_results = options.max_results or 100,
		phash_max_hamming = options.phash_max_hamming or 10,
		use_clip = options.use_clip ~= false,
		similarity_mode = options.similarity_mode or "phash",
	}
	if options.scope_photo_ids and type(options.scope_photo_ids) == "table" and #options.scope_photo_ids > 0 then
		body.scope_photo_ids = options.scope_photo_ids
	end
	local cid = getCatalogId()
	if cid then
		body.catalog_id = cid
	end
	log:info(
		"findSimilarImages: photo_id=%s max_results=%s phash_max_hamming=%s scope=%s",
		photoId,
		body.max_results,
		body.phash_max_hamming,
		body.scope_photo_ids and (#body.scope_photo_ids .. " ids") or "all"
	)
	local result, err = _request("POST", getBaseUrl() .. ENDPOINTS.FIND_SIMILAR, body, 120)
	if err then
		log:error("findSimilarImages failed: " .. tostring(err))
		return nil, err
	end
	local count = (result and result.results and #result.results) or 0
	log:info("findSimilarImages: got %s similar photo(s)", count)
	return result
end

function SearchIndexAPI.removePhotoId(photoId)
	local url = getBaseUrl() .. ENDPOINTS.REMOVE
	local body = { photo_id = photoId }
	log:trace("Removing photo_id: " .. photoId)

	local _, err = _request("POST", url, body)
	if not err then
		return true
	else
		ErrorHandler.handleError("Remove UUID failed", err)
		return false
	end
end

function SearchIndexAPI.removeUUID(uuid)
	return SearchIndexAPI.removePhotoId(uuid)
end

--- Remove only AI-generated metadata for a photo (keeps embeddings so the photo stays in the index).
--- Use when the user discards a suggestion in the review dialog so they can regenerate later.
function SearchIndexAPI.removePhotoMetadata(photoId)
	local url = getBaseUrl() .. ENDPOINTS.REMOVE_METADATA
	local body = { photo_id = photoId }
	log:trace("Removing metadata for photo_id: " .. photoId)

	local _, err = _request("POST", url, body)
	if not err then
		return true
	else
		ErrorHandler.handleError("Remove metadata failed", err)
		return false
	end
end

---
-- Sync cleanup: disassociate this catalog from backend photos that are no longer in the catalog.
-- Does not delete backend data; works with global photo ID and cross-catalog backends.
-- @return boolean success, string|nil error message
--
function SearchIndexAPI.syncCleanup()
	local catalogId = getCatalogId()
	if not catalogId then
		log:warn("syncCleanup: no catalog identifier")
		return false, "No catalog identifier"
	end

	if not SearchIndexAPI.pingServer() then
		return false, "Backend not reachable"
	end

	local catalog = LrApplication.activeCatalog()
	local allPhotos = catalog:getAllPhotos()
	local photoIds = {}
	local updateInterval = math.max(1, math.floor(#allPhotos / 50))

	local progressScope = LrProgressScope({
		title = LOC("$$$/LrGeniusAI/SearchIndexAPI/cleaningIndex=Cleaning search index"),
		functionContext = nil,
	})

	for i, photo in ipairs(allPhotos) do
		if progressScope:isCanceled() then
			progressScope:done()
			return false, "canceled"
		end
		local photoId = getPhotoIdForPhoto(photo)
		if photoId then
			photoIds[#photoIds + 1] = photoId
		end
		if i % updateInterval == 0 or i == #allPhotos then
			progressScope:setPortionComplete(i, #allPhotos)
			progressScope:setCaption(
				LOC("$$$/LrGeniusAI/SearchIndexAPI/cleaningIndexProgress=Cleaning index. Photo ^1/^2"),
				tostring(i),
				tostring(#allPhotos)
			)
		end
	end

	progressScope:setCaption(LOC("$$$/LrGeniusAI/SearchIndexAPI/syncCleanupSending=Syncing with backend..."))
	local batchSize = 5000
	local disassociated = 0
	for startIdx = 1, #photoIds, batchSize do
		if progressScope:isCanceled() then
			progressScope:done()
			return false, "canceled"
		end
		local stopIdx = math.min(startIdx + batchSize - 1, #photoIds)
		local batch = {}
		for j = startIdx, stopIdx do
			batch[#batch + 1] = photoIds[j]
		end
		local result, err = _request("POST", getBaseUrl() .. ENDPOINTS.SYNC_CLEANUP, {
			catalog_id = catalogId,
			photo_ids = batch,
		}, 120)
		if err then
			progressScope:done()
			log:error("syncCleanup failed: " .. tostring(err))
			return false, err
		end
		if result and result.disassociated then
			disassociated = disassociated + result.disassociated
		end
	end
	progressScope:done()
	log:info(
		"syncCleanup finished: "
			.. tostring(#photoIds)
			.. " photos in catalog, "
			.. tostring(disassociated)
			.. " disassociated"
	)
	return true
end

---
-- Claim backend photos for this catalog (add catalog_id to their catalog_ids).
-- Use after migration so existing indexed photos become visible to this catalog.
-- @param progressScope LrProgressScope|nil Optional; when provided, shows progress and supports cancel.
-- @return boolean success, string|nil error message, table|nil result
--
function SearchIndexAPI.claimPhotosForCatalog(progressScope)
	-- This function is executed as one of the catalog-scoped background DB migrations.
	-- Avoid calling `getCatalogId()` here because it would wait for migrations that include
	-- this very function (self-wait / deadlock-like behavior).
	local catalogId = getCatalogIdValue()
	if not catalogId then
		return false, "No catalog identifier", nil
	end
	if not SearchIndexAPI.pingServer() then
		return false, "Backend not reachable", nil
	end
	local catalog = LrApplication.activeCatalog()
	local allPhotos = catalog:getAllPhotos()
	local totalPhotos = #allPhotos
	local photoIds = {}

	-- Hash phase dominates wall time (~6ms per photo); report progress against
	-- photo count so the UI doesn't sit at 0% for minutes on large catalogs.
	if progressScope then
		progressScope:setPortionComplete(0, totalPhotos)
		progressScope:setCaption(
			LOC(
				"$$$/LrGeniusAI/SearchIndexAPI/claimingPhotosPreparing=Preparing ^1 photos for this catalog...",
				tostring(totalPhotos)
			)
		)
	end
	local progressStride = math.max(50, math.floor(totalPhotos / 200))
	for i, photo in ipairs(allPhotos) do
		if progressScope and progressScope:isCanceled() then
			progressScope:done()
			return false, "canceled", nil
		end
		local photoId, _ = getPhotoIdForPhoto(photo)
		if photoId then
			photoIds[#photoIds + 1] = photoId
		end
		if progressScope and (i % progressStride == 0 or i == totalPhotos) then
			progressScope:setPortionComplete(i, totalPhotos)
			progressScope:setCaption(
				LOC(
					"$$$/LrGeniusAI/SearchIndexAPI/claimingPhotosPreparingCount=Preparing ^1 of ^2 photos...",
					tostring(i),
					tostring(totalPhotos)
				)
			)
		end
	end
	if #photoIds == 0 then
		if progressScope then
			progressScope:done()
		end
		return true, nil, { claimed = 0, errors = 0 }
	end
	local batchSize = 2500
	local totalBatches = math.ceil(#photoIds / batchSize)
	local totalClaimed = 0
	local totalErrors = 0
	for startIdx = 1, #photoIds, batchSize do
		if progressScope then
			if progressScope:isCanceled() then
				progressScope:done()
				return false, "canceled", nil
			end
			local batchNum = math.floor((startIdx - 1) / batchSize) + 1
			progressScope:setCaption(
				LOC(
					"$$$/LrGeniusAI/SearchIndexAPI/claimingPhotosBatch=Claiming photos... batch ^1/^2",
					tostring(batchNum),
					tostring(totalBatches)
				)
			)
		end
		local stopIdx = math.min(startIdx + batchSize - 1, #photoIds)
		local batch = {}
		for j = startIdx, stopIdx do
			batch[#batch + 1] = photoIds[j]
		end
		local result, err = _request("POST", getBaseUrl() .. ENDPOINTS.SYNC_CLAIM, {
			catalog_id = catalogId,
			photo_ids = batch,
		}, 120)
		if err then
			if progressScope then
				progressScope:done()
			end
			return false, err, nil
		end
		if result then
			totalClaimed = totalClaimed + (result.claimed or 0)
			totalErrors = totalErrors + (result.errors or 0)
		end
	end
	if progressScope then
		progressScope:setPortionComplete(totalPhotos, totalPhotos)
	end
	return true, nil, { claimed = totalClaimed, errors = totalErrors }
end

function SearchIndexAPI.removeMissingFromIndex()
	-- Use sync cleanup (soft state): disassociate this catalog from photos no longer in catalog.
	-- Works with global photo ID and cross-catalog backends; no backend data is deleted.
	return SearchIndexAPI.syncCleanup()
end

---
-- Analyzes and indexes selected photos with LLM processing (metadata, embeddings).
-- Uses JPEG export instead of thumbnails for better reliability.
-- @param selectedPhotos table Array of LrPhoto objects to process.
-- @param progressScope LrProgressScope Progress scope for UI updates.
-- @param options table Processing options (tasks, provider, language, temperature, etc.).
--                Optional options.onPhotoAnalyzed(photo, photoId, progressScope): if provided,
--                invoked inside the worker loop immediately after each photo is successfully
--                analyzed. Lets callers write metadata per-photo as the batch progresses
--                instead of waiting for all photos to finish. Errors in the callback are
--                caught with LrTasks.pcall and logged; the batch continues.
-- @param closeProgressScope boolean|nil When false, does not call :done() on the scope (caller must close).
-- @return string status Status: "success", "canceled", "somefailed", or "allfailed".
-- @return number processed Number of photos processed.
-- @return number failed Number of photos that failed.
-- @return table responses Array of response data from the server for each photo.
-- @return string|nil warnings Combined warnings from the server.
--
function SearchIndexAPI.analyzeAndIndexSelectedPhotos(selectedPhotos, progressScope, options, closeProgressScope)
	local numPhotos = #selectedPhotos
	if numPhotos == 0 then
		return "success", 0, 0, {}
	end

	if not SearchIndexAPI.pingServer() then
		return "allfailed", numPhotos, numPhotos, {}
	end

	options = options or {}
	local shouldCloseScope = (closeProgressScope ~= false)

	progressScope:setCaption(
		LOC(
			"$$$/LrGeniusAI/AnalyzeAndIndex/ProcessingPhotos=Processing ^1 photos with ^2...",
			#selectedPhotos,
			options.model or "AI"
		)
	)
	progressScope:setPortionComplete(0, numPhotos)

	local photoToProcessStack = {}
	for _, photo in ipairs(selectedPhotos) do
		table.insert(photoToProcessStack, photo)
	end

	local maxWorkers = 1 -- tonumber(prefs.indexingParallelTasks) or 2
	local stats = { processed = 0, success = 0, failed = 0 }
	local processedPhotos = {}
	local activeWorkers = 0
	local keepRunning = true
	local previewRequestState = {
		enabled = (prefs and prefs.usePreviewThumbnails ~= false),
		timeoutSeconds = tonumber(prefs and prefs.previewThumbnailTimeoutSeconds) or 12,
		cooldownSeconds = tonumber(prefs and prefs.previewThumbnailCooldownSeconds) or 1,
		disableAfterConsecutiveTimeouts = tonumber(prefs and prefs.previewThumbnailDisableAfterTimeouts) or 3,
		consecutiveTimeouts = 0,
		disabledForRun = false,
	}

	local errorMessages = {}
	local warningsList = {}

	local analyzeWorker = function()
		while #photoToProcessStack > 0 do
			if progressScope:isCanceled() then
				break
			end
			if not keepRunning then
				break
			end

			local photo = table.remove(photoToProcessStack, 1)
			if photo ~= nil then
				local filename = photo:getFormattedMetadata("fileName")
				local hashStart = LrDate.currentTime()
				local photoId, photoIdErr = getPhotoIdForPhoto(photo)
				if photoId then
					log:trace(
						"Using photo_id for "
							.. filename
							.. " (hashing_ms="
							.. tostring(math.floor((LrDate.currentTime() - hashStart) * 1000))
							.. ")"
					)

					-- Prepare analysis options with photo-specific context
					local photoOptions = {}
					for k, v in pairs(options) do
						photoOptions[k] = v
					end
					if options.submit_gps then
						local gps = photo:getRawMetadata("gps")
						if gps then
							photoOptions.gps_coordinates = gps
						end
					end
					if options.submit_keywords then
						local keywords = photo:getFormattedMetadata("keywordTagsForExport")
						if keywords then
							-- Lightroom may return a comma-separated string; send as array so server
							-- does not treat it as iterable of characters (issue #45).
							if type(keywords) == "string" then
								photoOptions.existing_keywords = Util.string_split(keywords, ",")
							else
								photoOptions.existing_keywords = keywords
							end
						end
					end
					if options.submit_folder_names then
						local originalFilePath = photo:getRawMetadata("path")
						if originalFilePath then
							photoOptions.folder_names = Util.getStringsFromRelativePath(originalFilePath)
						end
					end
					-- Always submit catalog capture time.
					local datetime = photo:getRawMetadata("dateTime")
					if datetime ~= nil and type(datetime) == "number" then
						-- Keep backwards-compatible ISO string for older backends
						photoOptions.date_time = LrDate.timeToW3CDate(datetime)
						-- Also send Unix timestamp (seconds since 1970-01-01 UTC)
						photoOptions.date_time_unix = LrDate.timeToPosixDate(datetime)
					end
					photoOptions.user_context = photo:getPropertyForPlugin(_PLUGIN, "photoContext") or ""
					photoOptions.photo_id = photoId

					local success, indexResponse
					local usePreviewThumbnails = previewRequestState.enabled and not previewRequestState.disabledForRun
					local thumbnailSize = tonumber(prefs and prefs.exportSize) or 1024
					local leafName = LrPathUtils.leafName(filename or "photo.jpg")

					if usePreviewThumbnails then
						local jpegData, thumbErr = SearchIndexAPI.getJpegThumbnailForPhoto(
							photo,
							thumbnailSize,
							thumbnailSize,
							previewRequestState
						)
						if jpegData and #jpegData > 0 then
							previewRequestState.consecutiveTimeouts = 0
							log:trace("Using Lightroom preview for " .. filename)
							success, indexResponse =
								SearchIndexAPI.analyzeAndIndexPhotoBase64(photoId, jpegData, leafName, photoOptions)
						else
							log:trace(
								"Preview unavailable for "
									.. filename
									.. ", falling back to export: "
									.. tostring(thumbErr)
							)
							if thumbErr and string.find(thumbErr, "timed out", 1, true) then
								previewRequestState.consecutiveTimeouts = previewRequestState.consecutiveTimeouts + 1
								if
									previewRequestState.consecutiveTimeouts
									>= previewRequestState.disableAfterConsecutiveTimeouts
								then
									previewRequestState.disabledForRun = true
									log:warn(
										"Disabling Lightroom preview thumbnails for the rest of this batch after "
											.. tostring(previewRequestState.consecutiveTimeouts)
											.. " consecutive timeouts."
									)
								else
									log:trace(
										"Cooling down preview requests after timeout ("
											.. tostring(previewRequestState.consecutiveTimeouts)
											.. "/"
											.. tostring(previewRequestState.disableAfterConsecutiveTimeouts)
											.. ")"
									)
								end

								if previewRequestState.cooldownSeconds > 0 then
									LrTasks.sleep(previewRequestState.cooldownSeconds)
								end
							else
								previewRequestState.consecutiveTimeouts = 0
							end
						end
					end

					if not success then
						local exportedPhotoPath = SearchIndexAPI.exportPhotoForIndexing(photo)
						if exportedPhotoPath then
							log:trace("Using exported JPEG for " .. filename)
							success, indexResponse =
								SearchIndexAPI.analyzeAndIndexPhoto(photoId, exportedPhotoPath, photoOptions)
							LrFileUtils.delete(exportedPhotoPath)
						end
					end

					if success then
						stats.success = stats.success + 1
						if indexResponse and indexResponse.warnings and #indexResponse.warnings > 0 then
							for _, w in ipairs(indexResponse.warnings) do
								table.insert(warningsList, w)
							end
						end
						if options.onPhotoAnalyzed then
							local okCb, cbErr = LrTasks.pcall(function()
								options.onPhotoAnalyzed(photo, photoId, progressScope)
							end)
							if not okCb then
								log:error("onPhotoAnalyzed callback failed for " .. filename .. ": " .. tostring(cbErr))
							end
						end
					else
						stats.failed = stats.failed + 1
						table.insert(errorMessages, tostring(indexResponse or "Unknown"))
						log:error(
							"Failed to analyze/index photo: " .. filename .. " Error: " .. (indexResponse or "Unknown")
						)
					end
				else
					stats.failed = stats.failed + 1
					table.insert(errorMessages, "Could not compute photo ID: " .. tostring(photoIdErr))
					log:error("Failed to compute photo ID for " .. filename .. ": " .. tostring(photoIdErr))
				end

				stats.processed = stats.processed + 1
				table.insert(processedPhotos, photo)
				progressScope:setPortionComplete(stats.processed, numPhotos)
				progressScope:setCaption(
					LOC(
						"$$$/LrGeniusAI/AnalyzeAndIndex/ProcessingPhoto=Processing ^1 successful (^2 total/^3 failed)",
						stats.success,
						numPhotos,
						stats.failed
					)
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

	while activeWorkers > 0 do
		if progressScope:isCanceled() then
			break
		end
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

	if shouldCloseScope then
		progressScope:done()
	end

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

	local combinedError
	if #errorMessages > 0 then
		local uniqueErrors = {}
		local errorList = {}
		for _, msg in ipairs(errorMessages) do
			if not uniqueErrors[msg] then
				uniqueErrors[msg] = true
				table.insert(errorList, msg)
				if #errorList >= 5 then
					break
				end
			end
		end
		combinedError = table.concat(errorList, "\n")
	end

	local combinedWarnings
	if #warningsList > 0 then
		local uniqueWarnings = {}
		local warningListStrings = {}
		for _, w in ipairs(warningsList) do
			if not uniqueWarnings[w] then
				uniqueWarnings[w] = true
				table.insert(warningListStrings, w)
			end
		end
		combinedWarnings = table.concat(warningListStrings, "\n")
	end

	return status, stats.processed, stats.failed, processedPhotos, combinedError, combinedWarnings
end

---
-- Imports metadata from the Lightroom catalog into the backend index.
-- @param photosToProcess table Array of LrPhoto.
-- @param progressScope LrProgressScope Progress scope for UI updates.
-- @param closeProgressScope boolean|nil When false, does not call :done() on the scope (caller must close).
-- @param updateProgress boolean|nil When false, does not write to the scope's caption or portion-complete.
--                Use when sharing a scope with an outer loop that already tracks progress (e.g. the
--                per-photo onPhotoAnalyzed callback in analyzeAndIndexSelectedPhotos). Cancellation
--                is still honoured. Default: true (preserves legacy behaviour).
--
function SearchIndexAPI.importMetadataFromCatalog(photosToProcess, progressScope, closeProgressScope, updateProgress)
    local numPhotos = #photosToProcess
    if numPhotos == 0 then
        return "success", 0, 0
    end

    if not SearchIndexAPI.pingServer() then
        return "allfailed", numPhotos, numPhotos
    end

    local shouldCloseScope = (closeProgressScope ~= false)
    local shouldUpdateProgress = (updateProgress ~= false)

    if shouldUpdateProgress then
        progressScope:setCaption(LOC "$$$/LrGeniusAI/ImportMetadata/ProgressTitle=Importing metadata for photos...")
        progressScope:setPortionComplete(0, numPhotos)
    end

    local stats = { processed = 0, success = 0, failed = 0 }
    local batchSize = 50 -- Send metadata in batches
    local metadataBatch = {}

    for i, photo in ipairs(photosToProcess) do
        if photo ~= nil then
            if progressScope:isCanceled() then
                break
            end

            local photoId = getPhotoIdForPhoto(photo)
            local metadata = {
                photo_id = photoId,
                caption = photo:getFormattedMetadata("caption"),
                title = photo:getFormattedMetadata("title"),
                keywords = MetadataManager.getPhotoKeywordHierarchy(photo),
                alt_text = photo:getFormattedMetadata("altTextAccessibility")
            }
            if type(metadata.photo_id) ~= "string" or metadata.photo_id == "" then
                stats.failed = stats.failed + 1
                stats.processed = stats.processed + 1
                log:error("Skipping metadata import for photo due to missing photo_id: " ..
                    (photo:getFormattedMetadata("fileName") or "unknown"))
                if shouldUpdateProgress then
                    progressScope:setPortionComplete(stats.processed, numPhotos)
                end
            else
                table.insert(metadataBatch, metadata)
            end

            if #metadataBatch > 0 and (#metadataBatch >= batchSize or i == numPhotos) then
                local importBody = { metadata_items = metadataBatch }
                local importCid = getCatalogId()
                if importCid then
                    importBody.catalog_id = importCid
                end
                local response = _request('POST', getBaseUrl() .. ENDPOINTS.IMPORT_METADATA, importBody)
                if response ~= nil and response.status == "processed" then
                    stats.success = stats.success + #metadataBatch
                else
                    stats.failed = stats.failed + #metadataBatch
                    log:error("Failed to import metadata batch: " .. (response and response.error or "Unknown error"))
                end
                metadataBatch = {} -- Clear the batch
            end

            stats.processed = stats.processed + 1
            if shouldUpdateProgress then
                progressScope:setPortionComplete(stats.processed, numPhotos)
                progressScope:setCaption(
                    LOC("$$$/LrGeniusAI/ImportMetadata/Processing=Importing metadata... ^1/^2 (^3 failed)",
                        stats.processed, numPhotos, stats.failed)
                )
            end
        else
            log:error("Photo is nil in importMetadataFromCatalog, probably it got deleted in the meantime.")
        end
    end

    if shouldCloseScope then
        progressScope:done()
    end

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
	local url = getBaseUrl() .. "/ping"
	local result, hdrs = LrHttp.get(url)
	local status = (type(hdrs) == "number") and hdrs or (type(hdrs) == "table" and hdrs.status) or nil
	if status == 200 and result == "pong" then
		return true
	else
		return false
	end
end

function SearchIndexAPI.isBackendOnLocalhost()
	local url = getBaseUrl()
	return not not (url:match("^https?://127%.0%.0%.1") or url:match("^https?://localhost"))
end

function SearchIndexAPI.downloadDatabaseBackup()
	local url = getBaseUrl() .. ENDPOINTS.DB_BACKUP
	log:info("downloadDatabaseBackup: start, url=" .. tostring(url))
	local outputPath = LrDialogs.runSavePanel({
		title = "Save database backup",
		prompt = "Save Backup",
		canCreateDirectories = true,
		requiredFileType = "zip",
	})
	log:info(
		"downloadDatabaseBackup: save panel returned type="
			.. tostring(type(outputPath))
			.. " value="
			.. tostring(outputPath)
	)

	if not outputPath or outputPath == "" then
		log:info("Database backup download canceled by user")
		return nil, "canceled"
	end

	if type(outputPath) ~= "string" then
		local err = "Save panel returned unexpected type for outputPath: " .. tostring(type(outputPath))
		log:error("downloadDatabaseBackup: " .. err)
		return false, err
	end

	if not outputPath:lower():match("%.zip$") then
		outputPath = outputPath .. ".zip"
	end

	log:info("Downloading database backup from " .. url .. " to " .. outputPath)

	-- _request mit leerer Tabelle als body, kein Timeout (vermeidet SDK-Crash), raw=true für Binär-Zip
	local responseBody, hdrs = _request("GET", url, {}, nil, { raw = true })
	if responseBody == nil then
		local err = hdrs or "Backup download failed"
		log:error("downloadDatabaseBackup: " .. tostring(err))
		return false, err
	end
	local status = (type(hdrs) == "number") and hdrs or (type(hdrs) == "table" and hdrs.status) or nil
	log:info(
		"downloadDatabaseBackup: HTTP finished, status="
			.. tostring(status)
			.. ", hdrsType="
			.. tostring(type(hdrs))
			.. ", bodyType="
			.. tostring(type(responseBody))
			.. ", bodyLen="
			.. tostring(type(responseBody) == "string" and #responseBody or "n/a")
	)
	if status == nil or status < 200 or status >= 300 then
		local err = "Backup download failed. HTTP status: " .. tostring(status or "unknown")
		if type(responseBody) == "string" and #responseBody > 0 then
			local ok, decoded = LrTasks.pcall(function()
				return JSON:decode(responseBody)
			end)
			log:info(
				"downloadDatabaseBackup: error response JSON decode ok="
					.. tostring(ok)
					.. ", decodedType="
					.. tostring(type(decoded))
			)
			if ok and type(decoded) == "table" and decoded.error then
				err = err .. " - " .. tostring(decoded.error)
			end
		elseif responseBody ~= nil then
			err = err .. " - rawBody(" .. tostring(type(responseBody)) .. "): " .. tostring(responseBody)
		end
		log:error(err)
		return false, err
	end

	local file, openErr = io.open(outputPath, "wb")
	if not file then
		local err = "Could not create backup file: " .. tostring(openErr)
		log:error(err)
		return false, err
	end

	local dataToWrite = responseBody
	if dataToWrite == nil then
		dataToWrite = ""
	elseif type(dataToWrite) ~= "string" then
		log:warn(
			"downloadDatabaseBackup: responseBody is not a string, converting via tostring. type="
				.. tostring(type(dataToWrite))
		)
		dataToWrite = tostring(dataToWrite)
	end

	local writeOk, writeErr = LrTasks.pcall(function()
		file:write(dataToWrite)
	end)
	if not writeOk then
		file:close()
		local err = "Could not write backup file: " .. tostring(writeErr)
		log:error("downloadDatabaseBackup: " .. err)
		return false, err
	end
	file:close()

	if not LrFileUtils.exists(outputPath) then
		local err = "Backup file was not created."
		log:error(err)
		return false, err
	end

	log:info(
		"Database backup downloaded successfully: " .. outputPath .. " (writtenBytes=" .. tostring(#dataToWrite) .. ")"
	)
	return true, outputPath
end

-- -----------------------------
-- Structured backend lifecycle
-- -----------------------------
local SERVER_PID_FILENAME = "lrgenius-server.pid"
local SERVER_OK_FILENAME = "lrgenius-server.OK"
local SERVER_LOCK_FILENAME = "lrgenius-server.lock"

local serverStartInProgress = false

local function getServerControlDir()
	-- Backend uses --db-path = "<catalogParent>/lrgenius.db", and writes pid/OK files next to it.
	return LrPathUtils.parent(LrApplication.activeCatalog():getPath())
end

local function getServerPidFilePath()
	return LrPathUtils.child(getServerControlDir(), SERVER_PID_FILENAME)
end

local function getServerOkFilePath()
	return LrPathUtils.child(getServerControlDir(), SERVER_OK_FILENAME)
end

local function getServerLockFilePath()
	return LrPathUtils.child(getServerControlDir(), SERVER_LOCK_FILENAME)
end

local function cleanupServerPidAndOkFiles()
	local pidPath = getServerPidFilePath()
	local okPath = getServerOkFilePath()
	if LrFileUtils.exists(pidPath) then
		LrTasks.pcall(function()
			LrFileUtils.delete(pidPath)
		end)
	end
	if LrFileUtils.exists(okPath) then
		LrTasks.pcall(function()
			LrFileUtils.delete(okPath)
		end)
	end
end

local function readPidFromPidFile()
	local pidFilePath = getServerPidFilePath()
	local pidFile = io.open(pidFilePath, "r")
	if not pidFile then
		return nil
	end
	local pid = pidFile:read("*l")
	pidFile:close()
	if not pid then
		return nil
	end
	return tonumber(pid)
end

local function isPidAlive(pid)
	if not pid then
		return false
	end
	if MAC_ENV then
		-- Exit code 0 => process exists
		local cmd = "ps -p " .. tostring(pid) .. " >/dev/null 2>&1"
		local rc = LrTasks.execute(cmd)
		return rc == 0
	end
	if WIN_ENV then
		-- Best-effort (avoid brittle parsing of tasklist output)
		local cmd = 'tasklist /FI "PID eq ' .. tostring(pid) .. '" | findstr /I "' .. tostring(pid) .. '" >NUL'
		local rc = LrTasks.execute(cmd)
		return rc == 0
	end
	return false
end

local function acquireStartLock(lockStaleSeconds)
	if serverStartInProgress then
		return false
	end
	lockStaleSeconds = lockStaleSeconds or 120

	local lockPath = getServerLockFilePath()
	if LrFileUtils.exists(lockPath) then
		local lockFile = io.open(lockPath, "r")
		local content = lockFile and lockFile:read("*a") or ""
		if lockFile then
			lockFile:close()
		end

		local ts = content:match("ts=(%d+)")
		local tsN = tonumber(ts)
		if tsN and (os.time() - tsN) < lockStaleSeconds then
			-- Another start attempt is still considered fresh.
			return false
		else
			-- Stale lock: remove it.
			LrTasks.pcall(function()
				LrFileUtils.delete(lockPath)
			end)
		end
	end

	local f = io.open(lockPath, "w")
	if not f then
		return false
	end
	f:write("ts=" .. tostring(os.time()))
	f:close()

	serverStartInProgress = true
	return true
end

local function releaseStartLock()
	serverStartInProgress = false
	local lockPath = getServerLockFilePath()
	if LrFileUtils.exists(lockPath) then
		LrTasks.pcall(function()
			LrFileUtils.delete(lockPath)
		end)
	end
end

function SearchIndexAPI.shutdownServer(opts)
	opts = opts or {}
	local graceSeconds = opts.graceSeconds or 10
	local pollIntervalSeconds = opts.pollIntervalSeconds or 0.5
	local shutdownRequestTimeoutSeconds = opts.shutdownRequestTimeoutSeconds or 5

	if not SearchIndexAPI.pingServer() then
		log:trace("Search index server is not running (or unreachable)")
		cleanupServerPidAndOkFiles()
		return true
	end

	local url = getBaseUrl() .. ENDPOINTS.SHUTDOWN
	log:trace("Requesting graceful backend shutdown")

	-- /shutdown returns JSON, so we can go through _request() decoding.
	LrTasks.pcall(function()
		_request("POST", url, {}, shutdownRequestTimeoutSeconds)
	end)

	local deadline = LrDate.currentTime() + graceSeconds
	while LrDate.currentTime() < deadline do
		if not SearchIndexAPI.pingServer() then
			cleanupServerPidAndOkFiles()
			return true
		end
		LrTasks.sleep(pollIntervalSeconds)
	end

	log:trace("Graceful shutdown timed out; escalating to kill")
	return SearchIndexAPI.killServer({ killMode = "force", forceWaitSeconds = opts.forceWaitSeconds or 10 })
end

function SearchIndexAPI.unloadResources()
	local url = getBaseUrl() .. ENDPOINTS.UNLOAD
	log:trace("Requesting backend model unload")
	local status, response = LrTasks.pcall(function()
		return _request("POST", url, {}, 10) -- 10s timeout
	end)
	if status and response then
		log:trace("Backend models unloaded successfully")
		return true
	else
		log:warn("Failed to unload backend models: " .. tostring(response))
		return false
	end
end

function SearchIndexAPI.restartBackend()
	local url = getBaseUrl() .. ENDPOINTS.RESTART
	log:info("Requesting backend restart via API")
	local _, err = _request("POST", url, {}, 5)
	if err then
		log:error("Failed to request backend restart: " .. tostring(err))
		return false, err
	end

	-- Wait a bit and then ping until back
	LrTasks.sleep(2)
	local deadline = LrDate.currentTime() + 60
	while LrDate.currentTime() < deadline do
		if SearchIndexAPI.pingServer() then
			log:info("Backend restarted successfully")
			local dbPath = LrPathUtils.child(getServerControlDir(), "lrgenius.db")
			SearchIndexAPI.initializeCatalog(dbPath)
			return true
		end
		LrTasks.sleep(1)
	end
	return false, "Restart timeout"
end

function SearchIndexAPI.initializeCatalog(dbPath)
	if not SearchIndexAPI.isLocalBackend() then
		log:info("Skipping catalog initialization for remote backend.")
		return true
	end

	if not dbPath then
		dbPath = LrPathUtils.child(getServerControlDir(), "lrgenius.db")
	end

	local url = getBaseUrl() .. ENDPOINTS.INITIALIZE
	log:info("Initializing catalog database at backend: " .. tostring(dbPath))
	local response, err = _request("POST", url, { db_path = dbPath }, 10)

	if response and (response.status == "success" or response.status == "already_initialized") then
		log:info("Backend initialized successfully for database: " .. tostring(dbPath))
		return true
	else
		log:error(
			"Failed to initialize backend for catalog: "
				.. tostring(err or (response and response.error) or "Unknown error")
		)
		return false, err or (response and response.error)
	end
end

function SearchIndexAPI.killServer(opts)
	opts = opts or {}
	local killMode = opts.killMode or "force" -- "force" => SIGKILL on unix
	local forceWaitSeconds = opts.forceWaitSeconds or 10
	local pollIntervalSeconds = opts.pollIntervalSeconds or 0.5

	local pid = readPidFromPidFile()
	if not pid then
		-- Without a pid file, we can only do a best-effort ping check.
		if not SearchIndexAPI.pingServer() then
			cleanupServerPidAndOkFiles()
			return true
		end
		log:error("killServer: no PID available; cannot force kill safely.")
		return false
	end

	if not isPidAlive(pid) then
		cleanupServerPidAndOkFiles()
		return true
	end

	local killCmd
	if WIN_ENV then
		killCmd = "taskkill /PID " .. tostring(pid) .. " /F"
	elseif MAC_ENV then
		if killMode == "force" then
			killCmd = "kill -9 " .. tostring(pid)
		else
			killCmd = "kill " .. tostring(pid)
		end
	else
		log:error("killServer: unsupported platform for pid kill")
		return false
	end

	log:trace("Forcing backend process kill: " .. tostring(killCmd))
	local rc = LrTasks.execute(killCmd)
	if rc ~= 0 then
		log:error("killServer: kill command exit code: " .. tostring(rc))
	end

	local deadline = LrDate.currentTime() + forceWaitSeconds
	while LrDate.currentTime() < deadline do
		if not SearchIndexAPI.pingServer() then
			cleanupServerPidAndOkFiles()
			return true
		end
		LrTasks.sleep(pollIntervalSeconds)
	end

	cleanupServerPidAndOkFiles()
	return false
end

function SearchIndexAPI.startServer(opts)
	opts = opts or {}
	local readyTimeoutSeconds = opts.readyTimeoutSeconds or 60
	local lockStaleSeconds = opts.lockStaleSeconds or 120

	if SearchIndexAPI.pingServer() then
		log:trace("Search index server is already running, triggering initialization")
		local dbPath = LrPathUtils.child(getServerControlDir(), "lrgenius.db")
		if SearchIndexAPI.initializeCatalog(dbPath) then
			return true
		end
		return false
	end

	if not SearchIndexAPI.isLocalBackend() then
		log:trace("Backend URL points to remote server, skipping local server start")
		return false
	end

	if not acquireStartLock(lockStaleSeconds) then
		log:trace("Backend start lock is active; another start attempt may be in progress")
		return false
	end

	local dbPath = LrPathUtils.child(getServerControlDir(), "lrgenius.db")

	-- Make sure we don't leave the lock behind on early returns.
	local ok, startResult = LrTasks.pcall(function()
		-- If pid/OK are stale, clean them before starting.
		local pid = readPidFromPidFile()
		if pid and not isPidAlive(pid) then
			cleanupServerPidAndOkFiles()
		end

		-- Check standard system locations first (if installed via PKG/EXE)
		local serverBinary = nil
		if MAC_ENV then
			serverBinary = "/Applications/LrGeniusAI/Server/lrgenius-server"
		elseif WIN_ENV then
			serverBinary = "C:\\Program Files\\LrGeniusAI\\backend\\lrgenius-server.cmd"
		end

		-- Fallback to plugin-local binary (development or old installs)
		if not serverBinary or not LrFileUtils.exists(serverBinary) then
			local serverDir = LrPathUtils.child(LrPathUtils.parent(_PLUGIN.path), "lrgenius-server")
			serverBinary = LrPathUtils.child(serverDir, "lrgenius-server")
			if WIN_ENV then
				local serverLauncherCmd = serverBinary .. ".cmd"
				local serverExe = serverBinary .. ".exe"
				if LrFileUtils.exists(serverLauncherCmd) then
					serverBinary = serverLauncherCmd
				else
					serverBinary = serverExe
				end
			end
		end

		if not LrFileUtils.exists(serverBinary) then
			log:error(tostring(serverBinary) .. " not found. Not trying to start server")
			return false
		end

		local startServerCmd
		local serverDir = LrPathUtils.parent(serverBinary)
		if WIN_ENV then
			-- The .cmd launcher handles environment variables and uses pythonw.exe for invisible execution.
			startServerCmd = 'start /b /d "'
				.. serverDir
				.. '" "" "'
				.. tostring(serverBinary)
				.. '" --db-path "'
				.. dbPath
				.. '"'
		elseif MAC_ENV then
			if serverBinary:match("^/Applications") then
				-- System install: use launchctl to trigger the system-wide service
				startServerCmd = "launchctl kickstart -k gui/$(id -u)/com.lrgenius.server"
			else
				-- Local/Dev fallback
				local envPrefix = "KMP_DUPLICATE_LIB_OK=TRUE "
				startServerCmd = envPrefix .. 'bash "' .. tostring(serverBinary) .. '" --db-path "' .. dbPath .. '"'
			end
		else
			-- Unknown platform fallback
			local envPrefix = "KMP_DUPLICATE_LIB_OK=TRUE "
			startServerCmd = envPrefix .. 'bash "' .. tostring(serverBinary) .. '" --db-path "' .. dbPath .. '"'
		end

		log:trace("Trying to start search index server with command: " .. tostring(startServerCmd))
		LrTasks.startAsyncTask(function()
			local result = LrTasks.execute(startServerCmd)
			log:trace("Search index server start command exit code: " .. tostring(result))
		end)

		local deadline = LrDate.currentTime() + readyTimeoutSeconds
		while LrDate.currentTime() < deadline do
			if SearchIndexAPI.pingServer() then
				log:trace("Search index server is running")
				-- Initialize with current catalog
				if SearchIndexAPI.initializeCatalog(dbPath) then
					SearchIndexAPI.checkServerHealth()
					return true
				end
			end
			LrTasks.sleep(0.5)
		end

		log:trace("Search index server did not become ready or initialize within timeout")

		-- Diagnose failure
		local diag = SearchIndexAPI.diagnoseStartupFailure()
		if diag.binaryMissing then
			log:error(
				LOC(
					"$$$/LrGeniusAI/Diagnostics/BinaryMissing=The backend server binary is missing from the plugin folder."
				)
			)
		elseif diag.portBusy then
			log:error(LOC("$$$/LrGeniusAI/Diagnostics/PortBusy=Port 19819 is already in use by another application."))
		end
		if diag.logSnippet then
			log:error(LOC("$$$/LrGeniusAI/Diagnostics/LogSnippet=Recent server errors:") .. "\n" .. diag.logSnippet)
		end
		return false
	end)

	releaseStartLock()

	if not ok then
		log:error("startServer: unexpected error: " .. tostring(startResult))
		return false
	end

	return startResult == true
end

_requestMultipart = function(url, mimeChunks, timeout)
	log:trace(
		"_requestMultipart start: url="
			.. tostring(url)
			.. " timeout="
			.. tostring(timeout)
			.. " chunks="
			.. tostring(type(mimeChunks) == "table" and #mimeChunks or "n/a")
	)
	local result, hdrs = LrHttp.postMultipart(url, mimeChunks, nil, timeout)
	log:trace(
		"_requestMultipart raw return: resultType="
			.. tostring(type(result))
			.. " resultLen="
			.. tostring(type(result) == "string" and #result or "n/a")
			.. " hdrsType="
			.. tostring(type(hdrs))
	)

	-- hdrs kann Tabelle mit .status oder (in einigen LR-Versionen) direkt die Status-Nummer sein
	local status = (type(hdrs) == "number") and hdrs or (type(hdrs) == "table" and hdrs.status) or nil
	log:trace("_requestMultipart interpreted status: " .. tostring(status))
	if status ~= nil and status >= 200 and status < 300 then
		if result and #result > 0 then
			local ok, decodedOrErr = LrTasks.pcall(function()
				return JSON:decode(result)
			end)
			if not ok then
				log:error("_requestMultipart JSON decode failed: " .. tostring(decodedOrErr))
				return nil, "Invalid JSON response from server"
			end
			log:trace(
				"_requestMultipart decode success: decodedType="
					.. tostring(type(decodedOrErr))
					.. " hasStatus="
					.. tostring(type(decodedOrErr) == "table" and decodedOrErr.status or "n/a")
			)
			return decodedOrErr
		end
		log:trace("_requestMultipart success with empty body")
		return {} -- Return an empty table for successful but empty responses
	else
		local err_msg = "API request failed. HTTP status: " .. httpStatusForLog(status, hdrs)
		if result and #result > 0 then
			local ok, decoded_err = LrTasks.pcall(function()
				return JSON:decode(result)
			end)
			if ok and type(decoded_err) == "table" and decoded_err.error then
				err_msg = err_msg .. " - " .. decoded_err.error
			else
				err_msg = err_msg .. " Response: " .. tostring(result)
			end
		end
		log:error(err_msg)
		return nil, err_msg
	end
end

_request = function(method, url, body, timeout, options)
	options = options or {}
	local result, hdrs
	local bodyString = (body and type(body) == "table") and JSON:encode(body) or nil

	local ok, err = LrTasks.pcall(function()
		if method == "GET" then
			if timeout ~= nil then
				result, hdrs = LrHttp.get(tostring(url), nil, timeout)
			else
				result, hdrs = LrHttp.get(tostring(url))
			end
		else
			result, hdrs = LrHttp.post(
				tostring(url),
				bodyString or "",
				{ { field = "Content-Type", value = "application/json" } },
				method,
				timeout
			)
		end
	end)

	if not ok then
		log:error("_request network error: " .. tostring(err))
		return nil, tostring(err)
	end

	local status = (type(hdrs) == "number") and hdrs or (type(hdrs) == "table" and hdrs.status) or nil
	if status ~= nil and status >= 200 and status < 300 then
		if options.raw then
			return result, hdrs
		end
		if result and #result > 0 then
			log:trace("_request: decoding JSON result of length " .. #result)
			local ok2, decoded = LrTasks.pcall(JSON.decode, JSON, result)
			if ok2 then
				return decoded
			else
				local snippet = tostring(result):sub(1, 1000)
				log:error(
					"_request: JSON decode failed: "
						.. tostring(decoded)
						.. " | URL: "
						.. tostring(url)
						.. " | Raw Snippet: "
						.. snippet
				)
				return nil, "JSON decode failed: " .. tostring(decoded)
			end
		end
		return {}
	else
		log:trace("_request: status=" .. tostring(status) .. " type(hdrs)=" .. type(hdrs))
		local statusStr = httpStatusForLog(status, hdrs)
		local err_msg
		if status == nil then
			local urlFixed = tostring(url):gsub("%%?.*", "")
			err_msg = "API request failed (no response). URL: " .. urlFixed
			if type(hdrs) == "string" and hdrs ~= "" then
				err_msg = err_msg .. " - error: " .. hdrs
			end
		else
			err_msg = "API request failed. HTTP status: " .. statusStr
			if result and #result > 0 then
				local ok2, decoded_err = LrTasks.pcall(JSON.decode, JSON, result)
				if ok2 and type(decoded_err) == "table" and decoded_err.error then
					err_msg = err_msg .. " - " .. decoded_err.error
				else
					err_msg = err_msg .. " Response: " .. tostring(result):sub(1, 400)
				end
			end
		end
		log:error(err_msg)
		return nil, err_msg
	end
end

---
-- Gets photos that need processing for "New or unprocessed photos" scope.
-- When taskOptions is provided, uses backend to check which photos lack the selected tasks' data.
-- When taskOptions is nil, falls back to legacy behavior: photos not in index (with embeddings).
-- @param taskOptions table|nil { enableEmbeddings, enableMetadata, enableFaces, enableVertexAI, regenerateMetadata }
-- @param lookupProgressScope LrProgressScope|nil Optional progress for "looking up which photos need processing".
-- @return boolean success, table photosToProcess
--
function SearchIndexAPI.getMissingPhotosFromIndex(taskOptions, lookupProgressScope)
	local allPhotos = PhotoSelector.getPhotosInScope("all")
	if allPhotos == nil then
		ErrorHandler.handleError("No photos found in catalog", "Something went wrong")
		return false, {}
	end

	local totalCatalog = #allPhotos
	local function updateLookupProgress(current, total)
		if lookupProgressScope and not lookupProgressScope:isCanceled() then
			lookupProgressScope:setPortionComplete(current, total)
			lookupProgressScope:setCaption(
				LOC(
					"$$$/LrGeniusAI/AnalyzeAndIndex/LookupProgress=Looking up which photos need processing... ^1/^2",
					tostring(current),
					tostring(total)
				)
			)
		end
	end

	-- New behavior: use backend to check which photos need processing based on selected tasks
	if taskOptions and type(taskOptions) == "table" then
		if lookupProgressScope then
			lookupProgressScope:setCaption(
				LOC("$$$/LrGeniusAI/AnalyzeAndIndex/LookupPhase1=Preparing catalog photos for lookup...")
			)
			lookupProgressScope:setPortionComplete(0, totalCatalog)
		end

		local photoIds = {}
		local updateInterval = math.max(1, math.floor(totalCatalog / 50))
		for i, photo in ipairs(allPhotos) do
			if lookupProgressScope and lookupProgressScope:isCanceled() then
				return false, {}
			end
			local photoId, idErr = getPhotoIdForPhoto(photo)
			if photoId then
				table.insert(photoIds, photoId)
			else
				log:error("Could not compute photo_id for missing-check: " .. tostring(idErr))
			end
			if i % updateInterval == 0 or i == totalCatalog then
				updateLookupProgress(i, totalCatalog)
			end
		end
		if #photoIds == 0 then
			return true, {}
		end

		if lookupProgressScope then
			lookupProgressScope:setCaption(
				LOC("$$$/LrGeniusAI/AnalyzeAndIndex/LookupPhase2=Checking server for unprocessed photos...")
			)
		end

		local tasks = {}
		if taskOptions.enableEmbeddings then
			table.insert(tasks, "embeddings")
		end
		if taskOptions.enableMetadata then
			table.insert(tasks, "metadata")
		end
		if taskOptions.enableFaces then
			table.insert(tasks, "faces")
		end
		if taskOptions.enableVertexAI then
			table.insert(tasks, "vertexai")
		end

		local body = {
			photo_ids = photoIds,
			tasks = tasks,
			regenerate_metadata = taskOptions.regenerateMetadata or false,
		}
		local checkCid = getCatalogId()
		if checkCid then
			body.catalog_id = checkCid
		end
		local result, err = _request("POST", getBaseUrl() .. ENDPOINTS.CHECK_UNPROCESSED, body)
		if err then
			ErrorHandler.handleError("Failed to check unprocessed photos", err)
			return false, {}
		end

		local needingPhotoIds = result and (result.photo_ids or result.uuids) or {}
		local photoIdSet = {}
		for _, pid in ipairs(needingPhotoIds) do
			photoIdSet[pid] = true
		end

		if lookupProgressScope then
			lookupProgressScope:setCaption(
				LOC("$$$/LrGeniusAI/AnalyzeAndIndex/LookupPhase3=Matching photos to process...")
			)
			lookupProgressScope:setPortionComplete(0, totalCatalog)
		end

		local photosToProcess = {}
		for i, photo in ipairs(allPhotos) do
			if lookupProgressScope and lookupProgressScope:isCanceled() then
				return false, {}
			end
			local photoId = getPhotoIdForPhoto(photo)
			if photoIdSet[photoId] then
				table.insert(photosToProcess, photo)
			end
			if i % updateInterval == 0 or i == totalCatalog then
				updateLookupProgress(i, totalCatalog)
			end
		end
		return true, photosToProcess
	end

	-- Legacy: photos not in index (optionally requiring real embeddings)
	local requireEmbeddings = (taskOptions == true)
	local indexedPhotoIds, err = SearchIndexAPI.getAllIndexedPhotoIds(requireEmbeddings)
	if err then
		ErrorHandler.handleError("Failed to retrieve indexed photos", err)
		return false, {}
	end

	local photosToProcess = {}
	for _, photo in ipairs(allPhotos) do
		local photoId = getPhotoIdForPhoto(photo)
		if photoId and not Util.table_contains(indexedPhotoIds, photoId) then
			table.insert(photosToProcess, photo)
		end
	end
	return true, photosToProcess
end

---
-- Run face clustering to group similar faces into persons.
-- @param distanceThreshold number Optional cosine distance; default 0.5. Use 0.45 if over-merge; 0.55-0.65 if same person split.
-- @return table|nil { status, person_count, face_count, updated } or nil, err
function SearchIndexAPI.clusterFaces(distanceThreshold)
	local url = getBaseUrl() .. ENDPOINTS.FACES_CLUSTER
	local body = {}
	if distanceThreshold and type(distanceThreshold) == "number" then
		body.distance_threshold = distanceThreshold
	end
	local result, err = _request("POST", url, body)
	if err then
		log:error("clusterFaces failed: " .. err)
		return nil, err
	end
	return result
end

---
-- Get list of all persons (face clusters) with name, face_count, photo_count (no thumbnails).
-- @return table|nil { status, persons = { { person_id, name, face_count, photo_count }, ... } } or nil, err
function SearchIndexAPI.getPersons()
	local url = getBaseUrl() .. ENDPOINTS.FACES_PERSONS
	local result, err = _request("GET", url)
	if err then
		log:error("getPersons failed: " .. err)
		return nil, err
	end
	return result
end

---
-- Get base64 JPEG thumbnail for one person (lazy load). GET /faces/persons/<id>/thumbnail
-- @return table|nil { status, person_id, thumbnail } or nil, err
function SearchIndexAPI.getPersonThumbnail(personId)
	if not personId or personId == "" then
		return nil, "person_id required"
	end
	local url = getBaseUrl() .. ENDPOINTS.FACES_PERSON_PHOTOS .. "/" .. personId .. "/thumbnail"
	local result, err = _request("GET", url)
	if err then
		log:error("getPersonThumbnail failed: " .. err)
		return nil, err
	end
	return result
end

---
-- Set display name for a person.
-- @param personId string e.g. "person_0"
-- @param name string Display name (empty to clear)
-- @return boolean success, err
function SearchIndexAPI.setPersonName(personId, name)
	if not personId or personId == "" then
		return false, "person_id required"
	end
	local url = getBaseUrl() .. ENDPOINTS.FACES_PERSON_PHOTOS .. "/" .. personId
	local _, err = _request("PUT", url, { name = name or "" })
	if err then
		log:error("setPersonName failed: " .. err)
		return false, err
	end
	return true
end

---
-- Get photo UUIDs that contain this person.
-- @param personId string e.g. "person_0"
-- @return table|nil { status, person_id, photo_uuids } or nil, err
function SearchIndexAPI.getPhotosForPerson(personId)
	if not personId or personId == "" then
		return nil, "person_id required"
	end
	local url = getBaseUrl() .. ENDPOINTS.FACES_PERSON_PHOTOS .. "/" .. personId .. "/photos"
	local result, err = _request("GET", url, {})
	if err then
		log:error("getPhotosForPerson failed: " .. err)
		return nil, err
	end
	return result
end

---
-- Detect all faces in an image (base64). Returns list of { thumbnail, index } for selection.
-- @param imageBase64 string Base64-encoded image
-- @return table|nil { status, faces = [ { thumbnail, index }, ... ] } or nil, err
function SearchIndexAPI.detectFacesInImage(imageBase64)
	if not imageBase64 or imageBase64 == "" then
		return nil, "image (base64) required"
	end
	local url = getBaseUrl() .. ENDPOINTS.FACES_DETECT
	local result, err = _request("POST", url, { image = imageBase64 })
	if err then
		log:error("detectFacesInImage failed: " .. err)
		return nil, err
	end
	return result
end

---
-- Find indexed faces similar to the selected face in the image.
-- @param imageBase64 string Base64-encoded image
-- @param faceIndex number 0-based index of the face to use (default 0)
-- @param nResults number Max results (default 500 for full cluster)
-- @return table|nil { status, results = [ { face_id, photo_uuid, thumbnail, person_id, distance }, ... ] } or nil, err
function SearchIndexAPI.queryFacesByImage(imageBase64, faceIndex, nResults)
	if not imageBase64 or imageBase64 == "" then
		return nil, "image (base64) required"
	end
	local url = getBaseUrl() .. ENDPOINTS.FACES_QUERY
	local body = { image = imageBase64 }
	if faceIndex ~= nil and type(faceIndex) == "number" then
		body.face_index = faceIndex
	end
	if nResults ~= nil and type(nResults) == "number" then
		body.n_results = nResults
	end
	local result, err = _request("POST", url, body)
	if err then
		log:error("queryFacesByImage failed: " .. err)
		return nil, err
	end
	return result
end

function SearchIndexAPI.saveThumbnail(uuid, faceIndex, base64Data)
	local tempDir = LrPathUtils.getStandardFilePath("temp")
	local tempFile = LrPathUtils.child(tempDir, uuid .. "_" .. faceIndex .. ".jpg")
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
	local url = getBaseUrl() .. ENDPOINTS.MODELS
	local body = {
		openai_apikey = openaiApiKey,
		gemini_apikey = geminiApiKey,
		ollama_base_url = (prefs and prefs.ollamaBaseUrl) or nil,
		lmstudio_base_url = (prefs and prefs.lmstudioBaseUrl) or nil,
	}
	local result = _request("POST", url, body)
	return result
end

---
-- Downloads a raw log file from the server directly to a local path on disk.
-- Bypasses JSON parsing to avoid memory exhaustion for large logs.
-- @param logType string 'backend', 'ollama', or 'lmstudio'
-- @param targetPath string local file path to save to
-- @return boolean success
function SearchIndexAPI.downloadRawLog(logType, targetPath)
	if not logType or not targetPath then
		return false
	end

	local url = getBaseUrl() .. ENDPOINTS.LOGS_RAW .. "/" .. tostring(logType)
	log:trace("Downloading raw " .. logType .. " log from: " .. url)

	local ok, res, hdrs = LrTasks.pcall(function()
		return LrHttp.get(url, nil, 60)
	end)

	-- Status can be in hdrs.status (table) or hdrs itself (number) depending on LR version
	local status = (type(hdrs) == "table" and hdrs.status) or (type(hdrs) == "number" and hdrs) or nil

	if ok and status == 200 and res then
		local f, err = io.open(targetPath, "wb")
		if f then
			f:write(res)
			f:close()
			log:trace("Successfully downloaded and saved raw log to: " .. targetPath)
			return true
		else
			log:error("Failed to open target path for writing: " .. tostring(err))
		end
	else
		log:error(
			"Failed to download raw log: status="
				.. tostring(status)
				.. " ok="
				.. tostring(ok)
				.. " hdrsType="
				.. type(hdrs)
		)
	end
	return false
end

---
-- Migrates existing server-side photo UUID entries to the new photo_id values.
-- Builds mappings from current catalog photos: old_id=Lightroom UUID, new_id=global photo_id.
-- @return boolean success, string message
function SearchIndexAPI.migratePhotoIdsFromCatalog()
	local migrationStartedAt = LrDate.currentTime()
	log:info("migratePhotoIdsFromCatalog: started")

	if not SearchIndexAPI.pingServer() then
		log:error("migratePhotoIdsFromCatalog: backend server not reachable")
		return false, "Backend server is not reachable."
	end

	local indexedIds = SearchIndexAPI.getAllIndexedPhotoIds()
	if type(indexedIds) ~= "table" then
		log:error("migratePhotoIdsFromCatalog: could not retrieve indexed IDs")
		return false, "Could not retrieve indexed IDs from backend."
	end
	log:info("migratePhotoIdsFromCatalog: indexed IDs fetched: " .. tostring(#indexedIds))

	local indexedIdSet = {}
	for _, id in ipairs(indexedIds) do
		indexedIdSet[id] = true
	end

	local catalog = LrApplication.activeCatalog()
	local photos = catalog:getAllPhotos() or {}
	local totalPhotos = #photos
	if totalPhotos == 0 then
		log:info("migratePhotoIdsFromCatalog: no photos in catalog")
		return true, "No photos found in catalog."
	end
	log:info("migratePhotoIdsFromCatalog: catalog photos to inspect: " .. tostring(totalPhotos))

	local progressScope = LrProgressScope({
		title = "Migrating photo IDs...",
		functionContext = nil,
	})

	local mappings = {}
	local skipped = 0
	local skippedNotIndexed = 0
	local skippedAlreadyMigrated = 0

	for i, photo in ipairs(photos) do
		if progressScope:isCanceled() then
			progressScope:done()
			return false, "Migration canceled."
		end

		local legacyUuid = photo:getRawMetadata("uuid")
		if not legacyUuid or legacyUuid == "" or not indexedIdSet[legacyUuid] then
			skipped = skipped + 1
			skippedNotIndexed = skippedNotIndexed + 1
		else
			local photoId, photoIdErr = getPhotoIdForPhoto(photo)
			if photoId and photoId ~= "" and legacyUuid ~= photoId then
				if indexedIdSet[photoId] then
					skipped = skipped + 1
					skippedAlreadyMigrated = skippedAlreadyMigrated + 1
				else
					table.insert(mappings, {
						old_id = legacyUuid,
						new_id = photoId,
					})
				end
			else
				skipped = skipped + 1
				if photoIdErr then
					log:warn("Could not compute photo_id during migration prep: " .. tostring(photoIdErr))
				end
			end
		end

		progressScope:setPortionComplete(i, totalPhotos)
		progressScope:setCaption("Preparing migration mappings " .. tostring(i) .. "/" .. tostring(totalPhotos))
		if i % 250 == 0 then
			log:trace(
				"migratePhotoIdsFromCatalog: prep progress "
					.. tostring(i)
					.. "/"
					.. tostring(totalPhotos)
					.. " mappings="
					.. tostring(#mappings)
					.. " skippedNotIndexed="
					.. tostring(skippedNotIndexed)
					.. " skippedAlreadyMigrated="
					.. tostring(skippedAlreadyMigrated)
			)
		end
	end

	if #mappings == 0 then
		progressScope:done()
		log:info(
			"migratePhotoIdsFromCatalog: no mappings prepared. skippedNotIndexed="
				.. tostring(skippedNotIndexed)
				.. " skippedAlreadyMigrated="
				.. tostring(skippedAlreadyMigrated)
				.. " skippedTotal="
				.. tostring(skipped)
		)
		return true, "No migration needed. All photos are already using photo_id."
	end
	log:info("migratePhotoIdsFromCatalog: mappings prepared: " .. tostring(#mappings))

	local batchSize = 250
	local migratedTotal = 0
	local missingOldTotal = 0
	local conflictTotal = 0
	local errorTotal = 0

	for startIdx = 1, #mappings, batchSize do
		if progressScope:isCanceled() then
			progressScope:done()
			return false, "Migration canceled."
		end

		local stopIdx = math.min(startIdx + batchSize - 1, #mappings)
		local batch = {}
		for i = startIdx, stopIdx do
			table.insert(batch, mappings[i])
		end

		local response, err = _request("POST", getBaseUrl() .. ENDPOINTS.MIGRATE_PHOTO_IDS, {
			mappings = batch,
			overwrite = false,
			dry_run = false,
			update_faces = true,
			update_vertex = true,
		}, 300)

		if err then
			progressScope:done()
			log:error(
				"migratePhotoIdsFromCatalog: batch failed at "
					.. tostring(startIdx)
					.. "-"
					.. tostring(stopIdx)
					.. " err="
					.. tostring(err)
			)
			return false, "Migration request failed: " .. tostring(err)
		end

		local summary = (response and response.summary) or {}
		migratedTotal = migratedTotal + (summary.migrated or 0)
		missingOldTotal = missingOldTotal + (summary.missing_old or 0)
		conflictTotal = conflictTotal + (summary.conflicts or 0)
		errorTotal = errorTotal + (summary.errors or 0)

		log:trace(
			"migratePhotoIdsFromCatalog: batch "
				.. tostring(startIdx)
				.. "-"
				.. tostring(stopIdx)
				.. " migrated="
				.. tostring(summary.migrated or 0)
				.. " missing_old="
				.. tostring(summary.missing_old or 0)
				.. " conflicts="
				.. tostring(summary.conflicts or 0)
				.. " errors="
				.. tostring(summary.errors or 0)
		)

		progressScope:setPortionComplete(stopIdx, #mappings)
		progressScope:setCaption("Migrating photo IDs " .. tostring(stopIdx) .. "/" .. tostring(#mappings))
	end

	progressScope:done()
	local migrationElapsedMs = math.floor((LrDate.currentTime() - migrationStartedAt) * 1000)

	local msg = "Migration finished.\n"
		.. "Indexed IDs in backend: "
		.. tostring(#indexedIds)
		.. "\n"
		.. "Mappings prepared: "
		.. tostring(#mappings)
		.. "\n"
		.. "Migrated: "
		.. tostring(migratedTotal)
		.. "\n"
		.. "Missing old IDs: "
		.. tostring(missingOldTotal)
		.. "\n"
		.. "Conflicts: "
		.. tostring(conflictTotal)
		.. "\n"
		.. "Errors: "
		.. tostring(errorTotal)
		.. "\n"
		.. "Skipped (not indexed in backend): "
		.. tostring(skippedNotIndexed)
		.. "\n"
		.. "Skipped (already migrated): "
		.. tostring(skippedAlreadyMigrated)
		.. "\n"
		.. "Skipped in catalog prep: "
		.. tostring(skipped)
	log:info(
		"migratePhotoIdsFromCatalog: finished elapsedMs="
			.. tostring(migrationElapsedMs)
			.. " prepared="
			.. tostring(#mappings)
			.. " migrated="
			.. tostring(migratedTotal)
			.. " missing_old="
			.. tostring(missingOldTotal)
			.. " conflicts="
			.. tostring(conflictTotal)
			.. " errors="
			.. tostring(errorTotal)
			.. " skippedTotal="
			.. tostring(skipped)
	)
	return errorTotal == 0, msg
end

---
-- Generates hash-based global photo IDs for all photos in the current catalog
-- and writes them to the catalog-only plugin fields, without touching the backend.
-- Uses Util.getGlobalPhotoIdForPhoto() which will reuse cached IDs when present.
-- @return boolean success, string message
--
function SearchIndexAPI.generateGlobalPhotoIdsForCatalog()
	local startedAt = LrDate.currentTime()
	log:info("generateGlobalPhotoIdsForCatalog: started")

	local catalog = LrApplication.activeCatalog()
	local photos = catalog:getAllPhotos() or {}
	local totalPhotos = #photos

	if totalPhotos == 0 then
		log:info("generateGlobalPhotoIdsForCatalog: no photos in catalog")
		return true, "No photos found in catalog."
	end

	log:info("generateGlobalPhotoIdsForCatalog: catalog photos to inspect: " .. tostring(totalPhotos))

	local progressScope = LrProgressScope({
		title = "Generating hash-based photo IDs in catalog...",
		functionContext = nil,
	})

	local generated = 0
	local reused = 0
	local errors = 0

	for i, photo in ipairs(photos) do
		if progressScope:isCanceled() then
			progressScope:done()
			log:info(
				"generateGlobalPhotoIdsForCatalog: canceled by user at " .. tostring(i) .. "/" .. tostring(totalPhotos)
			)
			return false, "Photo-ID generation canceled."
		end

		local hadExistingId = not Util.nilOrEmpty(photo:getPropertyForPlugin(_PLUGIN, "globalPhotoId"))

		local photoId, err = Util.getGlobalPhotoIdForPhoto(photo, {
			windowBytes = Util.getDefaultPartialHashWindowBytes(),
		})

		if photoId and photoId ~= "" then
			if hadExistingId then
				reused = reused + 1
			else
				generated = generated + 1
			end
		else
			errors = errors + 1
			log:warn("generateGlobalPhotoIdsForCatalog: failed to compute ID for photo: " .. tostring(err))
		end

		progressScope:setPortionComplete(i, totalPhotos)
		if i % 250 == 0 or i == totalPhotos then
			progressScope:setCaption("Generating hash-based photo IDs " .. tostring(i) .. "/" .. tostring(totalPhotos))
		end
	end

	progressScope:done()

	local elapsedMs = math.floor((LrDate.currentTime() - startedAt) * 1000)
	local msg = "Photo-ID generation finished.\n"
		.. "Catalog photos: "
		.. tostring(totalPhotos)
		.. "\n"
		.. "New IDs generated: "
		.. tostring(generated)
		.. "\n"
		.. "Existing IDs reused: "
		.. tostring(reused)
		.. "\n"
		.. "Errors: "
		.. tostring(errors)

	log:info(
		"generateGlobalPhotoIdsForCatalog: finished elapsedMs="
			.. tostring(elapsedMs)
			.. " generated="
			.. tostring(generated)
			.. " reused="
			.. tostring(reused)
			.. " errors="
			.. tostring(errors)
	)

	return errors == 0, msg
end

function SearchIndexAPI.startClipDownload()
	if SearchIndexAPI.isClipReady() then
		log:trace("CLIP model is already cached")
		return
	end

	local status, err = _request("GET", getBaseUrl() .. ENDPOINTS.STATUS_CLIP_DOWNLOAD)
	if not err and status ~= nil and status.status == "downloading" then
		log:trace("CLIP model download is already in progress")
		return
	end

	local progressScope = LrProgressScope({
		title = LOC("$$$/LrGeniusAI/ClipDownload/ProgressTitle=Downloading CLIP AI model for advanced search"),
		functionContext = nil,
	})

	local url = getBaseUrl() .. ENDPOINTS.START_CLIP_DOWNLOAD
	local body = {}

	local _, postErr = _request("POST", url, body)

	if postErr then
		log:error("startClipDownload failed: " .. postErr)
		return nil, postErr
	end

	LrTasks.startAsyncTask(function()
		while true do
			local loopStatus, loopErr = _request("GET", getBaseUrl() .. ENDPOINTS.STATUS_CLIP_DOWNLOAD)
			if loopErr then
				ErrorHandler.handleError("Error downloading CLIP model", loopErr)
				if progressScope ~= nil then
					progressScope:setCaption(
						LOC("$$$/LrGeniusAI/ClipDownload/Error=Error downloading CLIP model: ^1"),
						loopErr
					)
					progressScope:done()
				end
				break
			end

			if loopStatus ~= nil then
				if progressScope ~= nil then
					progressScope:setCaption(LOC("$$$/LrGeniusAI/ClipDownload/Downloading=Downloading CLIP model..."))
				end
				if loopStatus.status == "downloading" then
					progressScope:setPortionComplete(loopStatus.progress, loopStatus.total)
				elseif loopStatus.status == "completed" then
					log:trace("CLIP model download completed")
					progressScope:done()
					LrDialogs.message(
						LOC("$$$/LrGeniusAI/ClipDownload/SuccessTitle=CLIP Download"),
						LOC("$$$/LrGeniusAI/ClipDownload/SuccessMessage=CLIP model downloaded successfully.")
					)
					break
				elseif
					loopStatus.status == "error"
					or (loopStatus.error and loopStatus.error ~= "null" and loopStatus.error ~= "")
				then
					local error_msg = loopStatus.error or "Unknown download error"
					ErrorHandler.handleError(
						LOC("$$$/LrGeniusAI/ClipDownload/ErrorTitle=Error downloading CLIP model"),
						error_msg
					)
					progressScope:done()
					break
				end
			end

			LrTasks.sleep(2)
		end
	end)
end

local lastClipReadyStatus = nil
function SearchIndexAPI.isClipReady()
	local url = getBaseUrl() .. ENDPOINTS.CLIP_STATUS
	local res, err = _request("GET", url)
	if err then
		local errStr = (type(err) == "string") and err or "unknown"
		log:error("isClipReady failed: " .. errStr)
		return false, errStr
	end
	if res ~= nil then
		local currentStatus = res.clip
		if currentStatus ~= lastClipReadyStatus then
			if currentStatus == "ready" then
				log:trace("CLIP model is ready")
			else
				log:trace("CLIP model is not ready: " .. tostring(res.message or "no message"))
			end
			lastClipReadyStatus = currentStatus
		end

		if currentStatus == "ready" then
			return true, res.message
		else
			return false, res.message
		end
	end
	log:error("isClipReady: Unknown error")
	return false, "Unknown error"
end

---
-- Checks the health of the backend server and its components (models, providers).
-- Surfaces critical loading failures to the user.
--
function SearchIndexAPI.checkServerHealth()
	local url = getBaseUrl() .. ENDPOINTS.HEALTH
	local res, err = _request("GET", url)
	if err then
		log:warn("checkServerHealth failed (could not reach /health): " .. tostring(err))
		return false, err
	end

	if res then
		-- 1. Check CLIP model
		if res.clip_model == "failed" then
			ErrorHandler.handleError(
				LOC("$$$/LrGeniusAI/Health/ClipFailed=AI search model failed to load."),
				res.clip_error or "Unknown error loading CLIP model."
			)
		end

		-- 2. Check Face model
		if res.face_model == "failed" then
			log:warn("Face detection model failed to load on server: " .. tostring(res.face_error))
		end

		-- 3. Check LLM providers
		local providers = res.llm_providers or {}
		local hasAvailable = false
		local failedProviders = {}
		for provider, status in pairs(providers) do
			if status == "available" or status == "registered" then
				hasAvailable = true
			elseif status == "failed" then
				table.insert(
					failedProviders,
					provider .. ": " .. (res.llm_errors and res.llm_errors[provider] or "unknown error")
				)
			end
		end

		if not hasAvailable and next(providers) ~= nil then
			ErrorHandler.handleError(
				LOC("$$$/LrGeniusAI/Health/NoProviders=No AI metadata providers are available."),
				LOC(
					"$$$/LrGeniusAI/Health/NoProvidersDetail=Please configure Ollama, LM Studio, ChatGPT, or Gemini in the plugin preferences."
				)
			)
		elseif #failedProviders > 0 then
			log:warn("Some AI providers failed to initialize: " .. table.concat(failedProviders, ", "))
		end
	end

	return true
end

function SearchIndexAPI.diagnoseStartupFailure()
	local results = {
		binaryMissing = false,
		portBusy = false,
		logSnippet = nil,
	}

	-- 1. Check binary existence
	local serverDir = LrPathUtils.child(LrPathUtils.parent(_PLUGIN.path), "lrgenius-server")
	local serverBinary = LrPathUtils.child(serverDir, "lrgenius-server")
	if WIN_ENV then
		local serverLauncherCmd = serverBinary .. ".cmd"
		local serverExe = serverBinary .. ".exe"
		if LrFileUtils.exists(serverLauncherCmd) then
			serverBinary = serverLauncherCmd
		else
			serverBinary = serverExe
		end
	end

	if not LrFileUtils.exists(serverBinary) then
		results.binaryMissing = true
		return results
	end

	-- 2. Check port 19819 (Mac only for now)
	if MAC_ENV then
		local status, output = LrTasks.pcall(function()
			return LrTasks.execute('bash -c "lsof -i :19819 | grep LISTEN"')
		end)
		if status and output and output ~= "" then
			results.portBusy = true
		end
	end

	-- 3. Check logs for errors
	local logPath = LrPathUtils.child(getServerControlDir(), "lrgenius-server.log")
	if LrFileUtils.exists(logPath) then
		local f = io.open(logPath, "r")
		if f then
			local content = f:read("*all")
			f:close()
			local lines = {}
			for line in content:gmatch("[^\r\n]+") do
				table.insert(lines, line)
			end
			local start = math.max(1, #lines - 10)
			local snippet = {}
			for i = start, #lines do
				table.insert(snippet, lines[i])
			end
			results.logSnippet = table.concat(snippet, "\n")
		end
	end

	return results
end

function SearchIndexAPI.getDetailedHealth()
	local health = {
		backend = SearchIndexAPI.pingServer() == true,
		clip = SearchIndexAPI.isClipReady() == true,
		gemini = not Util.nilOrEmpty(prefs.geminiApiKey),
		chatgpt = not Util.nilOrEmpty(prefs.chatgptApiKey),
		ollama = false,
		lmstudio = false,
	}

	-- Try to ping local LLMs if they are not default localhost but maybe they are
	if not Util.nilOrEmpty(prefs.ollamaBaseUrl) then
		local url = prefs.ollamaBaseUrl .. "/api/tags"
		local _, hdrs = LrHttp.get(url, nil, 500)
		local status = (type(hdrs) == "number") and hdrs or (type(hdrs) == "table" and hdrs.status)
		if status == 200 then
			health.ollama = true
		end
	end

	if not Util.nilOrEmpty(prefs.lmstudioBaseUrl) then
		local baseUrl = prefs.lmstudioBaseUrl
		if not baseUrl:match("^https?://") then
			baseUrl = "http://" .. baseUrl
		end
		local url = baseUrl .. "/v1/models"
		local _, hdrs = LrHttp.get(url, nil, 500)
		local status = (type(hdrs) == "number") and hdrs or (type(hdrs) == "table" and hdrs.status)
		if status == 200 then
			health.lmstudio = true
		end
	end

	return health
end

-- ---------------------------------------------------------------------------
-- Training API functions
-- ---------------------------------------------------------------------------

---
-- Add or update a training example on the backend.
-- @param photoId string        Stable photo identifier.
-- @param filepath string       Path to an exported JPEG for this photo.
-- @param developSettings table Lightroom develop settings (from photo:getDevelopSettings()).
-- @param options table         Optional: label, summary.
-- @return boolean success, table|string response or error message
---
function SearchIndexAPI.addTrainingExample(photoId, filepath, developSettings, options)
	if not photoId or photoId == "" then
		log:error("addTrainingExample: photo_id is missing")
		return false, "No photo ID provided"
	end
	options = options or {}
	local url = getBaseUrl() .. ENDPOINTS.TRAINING_ADD
	local mimeChunks = {}

	table.insert(mimeChunks, { name = "photo_id", value = photoId })
	table.insert(mimeChunks, { name = "develop_settings", value = JSON:encode(developSettings or {}) })

	if options.label and options.label ~= "" then
		table.insert(mimeChunks, { name = "label", value = options.label })
	end
	if options.summary and options.summary ~= "" then
		table.insert(mimeChunks, { name = "summary", value = options.summary })
	end

	-- Send EXIF fields for richer multi-criteria matching.
	if options.focal_length and type(options.focal_length) == "number" then
		table.insert(mimeChunks, { name = "focal_length", value = tostring(options.focal_length) })
	end
	if options.capture_time and type(options.capture_time) == "number" then
		table.insert(mimeChunks, { name = "capture_time", value = tostring(options.capture_time) })
	end
	if options.camera_make and options.camera_make ~= "" then
		table.insert(mimeChunks, { name = "camera_make", value = tostring(options.camera_make) })
	end
	if options.camera_model and options.camera_model ~= "" then
		table.insert(mimeChunks, { name = "camera_model", value = tostring(options.camera_model) })
	end
	if options.iso and type(options.iso) == "number" then
		table.insert(mimeChunks, { name = "iso", value = tostring(options.iso) })
	end
	if options.aperture and type(options.aperture) == "number" then
		table.insert(mimeChunks, { name = "aperture", value = tostring(options.aperture) })
	end
	if options.shutter_speed and options.shutter_speed ~= "" then
		table.insert(mimeChunks, { name = "shutter_speed", value = tostring(options.shutter_speed) })
	end

	if filepath and LrFileUtils.exists(filepath) then
		local filename = LrPathUtils.leafName(filepath)
		table.insert(mimeChunks, {
			name = "image",
			fileName = filename,
			filePath = filepath,
			contentType = "image/jpeg",
		})
	end

	log:trace("addTrainingExample: uploading photo_id=" .. tostring(photoId))
	local response, err = _requestMultipart(url, mimeChunks, 120)
	if not response then
		log:error("addTrainingExample failed: " .. tostring(err))
		return false, err or "Unknown error"
	end
	if response.status == "ok" then
		return true, response
	end
	log:error("addTrainingExample unexpected status: " .. tostring(response.status))
	return false, response.error or "Unexpected response"
end

---
-- Fetch the list of all training examples from the backend.
-- @return boolean success, table|string examples list or error message
---
function SearchIndexAPI.listTrainingExamples()
	local url = getBaseUrl() .. ENDPOINTS.TRAINING_LIST
	local response, err = _request("GET", url)
	if not response then
		log:error("listTrainingExamples failed: " .. tostring(err))
		return false, err or "Unknown error"
	end
	return true, response.examples or {}
end

---
-- Get the count of stored training examples.
-- @return number|nil count, string|nil error
---
function SearchIndexAPI.getTrainingCount()
	local url = getBaseUrl() .. ENDPOINTS.TRAINING_COUNT
	local response, err = _request("GET", url)
	if not response then
		log:error("getTrainingCount failed: " .. tostring(err))
		return nil, err or "Unknown error"
	end
	return tonumber(response.count) or 0, nil
end

---
-- Delete one training example by photo_id.
-- @param photoId string
-- @return boolean success, string|nil error
---
function SearchIndexAPI.deleteTrainingExample(photoId)
	if not photoId or photoId == "" then
		return false, "No photo ID provided"
	end
	local url = getBaseUrl() .. ENDPOINTS.TRAINING_DELETE .. "/" .. photoId
	local response, err = _request("DELETE", url)
	if not response then
		log:error("deleteTrainingExample failed: " .. tostring(err))
		return false, err or "Unknown error"
	end
	if response.status == "ok" then
		return true, nil
	end
	return false, response.error or "Not found"
end

---
-- Clear ALL training examples from the backend.
-- @return boolean success, string|nil error
---
function SearchIndexAPI.clearAllTrainingExamples()
	local url = getBaseUrl() .. ENDPOINTS.TRAINING_CLEAR
	local response, err = _request("DELETE", url)
	if not response then
		log:error("clearAllTrainingExamples failed: " .. tostring(err))
		return false, err or "Unknown error"
	end
	if response.status == "ok" then
		return true, nil
	end
	return false, response.error or "Unexpected response"
end

---
-- Get aggregate style-profile statistics from the backend.
-- @return table|nil { count, readiness, scene_distribution, exposure, focal_buckets, time_of_day, ... }
-- @return string|nil error message
---
function SearchIndexAPI.getTrainingStats()
	local url = getBaseUrl() .. ENDPOINTS.TRAINING_STATS
	local response, err = _request("GET", url)
	if not response then
		log:error("getTrainingStats failed: " .. tostring(err))
		return nil, err or "Unknown error"
	end
	return response, nil
end

---
-- Generate a style-matched edit recipe using the LLM-free style engine.
-- Falls back to LLM if use_llm_fallback=true and confidence is low.
-- @param photoId   string  Stable photo ID.
-- @param filepath  string  Path to an exported JPEG preview.
-- @param options   table   Same options as generateEditRecipe; extra keys:
--                           use_llm_fallback (bool), focal_length (number),
--                           capture_time (number unix), camera_make, camera_model,
--                           iso, aperture, shutter_speed.
-- @return boolean success, table|string response or error message
---
function SearchIndexAPI.getRemoteLogs()
	local url = getBaseUrl() .. ENDPOINTS.LOGS
	log:trace("Fetching remote logs from: " .. url)
	local response, err = _request("GET", url, nil, 10)
	log:trace("getRemoteLogs: _request returned type=" .. type(response))
	if not response then
		log:error("Failed to fetch remote logs: " .. tostring(err))
		return nil, err
	end
	return response
end

function SearchIndexAPI.styleEdit(photoId, filepath, options)
	if not photoId or photoId == "" then
		log:error("styleEdit: photo_id missing")
		return false, "No photo ID provided"
	end
	options = options or {}
	local url = getBaseUrl() .. ENDPOINTS.STYLE_EDIT
	local mimeChunks = {}

	table.insert(mimeChunks, { name = "photo_id", value = photoId })

	-- Optional extra EXIF context for the style engine
	local function addStr(key)
		if options[key] and tostring(options[key]) ~= "" then
			table.insert(mimeChunks, { name = key, value = tostring(options[key]) })
		end
	end
	addStr("use_llm_fallback")
	addStr("focal_length")
	addStr("capture_time")
	addStr("camera_make")
	addStr("camera_model")
	addStr("iso")
	addStr("aperture")
	addStr("shutter_speed")

	-- Standard edit options forwarded for LLM fallback compatibility
	local function addEditOpt(key, value)
		if value ~= nil then
			table.insert(mimeChunks, { name = key, value = tostring(value) })
		end
	end
	addEditOpt("provider", options.provider)
	addEditOpt("model", options.model)
	addEditOpt("language", options.language)
	addEditOpt("temperature", options.temperature)
	addEditOpt("include_masks", options.include_masks)
	addEditOpt("adjust_white_balance", options.adjust_white_balance)
	addEditOpt("adjust_basic_tone", options.adjust_basic_tone)
	addEditOpt("adjust_presence", options.adjust_presence)
	addEditOpt("adjust_color_mix", options.adjust_color_mix)
	addEditOpt("do_color_grading", options.do_color_grading)
	addEditOpt("use_tone_curve", options.use_tone_curve)
	addEditOpt("adjust_detail", options.adjust_detail)
	addEditOpt("adjust_effects", options.adjust_effects)
	addEditOpt("allow_auto_crop", options.allow_auto_crop)

	if filepath and LrFileUtils.exists(filepath) then
		local filename = LrPathUtils.leafName(filepath)
		table.insert(mimeChunks, {
			name = "image",
			fileName = filename,
			filePath = filepath,
			contentType = "image/jpeg",
		})
	end

	log:trace("styleEdit: uploading photo_id=" .. tostring(photoId))
	local response, err = _requestMultipart(url, mimeChunks, 180)
	if not response then
		log:error("styleEdit failed: " .. tostring(err))
		return false, err or "Unknown error"
	end
	if response.status == "success" then
		return true, response
	end
	if response.status == "error" then
		return false, response.error or "Style engine error"
	end
	log:error("styleEdit unexpected status: " .. tostring(response.status))
	return false, response.error or "Unexpected response"
end
