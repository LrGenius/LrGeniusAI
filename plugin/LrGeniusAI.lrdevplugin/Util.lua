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

		if next(val) == nil then
			return "{}"
		end -- Handle empty table

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
				table.insert(parts, next_indent .. dumpHelper(v, next_indent, seen))
			end
			return "{\n" .. table.concat(parts, ",\n") .. "\n" .. indent .. "}"
		else -- It's a dictionary-like table
			local sorted_keys = {}
			for k in pairs(val) do
				table.insert(sorted_keys, k)
			end
			-- Sort keys, converting to string for comparison to handle mixed key types (numbers and strings)
			table.sort(sorted_keys, function(a, b)
				return tostring(a) < tostring(b)
			end)

			for _, k in ipairs(sorted_keys) do
				local v = val[k]
				local key_str = (type(k) == "string" and not k:match("^[A-Za-z_][A-Za-z0-9_]*$"))
						and ('["' .. k .. '"]')
					or tostring(k)
				table.insert(parts, next_indent .. key_str .. " = " .. dumpHelper(v, next_indent, seen))
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
	if type(val) == "string" then
		return val == nil or trim(val) == ""
	else
		return val == nil
	end
end

---
-- The text to show the user for an error value, or `fallback` when the value
-- carries no text at all.
--
-- Callers used to write `err or "Something failed"`, which looks like it
-- guarantees a message but does not: the empty string is truthy in Lua, so a
-- backend answering `"error": ""` sailed straight through every one of those
-- guards and produced a dialog with a blank line where the reason belongs.
-- Anything that is not a non-blank string resolves to the fallback instead;
-- a table would otherwise reach the user as "table: 0x7f...".
--
-- @param value any        The error value, from a response or a pcall.
-- @param fallback string  Optional; what to say when `value` says nothing.
-- @return string
---
function Util.errorText(value, fallback)
	fallback = fallback or "No further details were reported."
	if type(value) == "number" then
		return tostring(value)
	end
	if type(value) ~= "string" or Util.nilOrEmpty(value) then
		return fallback
	end
	return value
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
	if ok then
		return value
	end
	return nil
end

local function safeGetFormattedMetadata(photo, key)
	local ok, value = LrTasks.pcall(function()
		return photo:getFormattedMetadata(key)
	end)
	if ok then
		return value
	end
	return nil
end

---
-- Whether a photo is a raw capture, as Lightroom sees it.
--
-- The backend cannot work this out for itself: everything it receives has been
-- normalised to JPEG first, so the original encoding is no longer in the bytes.
-- Lightroom's `fileFormat` is the authoritative answer, and this is the single
-- place that reads it, so the rest of the plug-in and the backend agree on one
-- definition rather than each guessing from a file extension.
--
-- Why it matters: raw holds a stop or more above what its default rendering
-- shows, so blown highlights are recoverable there and gone on a rendered file.
-- The edit guardrails use it to decide how far the white point may be pushed.
--
-- It also decides which scale Lightroom's `Temp` is on: Kelvin for raw, a
-- relative -100..100 for JPEG/TIFF/PNG. Applying the wrong one ruins the photo,
-- so the three answers are kept apart: a readable non-raw format is `false`, but
-- a format we could not read at all is `nil` rather than `false`. Callers treat
-- `nil` as "assume raw", which is what the plug-in did before this existed and
-- what leaves an unreadable raw's white balance alone.
--
-- DNG counts as raw — that is how Lightroom treats it, and a DNG carries the
-- same headroom.
--
-- @param photo LrPhoto The photo object.
-- @return boolean|nil True for RAW and DNG, false for a known other format,
--         nil when the format could not be determined.
--
function Util.isRawPhoto(photo)
	if photo == nil then
		return nil
	end
	local format = safeGetRawMetadata(photo, "fileFormat")
	if type(format) ~= "string" or format == "" then
		return nil
	end
	format = format:upper()
	return format == "RAW" or format == "DNG"
end

---
-- Where the catalog says a photo was taken.
--
-- The backend used to read this out of the image bytes it was handed, which
-- worked only for the small exported JPEGs: normalising a raw original -- or
-- any JPEG over the server's 2048 px limit -- re-encodes it, and the JPEG that
-- comes out carries neither EXIF nor IPTC. So a run with "submit originals"
-- turned on sent no location at all, however carefully the user had set the
-- switch (issue #321). The catalog does not have that problem, and it is also
-- the only source that reflects a place corrected in Lightroom after import.
--
-- Only the confirmed fields are here. Lightroom's reverse-geocoding
-- *suggestions* (the greyed-out ones) are not reachable through the SDK and do
-- not reach the exported file or the XMP sidecar either -- the backend turns
-- the coordinates into a place name itself for exactly that case.
--
-- @param photo LrPhoto The photo object.
-- @return table Fields to merge into a photo's request options; empty when the
--         catalog knows nothing about where the photo was taken.
--
function Util.getPhotoLocation(photo)
	local location = {}
	if photo == nil then
		return location
	end

	local fields = {
		location_sublocation = "location",
		location_city = "city",
		location_state = "stateProvince",
		location_country = "country",
		location_country_code = "isoCountryCode",
	}
	for field, metadataKey in pairs(fields) do
		local value = safeGetFormattedMetadata(photo, metadataKey)
		if type(value) == "string" then
			local cleaned = trim(value)
			if cleaned ~= "" then
				location[field] = cleaned
			end
		end
	end

	-- Both halves or neither: a latitude on its own is not a position, and the
	-- backend would have nothing to look a place name up with.
	local gps = safeGetRawMetadata(photo, "gps")
	if type(gps) == "table" and type(gps.latitude) == "number" and type(gps.longitude) == "number" then
		location.gps_latitude = gps.latitude
		location.gps_longitude = gps.longitude
	end

	return location
end

---
-- Names of the people Lightroom's face recognition has tagged on a photo.
--
-- Lightroom stores a named face as an ordinary keyword whose `keywordType`
-- attribute is "person", so once keywords have been flattened to plain strings
-- a person's name is indistinguishable from a place or an object. That is the
-- bug this exists to fix: handed "Ivo" in the same breath as "beach" and
-- "sunset", a model happily writes "the rocky shore of Ivo Beach" (issue
-- #315). Reading the keyword objects is the only way to tell the two apart.
--
-- Defensive throughout: `getAttributes` is not implemented by every keyword
-- object we may be handed, and a photo whose keywords cannot be read has to
-- degrade to "no face tags known" -- which is exactly the behaviour the
-- plug-in had before this existed -- rather than fail the indexing run.
--
-- @param photo LrPhoto The photo object.
-- @return table Array of person names (and their synonyms), in catalog order,
--         deduplicated.
--
function Util.getPersonKeywordNames(photo)
	if photo == nil then
		return {}
	end
	local keywords = safeGetRawMetadata(photo, "keywords")
	if type(keywords) ~= "table" then
		return {}
	end

	local names = {}
	local seen = {}
	local function remember(value)
		if type(value) ~= "string" then
			return
		end
		local cleaned = trim(value)
		if cleaned ~= "" and not seen[cleaned:lower()] then
			seen[cleaned:lower()] = true
			table.insert(names, cleaned)
		end
	end

	for _, keyword in ipairs(keywords) do
		local ok, attributes = LrTasks.pcall(function()
			return keyword:getAttributes()
		end)
		-- Compared case-insensitively: the SDK documents the value as
		-- "person", and a keyword written by another tool is not worth losing
		-- over its capitalisation.
		local keywordType = ok and type(attributes) == "table" and attributes.keywordType or nil
		if type(keywordType) == "string" and keywordType:lower() == "person" then
			local okName, name = LrTasks.pcall(function()
				return keyword:getName()
			end)
			if okName then
				remember(name)
			end
			-- A person keyword's synonyms are more names for the same person,
			-- and Lightroom writes them into the exported keyword list beside
			-- the name itself. Left out of this list they would stay in the
			-- plain keywords and read as scenery again.
			if type(attributes.synonyms) == "table" then
				for _, synonym in ipairs(attributes.synonyms) do
					remember(synonym)
				end
			end
		end
	end
	return names
end

---
-- Adds the prompts that ship with the plugin to the user's set, once each.
--
-- "Add it if it is missing" would put a preset the user deleted back on every
-- launch; "add it only on a fresh install" would never reach the people who
-- already have the plugin, who are exactly the ones a new preset is for. So
-- what is remembered is not which prompts exist but which have already been
-- offered: seeded once, then the user's set is theirs — edits stay edited and
-- deletions stay deleted.
--
-- @param prompts table|nil Map of prompt name -> instruction (the user's set).
-- @param seeded table|nil Map of prompt name -> true, the names already offered.
-- @param builtins table Array of `{ name = ..., instruction = ... }`, in order.
-- @return table The prompts, with any newly offered built-in added.
-- @return table The record of offered names, updated.
-- @return boolean Whether anything changed and the two are worth storing.
--
function Util.seedBuiltinPrompts(prompts, seeded, builtins)
	local result = prompts or {}
	local offered = seeded or {}
	local changed = prompts == nil or seeded == nil

	for _, preset in ipairs(builtins or {}) do
		if not offered[preset.name] then
			-- Only when the name is free: a user who wrote their own "Default"
			-- keeps it, and it is still marked as offered so this never runs
			-- against that name again.
			if result[preset.name] == nil then
				result[preset.name] = preset.instruction
			end
			offered[preset.name] = true
			changed = true
		end
	end

	return result, offered, changed
end

---
-- Builds the items for a prompt-picker popup menu, in a fixed order.
--
-- Three dialogs each built this list by iterating the prompts table with
-- `pairs`, whose order Lua does not define and which changes between runs.
-- With a single prompt that was invisible; the moment a second one shipped it
-- became a menu that reorders itself behind the user's back.
--
-- @param prompts table Map of prompt name -> instruction text (may be nil).
-- @param firstName string|nil A name to pin to the top (the default prompt).
-- @return table Array of `{ title = name, value = name }`, `firstName` first
--   and the rest alphabetically, compared case-insensitively so "beach" and
--   "Beach" do not depend on the locale's idea of order.
--
function Util.promptMenuItems(prompts, firstName)
	local names = {}
	for name in pairs(prompts or {}) do
		table.insert(names, name)
	end
	table.sort(names, function(a, b)
		if firstName ~= nil then
			if a == firstName then
				return b ~= firstName
			elseif b == firstName then
				return false
			end
		end
		local la, lb = string.lower(a), string.lower(b)
		if la == lb then
			return a < b
		end
		return la < lb
	end)

	local items = {}
	for _, name in ipairs(names) do
		table.insert(items, { title = name, value = name })
	end
	return items
end

---
-- Resolves a prompt name to one that actually exists in `prompts`.
--
-- The prompt picker is a popup_menu whose items are built from the prompt
-- table and whose value is the stored name. Nothing kept the two in step: a
-- prompt could vanish from the table (deleted, or -- see Util.storePromptText
-- -- emptied in a way that dropped its key) while `prefs.prompt` still named
-- it. The menu then has no item matching its value, and what Lightroom does
-- with that is platform-dependent: it may snap the selection to another
-- prompt, which reads as "my edit was not saved", or leave the bound value
-- nil, at which point the dialog's own observer writes `prompts[nil]`, and
-- "table index is nil" raised inside a binding callback takes Lightroom down
-- with it.
--
-- @param prompts table Map of prompt name -> instruction text (may be nil).
-- @param name string|nil The stored selection.
-- @param defaultName string|nil The name to prefer when the selection is gone.
-- @return string|nil A name present in `prompts`, or nil when it holds none.
--
function Util.resolvePromptName(prompts, name, defaultName)
	if type(prompts) ~= "table" then
		return nil
	end
	if type(name) == "string" and prompts[name] ~= nil then
		return name
	end
	if type(defaultName) == "string" and prompts[defaultName] ~= nil then
		return defaultName
	end
	-- Whatever is first in menu order, so the fallback matches what the user
	-- sees at the top of the picker rather than an arbitrary hash order.
	local items = Util.promptMenuItems(prompts, defaultName)
	if items[1] then
		return items[1].value
	end
	return nil
end

---
-- Stores a prompt's text under `name`, keeping the entry alive when the text
-- is empty.
--
-- Two Lua facts meet badly in the dialogs' `selectedPrompt` observer. Writing
-- nil into a table *removes* the key, so a prompt whose text the user cleared
-- stopped existing instead of becoming empty -- the entry disappeared, the
-- menu lost it, and the edit came back on the next open. And `t[nil] = v`
-- raises "table index is nil", so the same observer firing while no prompt is
-- selected throws from inside Lightroom's UI callback.
--
-- Emptying the field is a legitimate edit -- it means "no custom persona,
-- use the backend's own" -- so it is stored as an empty string, and
-- Util.promptForRequest is what turns that into an omitted field on the wire.
--
-- @param prompts table Map of prompt name -> instruction text.
-- @param name string|nil The prompt to write to.
-- @param text string|nil The new text; nil is stored as "".
-- @return boolean True when the text was stored.
--
function Util.storePromptText(prompts, name, text)
	if type(prompts) ~= "table" or type(name) ~= "string" or name == "" then
		return false
	end
	prompts[name] = type(text) == "string" and text or ""
	return true
end

---
-- The prompt text to put on the wire, or nil when there is nothing to send.
--
-- A blank custom prompt is not an instruction to give the model an empty
-- system prompt; it means the user wants no persona of their own. Every
-- backend route already falls back to its built-in default when the field is
-- absent, so blank is sent as absent.
--
-- @param text string|nil The prompt text from the dialog.
-- @return string|nil The trimmed text, or nil when it is blank.
--
function Util.promptForRequest(text)
	if type(text) ~= "string" then
		return nil
	end
	local trimmed = Util.trim(text)
	if trimmed == "" then
		return nil
	end
	return trimmed
end

---
-- Splits a flattened keyword list into ordinary keywords and face tags.
--
-- `keywordTagsForExport` flattens person keywords and the rest of the
-- catalog's keywords into one comma-separated list, which is precisely what
-- makes a person's name readable as scenery. Matching is case-insensitive and
-- on the whole name, so a person called "Ivo" pulls "Ivo" out of the keywords
-- and leaves "Ivory Coast" alone.
--
-- Only names that actually occur in `keywords` are reported as face tags: a
-- person keyword excluded from export was never sent to the model before, and
-- this change is about labelling the context correctly, not about widening it.
--
-- @param keywords table Array of keyword strings (may be nil).
-- @param personNames table Array of person names from Util.getPersonKeywordNames (may be nil).
-- @return table Keywords with every person name removed.
-- @return table The person names found among those keywords, in catalog order.
--
function Util.partitionPersonKeywords(keywords, personNames)
	local plain = {}
	local faces = {}

	local personByLower = {}
	if type(personNames) == "table" then
		for _, name in ipairs(personNames) do
			if type(name) == "string" then
				local cleaned = trim(name)
				if cleaned ~= "" then
					personByLower[cleaned:lower()] = cleaned
				end
			end
		end
	end

	local matched = {}
	if type(keywords) == "table" then
		for _, keyword in ipairs(keywords) do
			if type(keyword) == "string" then
				local cleaned = trim(keyword)
				if cleaned ~= "" then
					local person = personByLower[cleaned:lower()]
					if person then
						matched[person] = true
					else
						table.insert(plain, cleaned)
					end
				end
			end
		end
	end

	if type(personNames) == "table" then
		for _, name in ipairs(personNames) do
			if type(name) == "string" then
				local cleaned = trim(name)
				if matched[cleaned] then
					matched[cleaned] = nil
					table.insert(faces, cleaned)
				end
			end
		end
	end

	return plain, faces
end

---
-- Extracts standardized EXIF metadata from a photo for use by the backend.
-- Handles robustness for newer RAW formats (like .CR3) where raw metadata might be elusive.
-- @param photo LrPhoto The photo object.
-- @return table Map of EXIF fields (capture_time, focal_length, camera_make, camera_model, iso, aperture, shutter_speed).
--
function Util.getPhotoExif(photo)
	local exif = {}

	-- Focal Length (raw number)
	local fl = safeGetRawMetadata(photo, "focalLength")
	if type(fl) == "number" then
		exif.focal_length = fl
	end

	-- Capture Time (Unix timestamp)
	local dt = safeGetRawMetadata(photo, "dateTime")
	if type(dt) == "number" then
		exif.capture_time = dt
	end

	-- Camera Make & Model
	-- Prefer formatted metadata for camera info as it is often more reliably populated
	-- for modern proprietary RAW formats (CR3, etc.) in the SDK.
	local make = safeGetFormattedMetadata(photo, "cameraMaker") or safeGetRawMetadata(photo, "cameraMaker")
	if type(make) == "string" and make ~= "" then
		exif.camera_make = make
	end

	local model = safeGetFormattedMetadata(photo, "cameraModel") or safeGetRawMetadata(photo, "cameraModel")
	if type(model) == "string" and model ~= "" then
		exif.camera_model = model
	end

	-- ISO
	local iso = safeGetRawMetadata(photo, "isoSpeedRating")
	if type(iso) == "number" then
		exif.iso = iso
	end

	-- Aperture
	local ap = safeGetRawMetadata(photo, "aperture")
	if type(ap) == "number" then
		exif.aperture = ap
	end

	-- Shutter Speed (formatted string e.g. "1/200")
	local ss = safeGetFormattedMetadata(photo, "shutterSpeed")
	if type(ss) == "string" and ss ~= "" then
		exif.shutter_speed = ss
	end

	-- Exposure compensation in EV. This is the only thing that lets culling
	-- tell an exposure bracket apart from a burst, and without it a bracketed
	-- sequence gets one frame nominated as the winner and the rest offered up
	-- as reject candidates. Deliberately left nil rather than defaulted to 0
	-- when the camera did not record it: the backend requires every frame in a
	-- group to carry a value, and a fabricated 0 would read as "all frames shot
	-- at the same compensation", which is how a focus stack is recognised.
	local eb = safeGetRawMetadata(photo, "exposureBias")
	if type(eb) == "number" then
		exif.exposure_bias = eb
	end

	return exif
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
		"computePartialFileMd5: file="
			.. tostring(filePath)
			.. " size="
			.. tostring(attributes.fileSize)
			.. " chunkSize="
			.. tostring(chunkSize)
			.. " firstLen="
			.. tostring(firstLen)
			.. " lastLen="
			.. tostring(string.len(lastChunk))
			.. " elapsedMs="
			.. tostring(elapsedMs)
	)

	return digest,
		{
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
		log:error(
			"getGlobalPhotoIdForPhoto: file attributes unavailable for photo path="
				.. tostring(originalFilePath)
				.. " err="
				.. tostring(attrErr)
		)
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
		if
			cachedAlgorithm == LEGACY_HASH_ALGO
			and cachedSize == tonumber(attributes.fileSize)
			and math.floor(cachedMtime or 0) == math.floor(tonumber(attributes.fileModificationDate) or 0)
		then
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
		log:warn(
			"getGlobalPhotoIdForPhoto: stable metadata id failed, falling back to partial hash for "
				.. tostring(originalFilePath)
				.. " err="
				.. tostring(idErr)
		)
		local fallbackId, metadataOrErr = Util.buildGlobalPhotoId(originalFilePath, options.windowBytes)
		if not fallbackId then
			log:error(
				"getGlobalPhotoIdForPhoto: failed for "
					.. tostring(originalFilePath)
					.. " err="
					.. tostring(metadataOrErr)
			)
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
		photo:setPropertyForPlugin(
			_PLUGIN,
			"globalPhotoIdFileModificationDate",
			tostring(metadata.fileModificationDate or "")
		)
		photo:setPropertyForPlugin(_PLUGIN, "globalPhotoIdAlgorithm", tostring(metadata.algorithm or STABLE_ID_ALGO))
	end)

	local rebuildElapsedMs = math.floor((LrDate.currentTime() - rebuildStartedAt) * 1000)
	log:trace(
		"getGlobalPhotoIdForPhoto: cache miss -> generated id for "
			.. tostring(originalFilePath)
			.. " elapsedMs="
			.. tostring(rebuildElapsedMs)
			.. " idPrefix="
			.. tostring(string.sub(globalPhotoId, 1, 24))
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
		if
			relativePath ~= nil
			and string.len(relativePath) > 0
			and string.len(relativePath) < string.len(absolutePath)
		then
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
	local macPath14 = LrPathUtils.getStandardFilePath("home") .. "/Library/Logs/Adobe/Lightroom/LrClassicLogs/"
	local winPath14 = LrPathUtils.getStandardFilePath("home")
		.. "\\AppData\\Local\\Adobe\\Lightroom\\Logs\\LrClassicLogs\\"
	local macPathOld = LrPathUtils.getStandardFilePath("documents") .. "/LrClassicLogs/"
	local winPathOld = LrPathUtils.getStandardFilePath("documents") .. "\\LrClassicLogs\\"

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

---
-- Formats a timestamp into a filesystem-safe string (YYYY-MM-DD_HH-MM-SS).
-- @param timestamp number Unix timestamp (defaults to current time).
-- @return string Formatted timestamp.
--
function Util.formatTimestampSafe(timestamp)
	timestamp = timestamp or LrDate.currentTime()
	local w3c = LrDate.timeToW3CDate(timestamp)
	-- Convert W3C format (YYYY-MM-DDTHH:MM:SSZ) to filesystem safe (YYYY-MM-DD_HH-MM-SS)
	return w3c:gsub("T", "_"):gsub(":", "-"):sub(1, 19)
end

---
-- Formats a duration as "m:ss" (or "h:mm:ss" past an hour) for progress
-- captions. A ticking clock is what distinguishes a slow operation from a
-- frozen one, so this is used wherever a task can run for minutes.
-- @param seconds number Duration in seconds; nil/negative/NaN are treated as 0.
-- @return string Formatted duration.
--
function Util.formatElapsedTime(seconds)
	local total = tonumber(seconds) or 0
	-- NaN compares false against itself; catch it before math.floor.
	if total ~= total or total < 0 then
		total = 0
	end
	total = math.floor(total)
	local hours = math.floor(total / 3600)
	local minutes = math.floor((total % 3600) / 60)
	local secs = total % 60
	if hours > 0 then
		return string.format("%d:%02d:%02d", hours, minutes, secs)
	end
	return string.format("%d:%02d", minutes, secs)
end

function Util.copyLogfilesToDesktop(extraInfo)
	local progressScope = LrProgressScope({
		title = LOC("$$$/LrGeniusAI/PluginInfo/CopyingLogs=Copying log files to Desktop..."),
		canCancel = true,
	})

	local folderName = "LrGenius_" .. Util.formatTimestampSafe(LrDate.currentTime())
	local folder = LrPathUtils.child(LrPathUtils.getStandardFilePath("desktop"), folderName)
	if LrFileUtils.exists(folder) then
		log:trace("Removing pre-existing report folder: " .. folder)
		LrFileUtils.moveToTrash(folder)
	end

	if progressScope:isCanceled() then
		progressScope:done()
		return
	end
	progressScope:setPortionComplete(0.1, 1)

	log:trace("Creating report folder: " .. folder)
	LrFileUtils.createDirectory(folder)

	if extraInfo then
		local reportPath = LrPathUtils.child(folder, "report.txt")
		local f = io.open(reportPath, "w")
		if f then
			f:write("LrGeniusAI Error Report\n")
			f:write("======================\n\n")
			f:write("Date: " .. Util.formatTimestampSafe(LrDate.currentTime()) .. "\n")
			if extraInfo.error then
				f:write("Error: " .. tostring(extraInfo.error) .. "\n")
			end
			if extraInfo.details then
				f:write("Details: " .. tostring(extraInfo.details) .. "\n")
			end
			f:write("\nSystem Info:\n")
			f:write("Lightroom Version: " .. tostring(LrApplication.versionString()) .. "\n")
			f:write("OS: " .. (MAC_ENV and "macOS" or "Windows") .. "\n")
			f:close()
		end
	end

	if progressScope:isCanceled() then
		progressScope:done()
		return
	end
	progressScope:setPortionComplete(0.3, 1)

	local filePath = LrPathUtils.child(folder, "LrGeniusAI.log")
	local logFilePath = Util.getLogfilePath()
	if LrFileUtils.exists(logFilePath) then
		log:trace("Copying local logfile: " .. logFilePath)
		-- On Windows, LrFileUtils.copy can fail if the file is locked by the logger.
		-- Reading the file content via LrFileUtils.readFile is more robust.
		local content = LrFileUtils.readFile(logFilePath)
		if content then
			local f = io.open(filePath, "wb")
			if f then
				f:write(content)
				f:close()
			else
				-- Fallback to copy if file handle fails
				LrFileUtils.copy(logFilePath, filePath)
			end
		else
			-- Fallback to copy if read fails
			LrFileUtils.copy(logFilePath, filePath)
		end
	else
		log:warn("Logfile not found: " .. tostring(logFilePath))
		-- Don't show error here, just continue as we might have server logs
	end

	if progressScope:isCanceled() then
		progressScope:done()
		return
	end
	progressScope:setPortionComplete(0.5, 1)
	progressScope:setCaption(LOC("$$$/LrGeniusAI/Util/FetchingServerLogs=Fetching server-side logs via API..."))

	-- Use the new streaming method to download logs directly to disk, avoiding memory spikes
	local url = tostring(prefs.backendServerUrl or "")
	local host = (url:match("://([^:/]+)") or url:match("^([^:/]+)") or ""):lower()
	local prefix = ""
	if host ~= "" and host ~= "127.0.0.1" and host ~= "localhost" then
		prefix = host .. "-"
	end

	local logFiles = {
		{ type = "backend", filename = "lrgenius-server.log" },
		{ type = "ollama", filename = "ollama.log" },
		{ type = "lmstudio", filename = "lmstudio.log" },
	}

	log:trace("Fetching server-side logs via streaming API...")
	for i, logInfo in ipairs(logFiles) do
		if progressScope:isCanceled() then
			break
		end

		local targetName = prefix .. logInfo.filename
		local targetPath = LrPathUtils.child(folder, targetName)

		progressScope:setCaption(LOC("$$$/LrGeniusAI/Util/FetchingLog=Fetching ^1...", logInfo.filename))
		local success = SearchIndexAPI.downloadRawLog(logInfo.type, targetPath)

		if success then
			log:trace("Successfully streamed log: " .. logInfo.filename)
		else
			log:trace("Log not available or fetch failed: " .. logInfo.filename)
		end

		progressScope:setPortionComplete(0.5 + (i / #logFiles) * 0.4, 1)
	end

	progressScope:setPortionComplete(1.0, 1)
	progressScope:setCaption(LOC("$$$/LrGeniusAI/common/Done=Done."))
	progressScope:done()

	if LrFileUtils.exists(folder) then
		LrShell.revealInShell(folder)
	else
		ErrorHandler.handleError(LOC("$$$/LrGeniusAI/Util/LogfileCopyFailed=Logfile copy failed"), folder)
	end
end

function Util.getOllamaLogfilePath()
	local macPath = LrPathUtils.getStandardFilePath("home") .. "/.ollama/logs/server.log"
	local winPath = LrPathUtils.getStandardFilePath("home") .. "\\AppData\\Local\\ollama\\server.log"

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
	if o == nil then
		return nil
	end
	if seen[o] then
		return seen[o]
	end

	local no
	if type(o) == "table" then
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

local function cleanStringList(rawList, reservedLowered)
	local cleaned = {}
	local seen = {}
	if reservedLowered then
		for _, lowered in ipairs(reservedLowered) do
			seen[lowered] = true
		end
	end
	if type(rawList) ~= "table" then
		return cleaned
	end
	for _, entry in ipairs(rawList) do
		if type(entry) == "string" then
			local text = Util.trim(entry)
			if text ~= "" then
				local lowered = string.lower(text)
				if not seen[lowered] then
					table.insert(cleaned, text)
					seen[lowered] = true
				end
			end
		end
	end
	return cleaned
end

local function sanitizeKeywordLeaf(value)
	if type(value) == "string" then
		local keyword = Util.trim(value)
		if keyword == "" then
			return nil, {}, {}, {}
		end
		return keyword, {}, {}, {}
	end

	if isKeywordLeafObject(value) then
		local keyword = Util.trim(value.name)
		if keyword == "" then
			return nil, {}, {}, {}
		end

		local nameLower = string.lower(keyword)
		local cleanedSynonyms = cleanStringList(value.synonyms, { nameLower })
		local cleanedAliases = cleanStringList(value.aliases, { nameLower })

		-- synonym_aliases must not collide with the translation names themselves.
		local translationLowers = { nameLower }
		for _, syn in ipairs(cleanedSynonyms) do
			table.insert(translationLowers, string.lower(syn))
		end
		local cleanedSynonymAliases = cleanStringList(value.synonym_aliases, translationLowers)

		return keyword, cleanedSynonyms, cleanedAliases, cleanedSynonymAliases
	end

	return nil, {}, {}, {}
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

	table.sort(stringKeys, function(a, b)
		return a < b
	end)
	table.sort(numberKeys, function(a, b)
		return a < b
	end)

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
-- @return table keywordsVal, table keywordsMeta, table orderedIds
--
function Util.extractAllKeywords(hierarchicalTable)
	if hierarchicalTable == nil or type(hierarchicalTable) ~= "table" then
		return {}, {}, {}
	end

	local result = {}
	local meta = {}
	local orderedIds = {}
	local keywordCounter = 0
	local seenKeywords = {}

	local function recurse(tbl, currentPath)
		iterateDeterministic(tbl, function(key, value)
			local keyIsString = type(key) == "string"

			-- Defensive: ignore string keys that are purely numeric, as they are likely
			-- leaked indices from JSON conversion or previous processing.
			local isNumericKey = keyIsString and (tonumber(key) ~= nil)

			if isKeywordLeafObject(value) or type(value) == "string" then
				-- It's a leaf value (string or leaf object)
				local keywordName, synonyms, aliases, synonymAliases = sanitizeKeywordLeaf(value)
				if keywordName and keywordName ~= "" then
					-- Determine the category path for this keyword
					local finalPath = currentPath
					if keyIsString and not isNumericKey and keywordName ~= key then
						-- If the key is a string and different from the name,
						-- it's likely a parent/category name
						if finalPath == "" then
							finalPath = key
						else
							finalPath = finalPath .. " > " .. key
						end
					end

					-- Deduplication: prevent adding same keyword name under same path
					local dedupeKey = finalPath .. "//" .. keywordName
					if not seenKeywords[dedupeKey] then
						keywordCounter = keywordCounter + 1
						local keywordId = "kw_" .. tostring(keywordCounter)
						result[keywordId] = keywordName
						meta[keywordId] = {
							synonyms = synonyms,
							aliases = aliases,
							synonymAliases = synonymAliases,
							path = finalPath,
						}
						table.insert(orderedIds, keywordId)
						seenKeywords[dedupeKey] = true
					end
				end
			elseif type(value) == "table" then
				-- It's a sub-hierarchy
				local subPath = currentPath
				if keyIsString and not isNumericKey then
					if subPath == "" then
						subPath = key
					else
						subPath = subPath .. " > " .. key
					end
				end
				recurse(value, subPath)
			end
		end)
	end

	recurse(hierarchicalTable, "")

	log:trace("Extracted keywords: " .. Util.dumpTable(result))

	return result, meta, orderedIds
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

	local function buildKeywordLeaf(keywordId, fallbackValue, fallback)
		if not keywordsSel[keywordId] then
			return nil
		end
		local newKeyword = keywordsVal[keywordId] or fallbackValue
		if not newKeyword or Util.trim(newKeyword) == "" then
			return nil
		end
		newKeyword = Util.trim(newKeyword)
		local meta = keywordsMeta and keywordsMeta[keywordId] or nil
		fallback = fallback or {}
		local synonyms = (meta and meta.synonyms) or fallback.synonyms or {}
		local aliases = (meta and meta.aliases) or fallback.aliases or {}
		local synonymAliases = (meta and meta.synonymAliases) or fallback.synonymAliases or {}

		local hasExtra = (synonyms and #synonyms > 0)
			or (aliases and #aliases > 0)
			or (synonymAliases and #synonymAliases > 0)
		if not hasExtra then
			return newKeyword
		end

		local leaf = { name = newKeyword }
		if synonyms and #synonyms > 0 then
			leaf.synonyms = Util.deepcopy(synonyms)
		end
		if aliases and #aliases > 0 then
			leaf.aliases = Util.deepcopy(aliases)
		end
		if synonymAliases and #synonymAliases > 0 then
			leaf.synonym_aliases = Util.deepcopy(synonymAliases)
		end
		return leaf
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
				local leafValue = buildKeywordLeaf(keywordId, value.name, {
					synonyms = value.synonyms or {},
					aliases = value.aliases or {},
					synonymAliases = value.synonym_aliases or {},
				})
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
-- Splits a string into a table based on a delimiter.
-- @param str The string to split.
-- @param delimiter The delimiter string.
-- @return table of parts
--
function Util.split(str, delimiter)
	local result = {}
	local from = 1
	local delim_from, delim_to = string.find(str, delimiter, from, true)
	while delim_from do
		table.insert(result, string.sub(str, from, delim_from - 1))
		from = delim_to + 1
		delim_from, delim_to = string.find(str, delimiter, from, true)
	end
	table.insert(result, string.sub(str, from))
	return result
end

---
-- Builds a hierarchical keyword table from a list of full path strings.
-- Each item in pathsWithMeta should be { path = "A > B > C", synonyms = { ... } }
-- @param pathsWithMeta table list of paths and synonyms.
-- @return hierarchical table
--
function Util.buildHierarchyFromPaths(pathsWithMeta)
	local root = {}
	for _, item in ipairs(pathsWithMeta) do
		local parts = Util.split(item.path, ">")
		if #parts > 0 then
			local current = root
			for i = 1, #parts - 1 do
				local catName = Util.trim(parts[i])
				if catName ~= "" then
					if current[catName] == nil then
						current[catName] = {}
					end
					current = current[catName]
				end
			end

			local leafName = Util.trim(parts[#parts])
			if leafName ~= "" then
				local hasSynonyms = item.synonyms and #item.synonyms > 0
				local hasAliases = item.aliases and #item.aliases > 0
				local hasSynonymAliases = item.synonymAliases and #item.synonymAliases > 0
				local leafNode
				if hasSynonyms or hasAliases or hasSynonymAliases then
					leafNode = { name = leafName }
					if hasSynonyms then
						leafNode.synonyms = item.synonyms
					end
					if hasAliases then
						leafNode.aliases = item.aliases
					end
					if hasSynonymAliases then
						leafNode.synonym_aliases = item.synonymAliases
					end
				else
					leafNode = leafName
				end
				table.insert(current, leafNode)
			end
		end
	end
	return root
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
		for _, v in pairs(tbl) do
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
	local keys = {}
	for key, _ in pairs(t) do
		table.insert(keys, key)
	end
	return keys
end

function Util.waitForServerDialog(options)
	options = options or {}
	if SearchIndexAPI.pingServer() then
		local compatible, versionMessage = SearchIndexAPI.ensureVersionCompatibility()
		if compatible then
			-- Deep health check for soft warnings
			local report = Util.checkPluginHealth(options)
			if not report.healthy then
				return Util.showHealthIssuesDialog(report)
			end
			-- Check for updates via backend if enabled
			if prefs.periodicalUpdateCheck then
				LrTasks.startAsyncTask(function()
					require("UpdateCheck")
					UpdateCheck.checkForNewVersionInBackground()
				end)
			end
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
				SearchIndexAPI.startServer({ readyTimeoutSeconds = 120 })
			end)

			if SearchIndexAPI.pingServer() then
				local compatible2, versionMessage2 = SearchIndexAPI.ensureVersionCompatibility()
				if compatible2 then
					-- Deep health check for soft warnings
					local report = Util.checkPluginHealth(options)
					if not report.healthy then
						return Util.showHealthIssuesDialog(report)
					end
					-- Check for updates via backend if enabled
					if prefs.periodicalUpdateCheck then
						LrTasks.startAsyncTask(function()
							require("UpdateCheck")
							UpdateCheck.checkForNewVersionInBackground()
						end)
					end
					return true
				end
				versionMessage = versionMessage2 or versionMessage
			end
		end

		LrDialogs.message("Plugin/Backend version mismatch", versionMessage or "Version check failed.", "critical")
		return false
	end

	local result = false

	LrFunctionContext.callWithContext("WaitForServerContext", function(waitContext)
		local progressScope = LrDialogs.showModalProgressDialog({
			title = LOC("$$$/lrc-ai-assistant/WaitForServer/title=LrGeniusAI"),
			caption = LOC("$$$/lrc-ai-assistant/WaitForServer/caption=Waiting for LrGeniusAI database to load..."),
			cannotCancel = false,
			functionContext = waitContext,
		})

		local elapsedTime = 0
		local timeout = 120 -- 120 seconds timeout

		while not progressScope:isCanceled() and elapsedTime < timeout do
			if SearchIndexAPI.pingServer() then
				local compatible, versionMessage = SearchIndexAPI.ensureVersionCompatibility()
				progressScope:done()
				if compatible then
					-- Deep health check for soft warnings
					local report = Util.checkPluginHealth(options)
					if not report.healthy then
						result = Util.showHealthIssuesDialog(report)
					else
						-- Check for updates via backend if enabled
						if prefs.periodicalUpdateCheck then
							LrTasks.startAsyncTask(function()
								require("UpdateCheck")
								UpdateCheck.checkForNewVersionInBackground()
							end)
						end
						result = true
					end
					return
				end

				-- If we found a mismatch after the server started (likely a stale backend),
				-- restart it once and re-check.
				if SearchIndexAPI.isBackendOnLocalhost() then
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
						SearchIndexAPI.startServer({ readyTimeoutSeconds = 120 })
					end)

					-- Re-check compatibility immediately (without restarting the modal loop).
					if SearchIndexAPI.pingServer() then
						local compatible2, versionMessage2 = SearchIndexAPI.ensureVersionCompatibility()
						if compatible2 then
							-- Deep health check for soft warnings
							local report = Util.checkPluginHealth(options)
							if not report.healthy then
								result = Util.showHealthIssuesDialog(report)
							else
								-- Check for updates via backend if enabled
								if prefs.periodicalUpdateCheck then
									LrTasks.startAsyncTask(function()
										require("UpdateCheck")
										UpdateCheck.checkForNewVersionInBackground()
									end)
								end
								result = true
							end
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
			progressScope:done()
			-- Diagnose and show detailed error
			local diag = SearchIndexAPI.diagnoseStartupFailure()
			Util.showDiagnosticFailureDialog(diag)
			result = false
		end
	end)

	return result
end

--- Bare check/cross icon reflecting a property, for use next to a status text
-- that says more than the icon can (a tri-state, a reason, a colour).
-- @param f the LrView factory
-- @param key name of the property driving the icon
-- @param isOk optional predicate mapping the property value to a boolean;
--        defaults to the value's own truthiness
function Util.statusIcon(f, key, isOk)
	return f:picture({
		value = LrView.bind({
			key = key,
			transform = function(value)
				local ok
				if isOk then
					ok = isOk(value)
				else
					ok = value
				end
				return _PLUGIN:resourceId(ok and "ok.png" or "nok.png")
			end,
		}),
		width = 16,
		height = 16,
	})
end

--- Read-only status indicator: a check or cross icon plus a label.
-- Used instead of a permanently disabled checkbox, which looks like a control
-- the user could toggle but cannot.
-- @param f the LrView factory
-- @param key name of the boolean property driving the icon
-- @param title the label shown next to the icon
function Util.statusIndicator(f, key, title)
	return f:row({
		spacing = f:label_spacing(),
		Util.statusIcon(f, key),
		f:static_text({ title = title }),
	})
end

function Util.showDiagnosticFailureDialog(diag)
	local f = LrView.osFactory()

	local message =
		LOC("$$$/LrGeniusAI/Health/BackendCritical=The local backend server is not running and could not be started.")
	local hint

	if diag.binaryMissing then
		hint = LOC(
			"$$$/LrGeniusAI/Diagnostics/BinaryMissingHint=Please reinstall the LrGeniusAI plugin or check if your antivirus has quarantined the 'lrgenius-server' file."
		)
	elseif diag.portBusy then
		hint = LOC(
			"$$$/LrGeniusAI/Diagnostics/PortBusyHint=Please close other applications like Ollama or another instance of Lightroom that might be using this port."
		)
	else
		hint = LOC(
			"$$$/LrGeniusAI/Onboarding/BackendHint=If the server fails to start, check if another application is using port 19819 or if your firewall is blocking it."
		)
	end

	-- Built as a flat table and handed to f:column() in one call. Appending to
	-- an already-constructed column does nothing, which is why the log snippet
	-- below never reached the user (issue #313).
	local rows = {
		f:static_text({
			title = message,
			font = "<system/bold>",
			text_color = LrColor(1, 0, 0),
		}),
		f:static_text({
			title = LOC("$$$/LrGeniusAI/Health/FixIt=How to fix it:"),
			font = "<system/bold>",
		}),
		f:static_text({
			title = hint,
			width_in_chars = 60,
		}),
	}

	if diag.logSnippet then
		rows[#rows + 1] = f:spacer({ height = 10 })
		rows[#rows + 1] = f:static_text({ title = LOC("$$$/LrGeniusAI/Diagnostics/LogSnippet=Recent server errors:") })
		rows[#rows + 1] = f:edit_field({
			value = diag.logSnippet,
			width_in_chars = 60,
			height_in_lines = 10,
			enabled = false,
		})
	end

	local contents = f:column({
		spacing = f:control_spacing(),
		unpack(rows),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/Health/DialogTitle=LrGeniusAI System Check"),
		contents = contents,
		actionVerb = LOC("$$$/LrGeniusAI/Health/OpenWizard=Run Setup Wizard"),
		cancelVerb = LOC("$$$/LrGeniusAI/common/Close=Close"),
	})

	if result == "ok" then
		OnboardingWizard.show(true)
	end
end

function Util.checkPluginHealth(options)
	options = options or {}
	local health = SearchIndexAPI.getDetailedHealth()
	local report = {
		healthy = true,
		critical = false,
		issues = {},
		diagnostics = nil,
	}

	if not health.backend then
		report.healthy = false
		report.critical = true
		report.diagnostics = SearchIndexAPI.diagnoseStartupFailure()
		table.insert(report.issues, {
			title = LOC("$$$/LrGeniusAI/Health/BackendFailed=Backend server is not reachable."),
			hint = LOC(
				"$$$/LrGeniusAI/Health/BackendCritical=The local backend server is not running and could not be started."
			),
			critical = true,
		})
	end

	if not health.clip and (options.requireClip or prefs.useClip) then
		report.healthy = false
		table.insert(report.issues, {
			title = LOC("$$$/LrGeniusAI/Health/ClipMissing=AI search model is missing."),
			hint = LOC(
				"$$$/LrGeniusAI/Health/ClipMissingHint=Smart photo search will be disabled. You can download the model in the Setup Wizard."
			),
			critical = false,
		})
	end

	if not SearchIndexAPI.hasAnyLlmProvider(health) then
		report.healthy = false
		table.insert(report.issues, {
			title = LOC("$$$/LrGeniusAI/Health/ApiKeysMissing=No AI providers configured for AI generation."),
			hint = LOC(
				"$$$/LrGeniusAI/Health/ApiKeysMissingHint=You need to configure Gemini or ChatGPT API keys (or a local provider) to generate keywords and descriptions."
			),
			critical = options.requireProviders == true,
		})
		if options.requireProviders then
			report.critical = true
		end
	end

	return report
end

function Util.showHealthIssuesDialog(report)
	local f = LrView.osFactory()

	local issues = report.issues or {}

	-- A dialog that says "we found some issues" and then lists none tells the
	-- user nothing they can act on, which is exactly the failure the "errors
	-- must surface" rule exists to prevent. If the report ever arrives without
	-- its reasons, say so and point at the log rather than showing a blank box.
	if #issues == 0 then
		issues = {
			{
				title = "The system check reported a problem but could not identify it.",
				hint = "Please open Plug-in Manager > LrGeniusAI > Show logfile and include the log if you report this.",
				critical = false,
			},
		}
	end

	-- Children must be passed to f:column() in one go: the view is built from
	-- this table at construction time, so rows appended to the finished column
	-- afterwards are silently dropped and never rendered (issue #313).
	local rows = {
		f:static_text({
			title = LOC(
				"$$$/LrGeniusAI/Health/IssuesFound=We found some issues that might prevent the plugin from working correctly:"
			),
			font = "<system/bold>",
		}),
	}

	for _, issue in ipairs(issues) do
		rows[#rows + 1] = f:row({
			f:static_text({
				title = "• " .. issue.title,
				text_color = issue.critical and LrColor(1, 0, 0) or LrColor(0.8, 0.8, 0),
				font = issue.critical and "<system/bold>" or "<system>",
			}),
		})
		rows[#rows + 1] = f:row({
			f:spacer({ width = 20 }),
			f:static_text({
				title = issue.hint,
				width_in_chars = 60,
				size = "small",
			}),
		})
	end

	local contents = f:column({
		spacing = f:control_spacing(),
		unpack(rows),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/Health/DialogTitle=LrGeniusAI System Check"),
		contents = contents,
		actionVerb = report.critical and LOC("$$$/LrGeniusAI/Health/OpenWizard=Run Setup Wizard")
			or LOC("$$$/LrGeniusAI/Health/IgnoreAndContinue=Ignore and Continue"),
		cancelVerb = LOC("$$$/LrGeniusAI/common/Cancel=Cancel"),
		otherVerb = not report.critical and LOC("$$$/LrGeniusAI/Health/OpenWizard=Run Setup Wizard") or nil,
	})

	if result == "ok" then
		if report.critical then
			OnboardingWizard.show(true)
			return false
		else
			return true -- Ignored and continue
		end
	elseif result == "other" then
		OnboardingWizard.show(true)
		return false
	end

	return false
end

---
-- Adds a photo to the "Rejected AI Descriptions" collection (under set "LrGeniusAI").
-- Finds or creates the set and collection by name, then adds the photo.
-- @param photo LrPhoto
-- @param writeOptions optional; e.g. Defaults.catalogWriteAccessOptions
--
function Util.addPhotoToRejectedDescriptionsCollection(photo, writeOptions)
	if not photo then
		return
	end
	writeOptions = writeOptions or { timeout = 60 }
	local catalog = LrApplication.activeCatalog()
	local setName = LOC("$$$/LrGeniusAI/Rejected/CollectionSetName=LrGeniusAI")
	local collName = LOC("$$$/LrGeniusAI/Rejected/CollectionName=Rejected AI Descriptions")

	-- Creating a collection (or collection set) and then immediately reading/using
	-- it in the same withWriteAccessDo call can fail with a "can't get collection
	-- info" error because the object isn't fully committed until the write-access
	-- block exits. Use separate blocks, matching the pattern used elsewhere
	-- (TaskFindSimilarFaces.lua, TaskFindSimilarImages.lua, TaskPeople.lua, TaskSemanticSearch.lua).
	local collectionSet
	catalog:withWriteAccessDo(
		LOC("$$$/LrGeniusAI/Rejected/CreateCollectionSet=Create LrGeniusAI Collection Set"),
		function()
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
		end,
		writeOptions
	)
	if not collectionSet then
		log:error("Util.addPhotoToRejectedDescriptionsCollection: could not create/find collection set")
		return
	end

	local collection
	catalog:withWriteAccessDo(
		LOC("$$$/LrGeniusAI/Rejected/CreateCollection=Create Rejected AI Descriptions Collection"),
		function()
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
		end,
		writeOptions
	)
	if not collection then
		log:error("Util.addPhotoToRejectedDescriptionsCollection: could not create/find collection")
		return
	end

	catalog:withWriteAccessDo(LOC("$$$/LrGeniusAI/Rejected/AddToCollection=Add to Rejected AI Descriptions"), function()
		collection:addPhotos({ photo })
	end, writeOptions)
end

-- Photos per request when llama.cpp is selected and the user has not said
-- otherwise. Kept modest because the whole group has to fit the context
-- window alongside the pinned prefix; the server clamps it further to the
-- engine's own n_parallel.
local DEFAULT_GROUPED_BATCH_SIZE = 4

-- Photos per request when the run never calls an LLM (embeddings, faces,
-- species, cull). Nothing has to fit a context window here, so the ceiling is
-- memory: the server reads the originals one at a time but keeps every
-- normalised JPEG until the batch finishes, which is a few hundred KB each.
local DEFAULT_NON_LLM_BATCH_SIZE = 16

---
-- An unusable override (absent, unparseable, zero, negative, NaN) means the
-- default, matching how the server treats a bad `llm_batch_size`. Zero in
-- particular must not become a literal batch of no photos.
--
local function clampBatch(configured, default)
	local n = tonumber(configured)
	if n == nil or n ~= n or n < 1 then -- n ~= n catches NaN
		return default
	end
	return math.floor(n)
end

---
-- How many photos to send per indexing request.
--
-- Two different reasons to group, with different limits:
--
-- * A run without the `metadata` task never talks to a model that cares how
--   many photos arrive together, so it groups regardless of provider. The win
--   is one HTTP round trip instead of N, plus the server's parallel passes
--   (file read, decode/resize, culling metrics and pHash) finally having a
--   batch to spread across cores.
-- * A run that does generate metadata only benefits on the in-process
--   llama.cpp backend, which shares one pinned prompt prefix across the group
--   and decodes them in parallel sequences. Every remote provider is billed
--   and rate-limited per request and gains nothing, so they stay at one photo
--   per request. MLX is local but decodes one photo at a time against a fresh
--   KV cache, so grouping there would only delay the first result without
--   producing any of them sooner.
--
-- Either way grouping requires sending originals by reference — the export
-- path hands the server one temp JPEG at a time.
--
-- This is deliberately a pure function so it can be tested without Lightroom.
-- Note it does not touch maxWorkers: batching is server-side, and the plugin
-- keeps a single worker.
--
-- @param provider string|nil Selected provider name.
-- @param useOriginals boolean Whether originals are submitted by reference.
-- @param isLocalBackend boolean Whether the backend can read local files.
-- @param configured number|string|nil Optional user override.
-- @param llmActive boolean|nil False when the run generates no AI metadata.
--   Absent means "assume it does", which is the behaviour every caller had
--   before the non-LLM path existed.
-- @return number Photos per request; always >= 1.
--
function Util.groupedBatchSize(provider, useOriginals, isLocalBackend, configured, llmActive)
	if not useOriginals or not isLocalBackend then
		return 1
	end
	-- No model in the loop, so no provider policy applies.
	if llmActive == false then
		return clampBatch(configured, DEFAULT_NON_LLM_BATCH_SIZE)
	end
	if type(provider) ~= "string" or provider:lower() ~= "llamacpp" then
		return 1
	end
	return clampBatch(configured, DEFAULT_GROUPED_BATCH_SIZE)
end

---
-- Indexes a batch response's `results` array by photo_id.
--
-- A grouped request can partly succeed, and the aggregate counts do not say
-- which photo failed. The caller needs that to retry only the failures and to
-- fire its per-photo callback only for the ones that landed.
--
-- Each entry's own `warnings` are carried through as well. The response also
-- has a request-level `warnings` list, but that one cannot say which photo
-- lost a signal — and a caller that reports "some photo indexed without face
-- detection" for a group of sixteen has not reported anything actionable.
--
-- @param results table|nil Array of { photo_id, success, error, warnings }.
-- @return table Map of photo_id ->
--   { success = boolean, error = string|nil, warnings = table|nil }.
--
function Util.resultsByPhotoId(results)
	local byId = {}
	if type(results) ~= "table" then
		return byId
	end
	for _, entry in ipairs(results) do
		if type(entry) == "table" and type(entry.photo_id) == "string" then
			byId[entry.photo_id] = {
				success = entry.success == true,
				error = entry.error,
				warnings = type(entry.warnings) == "table" and entry.warnings or nil,
			}
		end
	end
	return byId
end

---
-- True when a run's task list reaches an LLM.
--
-- `metadata` is the only task that does — embeddings, faces, species and cull
-- all run against local ONNX models. Callers use this to decide whether the
-- LLM's batching rules apply or whether the run is free to group as large as
-- memory allows.
--
-- An absent or malformed list means "assume it does": guessing wrong in that
-- direction only costs a smaller batch, while the opposite would send a cloud
-- provider sixteen photos in one request.
--
-- @param tasks table|nil Array of task names.
-- @return boolean
--
function Util.tasksCallLlm(tasks)
	if type(tasks) ~= "table" then
		return true
	end
	for _, task in ipairs(tasks) do
		if task == "metadata" then
			return true
		end
	end
	return false
end

---
-- Flattens a keyword-category structure into a plain list of names.
--
-- Accepts both shapes the plugin sends as `keyword_categories`: the flat array
-- from KeywordConfigProvider, and the nested tree from
-- MetadataManager.getCatalogKeywordHierarchy() (parent name -> child table).
-- Every node name is returned, at every depth, because the backend derives its
-- category labels from that tree and a name at any level can end up as one.
--
-- @param categories table|nil Flat array or nested table of category names.
-- @return table Array of names, in no particular order, with duplicates kept.
--
function Util.flattenKeywordCategoryNames(categories)
	local names = {}
	if type(categories) ~= "table" then
		return names
	end
	local seenTables = {}
	local function recurse(node)
		if seenTables[node] then -- a malformed/cyclic table must not hang the task
			return
		end
		seenTables[node] = true
		for key, value in pairs(node) do
			if type(key) == "string" then
				table.insert(names, key)
			end
			if type(value) == "string" then
				table.insert(names, value)
			elseif type(value) == "table" then
				recurse(value)
			end
		end
	end
	recurse(categories)
	return names
end

---
-- Picks the catalog keyword vocabulary to send to the LLM.
--
-- Selection is by usage: the keywords applied to the most photos are the ones
-- the model should reuse, and a big catalog has far more keywords than fit in a
-- prompt. Category names are pinned — they are the labels the model must sort
-- into, so they survive the cap regardless of how often they are used, and they
-- are merged into this one list instead of being repeated as separate
-- vocabulary entries.
--
-- The selected names come back sorted alphabetically, not by count. This block
-- is run-constant context in the backend prompt, so a stable order keeps the
-- cacheable prefix identical between runs when the selected *set* has not
-- changed — which is the common case, since ordinary tagging shifts counts
-- without changing which keywords are the popular ones.
--
-- @param entries table Array of { name = string, count = number }.
-- @param limit number|nil Maximum names to return (default 500).
-- @param options table|nil { pinned = array of names always included }.
-- @return table Array of keyword names, alphabetically sorted.
--
function Util.rankKeywordsByUsage(entries, limit, options)
	limit = tonumber(limit) or 500
	options = options or {}

	local selected = {} -- lowercased name -> display name
	local selectedCount = 0

	local function claim(name)
		if type(name) ~= "string" then
			return false
		end
		local trimmed = name:gsub("^%s*(.-)%s*$", "%1")
		if trimmed == "" then
			return false
		end
		local key = trimmed:lower()
		if selected[key] then
			return false
		end
		selected[key] = trimmed
		selectedCount = selectedCount + 1
		return true
	end

	-- Pinned names first: they take their slots before the cap applies.
	for _, name in ipairs(options.pinned or {}) do
		if selectedCount >= limit then
			break
		end
		claim(name)
	end

	-- Fold duplicate names (the same word under two parents is one term to the
	-- model) by summing their photo counts.
	local totals, order = {}, {}
	for _, entry in ipairs(entries or {}) do
		if type(entry) == "table" and type(entry.name) == "string" then
			local trimmed = entry.name:gsub("^%s*(.-)%s*$", "%1")
			local key = trimmed:lower()
			if trimmed ~= "" and not selected[key] then
				if not totals[key] then
					totals[key] = { name = trimmed, count = 0 }
					table.insert(order, key)
				end
				totals[key].count = totals[key].count + (tonumber(entry.count) or 0)
			end
		end
	end

	local ranked = {}
	for _, key in ipairs(order) do
		table.insert(ranked, totals[key])
	end
	table.sort(ranked, function(a, b)
		if a.count ~= b.count then
			return a.count > b.count
		end
		return a.name < b.name -- ties resolved by name so the result is deterministic
	end)

	for _, entry in ipairs(ranked) do
		if selectedCount >= limit then
			break
		end
		claim(entry.name)
	end

	local result = {}
	for _, name in pairs(selected) do
		table.insert(result, name)
	end
	table.sort(result)
	return result
end

---
-- The on-device model families an indexing run needs, given its `tasks` array.
--
-- Only the models the backend loads itself are listed. `metadata` runs on an
-- LLM and `vertexai` in the cloud, so neither has an entry — a run of those
-- alone needs nothing downloaded.
--
-- `cull` maps to the face model because the fast cull ingest still scores face
-- quality (the backend's `compute_faces = has_task("faces") || cull_pass`);
-- it deliberately skips the embedding, so it does not need `clip`.
--
-- @param tasks table Array of task names as sent to /v1/index/photos.
-- @return table Array of family ids, in the order /v1/models/assets reports them.
--
function Util.requiredModelFamilies(tasks)
	if type(tasks) ~= "table" then
		return {}
	end
	local needed = {}
	for _, task in ipairs(tasks) do
		if task == "embeddings" then
			needed.clip = true
		elseif task == "faces" or task == "cull" then
			needed.face = true
		elseif task == "species" then
			needed.bioclip = true
		end
	end
	-- Fixed order rather than pairs(): the result reaches a dialog, and a list
	-- that reshuffles between runs reads like a different problem each time.
	local result = {}
	for _, id in ipairs({ "clip", "face", "bioclip" }) do
		if needed[id] then
			table.insert(result, id)
		end
	end
	return result
end

---
-- Which of the families a run needs are not downloaded yet.
--
-- Takes the `families` array from /v1/models/assets. A family the run does not
-- need is ignored however unready it is, and a required family the backend
-- does not report at all is treated as present: an older backend that has
-- never heard of it must not block a run it can perform perfectly well.
--
-- @param families table `families` from /v1/models/assets.
-- @param requiredIds table Array of family ids from Util.requiredModelFamilies.
-- @return table Array of { id, name, approx_bytes } per missing family, in required order.
--
function Util.missingModelFamilies(families, requiredIds)
	if type(families) ~= "table" or type(requiredIds) ~= "table" then
		return {}
	end
	local byId = {}
	for _, family in ipairs(families) do
		if type(family) == "table" and family.id then
			byId[family.id] = family
		end
	end
	local missing = {}
	for _, id in ipairs(requiredIds) do
		local family = byId[id]
		if family ~= nil and not family.ready then
			table.insert(missing, {
				id = id,
				name = family.name or id,
				approx_bytes = tonumber(family.approx_bytes) or 0,
			})
		end
	end
	return missing
end

---
-- How much of the catalog a text search can actually reach.
--
-- A semantic search only sees photos that have a SigLIP embedding, so a
-- half-indexed catalog answers "nothing found" for a subject that is sitting
-- right there. The search dialog says so up front instead of letting an empty
-- result read as an empty catalog.
--
-- The catalog's own count is the denominator when it is known: the backend's
-- `total` counts only photos it has already seen, which by definition cannot
-- reveal the ones that were never indexed at all. Falls back to the backend's
-- figure when the catalog count is unavailable.
--
-- @param stats table|nil The /v1/db/stats response.
-- @param catalogPhotoCount number|nil Photos in the Lightroom catalog.
-- @return table|nil { total, searchable, unsearchable }, or nil when unknown.
--
function Util.searchCoverage(stats, catalogPhotoCount)
	local photos = type(stats) == "table" and stats.photos or nil
	if type(photos) ~= "table" then
		return nil
	end
	local withEmbedding = tonumber(photos.with_embedding)
	local backendTotal = tonumber(photos.total)
	if withEmbedding == nil then
		return nil
	end
	local total = tonumber(catalogPhotoCount) or backendTotal
	if total == nil then
		return nil
	end
	-- Clamped rather than trusted: rows survive the photos they describe (the
	-- backend never physically deletes), so a catalog that has shrunk can
	-- report more embeddings than photos, and a negative "missing" count would
	-- be worse than no warning at all.
	local searchable = math.min(withEmbedding, total)
	if searchable < 0 then
		searchable = 0
	end
	return {
		total = total,
		searchable = searchable,
		unsearchable = total - searchable,
	}
end

---
-- A download size a person can judge a "download now?" prompt against.
--
-- Picks the unit rather than always reporting GB: the face model is ~0.09 GB,
-- which reads as "nothing is there" next to a real figure of 94 MB.
--
-- @param bytes number Size in bytes.
-- @return string e.g. "94 MB" or "2.3 GB".
--
function Util.formatDownloadSize(bytes)
	local n = tonumber(bytes) or 0
	if n < 0 then
		n = 0
	end
	if n >= 1e9 then
		return string.format("%.1f GB", n / 1e9)
	end
	return string.format("%.0f MB", n / 1e6)
end

return Util
