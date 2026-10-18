-- AgentCommands.lua
-- What the plugin does for each command the agent bridge hands it (see
-- AgentBridge.lua for the transport, and lrg-mcp in the backend for the tools
-- that send them). One handler per action. A handler takes the command's
-- arguments and an `env`, and returns a result table and, optionally, a list
-- of warnings; it raises an error with a message written for the agent (and,
-- through it, the user) when it cannot do the job.
--
-- Photos travel as backend photo IDs (the same IDs search results carry), and
-- are matched to catalog photos here.
--
-- Changes always run in one catalog:withWriteAccessDo per command, named
-- after what the agent did, so Edit > Undo takes back exactly one agent call.

AgentCommands = {}

local UNDO_NAMES = {
	update = "LrGeniusAI agent: update photos",
	show = "LrGeniusAI agent: show photos",
	collection = "LrGeniusAI agent: collection",
	develop = "LrGeniusAI agent: develop settings",
}

-- Commands that change photos. The user can make the bridge read-only, and
-- then these are refused. `show_photos` is deliberately not here: it only
-- fills the agent's own collection, which is how a read-only agent shows what
-- it found.
local CHANGES = {
	update_photos = true,
	collection = true,
	apply_develop_settings = true,
}

-- Commands that run code from the agent. Only offered when the plugin runs
-- from its .lrdevplugin folder with the developer switch on.
local DEVELOPER = {
	eval_lua = true,
	reload_modules = true,
}

-- How long a write waits for another task's catalog lock before giving up.
local WRITE_TIMEOUT = { timeout = 30 }

local AGENT_COLLECTION_SET = "LrGeniusAI Agent"

---------------------------------------------------------------------------
-- Pure helpers (unit tested in plugin/spec/agent_commands_spec.lua)
---------------------------------------------------------------------------

local LABEL_NUMBERS = { red = 1, yellow = 2, green = 3, blue = 4, purple = 5 }
local FLAG_VALUES = { pick = 1, reject = -1, none = 0, unflagged = 0 }
local FILE_FORMATS = {
	raw = "RAW",
	dng = "DNG",
	jpeg = "JPG",
	jpg = "JPG",
	tiff = "TIFF",
	tif = "TIFF",
	png = "PNG",
	psd = "PSD",
	psb = "PSB",
	avif = "AVIF",
	jxl = "JXL",
	video = "VIDEO",
}

--- IPTC field names as the agent sends them -> setRawMetadata keys.
AgentCommands.METADATA_KEYS = {
	headline = "headline",
	creator = "creator",
	copyright = "copyright",
	location = "location",
	city = "city",
	state = "stateProvince",
	country = "country",
	iso_country_code = "isoCountryCode",
	instructions = "instructions",
	job_identifier = "jobIdentifier",
	alt_text = "altTextAccessibility",
	extended_description = "extDescrAccessibility",
}

--- Splits "Parent > Child > Leaf" into its parts, trimmed, empties dropped.
function AgentCommands.splitPath(path)
	local parts = {}
	for part in tostring(path or ""):gmatch("[^>]+") do
		local trimmed = part:gsub("^%s+", ""):gsub("%s+$", "")
		if trimmed ~= "" then
			parts[#parts + 1] = trimmed
		end
	end
	return parts
end

--- Turns the agent's find_photos criteria into a catalog:findPhotos search
--- descriptor. Capture dates are not part of it: they are filtered in Lua
--- afterwards (see dateRange), which is exact where the smart-collection date
--- operators are not. Returns nil when no criterion maps to findPhotos, which
--- means "start from every photo".
function AgentCommands.buildSearchDesc(criteria)
	criteria = criteria or {}
	local entries = {}
	local function add(entry)
		entries[#entries + 1] = entry
	end
	if criteria.rating_min ~= nil then
		add({ criteria = "rating", operation = ">=", value = tonumber(criteria.rating_min) })
	end
	if criteria.rating_max ~= nil then
		add({ criteria = "rating", operation = "<=", value = tonumber(criteria.rating_max) })
	end
	if criteria.flag ~= nil then
		local value = FLAG_VALUES[tostring(criteria.flag):lower()]
		if value == nil then
			error('flag must be "pick", "reject" or "unflagged".')
		end
		add({ criteria = "pick", operation = "==", value = value })
	end
	if criteria.color_label ~= nil then
		local label = tostring(criteria.color_label):lower()
		local value = LABEL_NUMBERS[label] or (label == "none" and "none" or nil)
		if value == nil then
			error("color_label must be red, yellow, green, blue, purple or none.")
		end
		add({ criteria = "labelColor", operation = "==", value = value })
	end
	if criteria.text then
		add({ criteria = "all", operation = "all", value = criteria.text })
	end
	if criteria.folder then
		add({ criteria = "folder", operation = "all", value = criteria.folder })
	end
	if criteria.collection then
		add({ criteria = "collection", operation = "all", value = criteria.collection })
	end
	if criteria.camera then
		add({ criteria = "camera", operation = "all", value = criteria.camera })
	end
	if criteria.lens then
		add({ criteria = "lens", operation = "all", value = criteria.lens })
	end
	if criteria.file_type then
		local format = FILE_FORMATS[tostring(criteria.file_type):lower()]
		if not format then
			error("file_type must be raw, dng, jpeg, tiff, png, psd, psb, avif, jxl or video.")
		end
		add({ criteria = "fileFormat", operation = "==", value = format })
	end
	if criteria.edited ~= nil then
		add({ criteria = "hasAdjustments", operation = criteria.edited and "isTrue" or "isFalse", value = true })
	end
	if type(criteria.keywords) == "table" and #criteria.keywords > 0 then
		local any = { combine = "union" }
		for _, keyword in ipairs(criteria.keywords) do
			any[#any + 1] = { criteria = "keywords", operation = "words", value = keyword }
		end
		add(#any == 1 and any[1] or any)
	end

	if #entries == 0 then
		return nil
	elseif #entries == 1 then
		return entries[1]
	end
	entries.combine = "intersect"
	return entries
end

--- "YYYY-MM-DD" -> year, month, day (numbers), or nil.
function AgentCommands.parseDay(text)
	local y, m, d = tostring(text or ""):match("^%s*(%d%d%d%d)-(%d%d)-(%d%d)%s*$")
	if not y then
		return nil
	end
	return tonumber(y), tonumber(m), tonumber(d)
end

--- New value for one develop setting: `relative` adds to the current value.
--- Returns nil (and a reason) when the change cannot apply.
function AgentCommands.developValue(current, requested, relative)
	if not relative then
		return requested
	end
	if type(requested) ~= "number" then
		return nil, "relative changes need a number"
	end
	if type(current) ~= "number" then
		return nil, "the photo has no number for this setting to add to"
	end
	return current + requested
end

--- Makes any Lua value safe to send as JSON: Lightroom objects become their
--- description, cycles and depth are cut, and big tables are capped.
function AgentCommands.toJsonSafe(value, depth, seen)
	depth = depth or 0
	seen = seen or {}
	local t = type(value)
	if t == "nil" or t == "boolean" or t == "string" then
		return value
	elseif t == "number" then
		-- NaN and infinity have no JSON form.
		if value ~= value or value == math.huge or value == -math.huge then
			return tostring(value)
		end
		return value
	elseif t == "table" then
		if seen[value] then
			return "<cycle>"
		end
		if depth >= 6 then
			return "<table>"
		end
		seen[value] = true
		local out = {}
		local count = 0
		for k, v in pairs(value) do
			count = count + 1
			if count > 500 then
				out["..."] = "more entries cut"
				break
			end
			local key = (type(k) == "number" or type(k) == "string") and k or tostring(k)
			out[key] = AgentCommands.toJsonSafe(v, depth + 1, seen)
		end
		seen[value] = nil
		return out
	end
	-- userdata (Lightroom objects), functions, threads
	local ok, described = pcall(tostring, value)
	return ok and described or ("<" .. t .. ">")
end

--- A one-line caption for the progress bar, saying what the agent asked for.
function AgentCommands.describe(command)
	local action = command and command.action or "?"
	local args = command and command.args or {}
	local count = type(args.photo_ids) == "table" and #args.photo_ids or nil
	local photos = count and string.format(" (%d photo%s)", count, count == 1 and "" or "s") or ""
	local names = {
		status = "Agent: reading Lightroom's status",
		get_photos = "Agent: reading photos",
		find_photos = "Agent: searching the catalog",
		photo_info = "Agent: reading photo details" .. photos,
		show_photos = "Agent: showing photos" .. photos,
		update_photos = "Agent: updating photos" .. photos,
		collection = "Agent: changing a collection" .. photos,
		list_collections = "Agent: reading collections",
		get_develop_settings = "Agent: reading develop settings",
		apply_develop_settings = "Agent: applying develop settings" .. photos,
		render_preview = "Agent: rendering a preview",
		analyze_photos = "Agent: indexing photos" .. photos,
		run_tests = "Agent: running the plugin's tests",
		read_log = "Agent: reading the log",
		eval_lua = "Agent: running developer code",
		reload_modules = "Agent: reloading plugin modules",
	}
	return names[action] or ("Agent: " .. tostring(action))
end

---------------------------------------------------------------------------
-- Photo identity
---------------------------------------------------------------------------

local function catalog()
	return LrApplication.activeCatalog()
end

--- A fresh per-session lookup cache for AgentCommands.resolvePhotos.
function AgentCommands.newCache()
	return { byId = {}, builtAt = nil }
end

--- The backend photo ID of a catalog photo, remembering the pair so the
--- reverse lookup needs no catalog scan.
local function photoIdOf(env, photo)
	local id, err = SearchIndexAPI.getPhotoIdForPhoto(photo)
	if id and env and env.cache then
		env.cache.byId[id] = photo
	end
	return id, err
end

--- Backend photo IDs -> catalog photos (in the given order) plus the IDs that
--- are not in this catalog. A miss rebuilds the lookup from the whole catalog,
--- at most every 30 seconds — the first lookup of a session pays for it once.
function AgentCommands.resolvePhotos(env, photoIds)
	local photos, missing = {}, {}
	if type(photoIds) ~= "table" or #photoIds == 0 then
		return photos, missing
	end

	-- Without global IDs the photo ID is Lightroom's own UUID.
	if prefs.useGlobalPhotoId == false then
		local cat = catalog()
		for _, id in ipairs(photoIds) do
			local photo = cat:findPhotoByUuid(id)
			if photo then
				photos[#photos + 1] = photo
			else
				missing[#missing + 1] = id
			end
		end
		return photos, missing
	end

	local cache = env.cache
	local unknown = false
	for _, id in ipairs(photoIds) do
		if not cache.byId[id] then
			unknown = true
			break
		end
	end
	if unknown and (cache.builtAt == nil or LrDate.currentTime() - cache.builtAt > 30) then
		local cat = catalog()
		local all = cat:getAllPhotos()
		local progress = env.progress
			and function(done, total)
				env.progress(done, total, "Matching photos to the catalog")
			end
		-- The cached identities first, without touching files; only photos
		-- that have none yet pay for the full computation.
		local byId, uncached = Util.indexCachedPhotoIds(cat, all, progress)
		for id, photo in pairs(byId) do
			cache.byId[id] = photo
		end
		for index, photo in ipairs(uncached) do
			photoIdOf(env, photo)
			if index % 200 == 0 then
				LrTasks.yield()
			end
		end
		cache.builtAt = LrDate.currentTime()
	end
	for _, id in ipairs(photoIds) do
		local photo = cache.byId[id]
		if photo then
			photos[#photos + 1] = photo
		else
			missing[#missing + 1] = id
		end
	end
	return photos, missing
end

--- The warning for IDs that are not in the open catalog, capped.
local function missingWarning(missing)
	if #missing == 0 then
		return nil
	end
	local shown = {}
	for i = 1, math.min(#missing, 5) do
		shown[#shown + 1] = missing[i]
	end
	local more = #missing > 5 and string.format(" and %d more", #missing - 5) or ""
	return string.format(
		"%d photo(s) are not in the open catalog and were skipped: %s%s",
		#missing,
		table.concat(shown, ", "),
		more
	)
end

--- Photos -> backend IDs, with a count of photos that have none.
local function idsOf(env, photos, limit)
	local ids, unidentified = {}, 0
	for index, photo in ipairs(photos) do
		if limit and #ids >= limit then
			break
		end
		local id = photoIdOf(env, photo)
		if id then
			ids[#ids + 1] = id
		else
			unidentified = unidentified + 1
		end
		if index % 200 == 0 then
			LrTasks.yield()
		end
	end
	return ids, unidentified
end

--- Leaves out photos Lightroom has locked for a background AI operation
--- (LrC 15.3+); older Lightroom versions have no such lock.
local function editable(photos)
	local cat = catalog()
	-- Fails on Lightroom versions before 15.3, which have no such lock.
	local ok, available, locked = LrTasks.pcall(function()
		return cat:getPhotosAvailableForEditing(photos)
	end)
	if not ok or type(available) ~= "table" then
		return photos, {}
	end
	return available, locked or {}
end

local function writeAccess(name, fn)
	local cat = catalog()
	local done = false
	cat:withWriteAccessDo(name, function()
		fn(cat)
		done = true
	end, WRITE_TIMEOUT)
	if not done then
		error("Lightroom is busy writing to the catalog (another task holds it). Try again when it finishes.")
	end
end

---------------------------------------------------------------------------
-- Selection, view and catalog queries
---------------------------------------------------------------------------

local function selectedPhotos()
	local cat = catalog()
	-- getTargetPhotos() falls back to the whole filmstrip when nothing is
	-- selected; only a target photo proves there is a selection.
	if not cat:getTargetPhoto() then
		return {}
	end
	return cat:getTargetPhotos()
end

local function describeSource(cat, source)
	if type(source) == "string" then
		local names = {
			[cat.kAllPhotos] = "All Photographs",
			[cat.kPreviousImport] = "Previous Import",
			[cat.kQuickCollectionIdentifier] = "Quick Collection",
			[cat.kLastCatalogExport] = "Last Catalog Export",
		}
		return { kind = "special", name = names[source] or source }
	end
	local ok, info = LrTasks.pcall(function()
		local kind = source:type()
		if kind == "LrFolder" then
			return { kind = "folder", name = source:getName(), path = source:getPath() }
		elseif kind == "LrCollection" then
			return {
				kind = source:isSmartCollection() and "smart_collection" or "collection",
				name = source:getName(),
			}
		end
		return { kind = kind, name = source.getName and source:getName() or kind }
	end)
	return ok and info or { kind = "unknown" }
end

--- Every photo in the folders/collections Lightroom is showing. Ignores the
--- Library filter bar, which the SDK cannot read reliably.
local function viewPhotos()
	local cat = catalog()
	local seen, photos = {}, {}
	local function addAll(list)
		for _, photo in ipairs(list or {}) do
			local key = photo.localIdentifier
			if not seen[key] then
				seen[key] = true
				photos[#photos + 1] = photo
			end
		end
	end
	for _, source in ipairs(cat:getActiveSources() or {}) do
		if source == cat.kAllPhotos then
			addAll(cat:getAllPhotos())
		elseif type(source) ~= "string" then
			local ok, list = LrTasks.pcall(function()
				if source:type() == "LrFolder" then
					return source:getPhotos(true)
				end
				return source:getPhotos()
			end)
			if ok then
				addAll(list)
			end
		end
	end
	if #photos == 0 then
		-- Special sources (Quick Collection, Previous Import) have no getter
		-- of their own; with nothing selected this returns the filmstrip.
		addAll(cat:getMultipleSelectedOrAllPhotos())
	end
	return photos
end

local handlers = {}

function handlers.ping(_, env)
	return { pong = true, allow_changes = env.allowChanges, developer_commands = env.devTools }
end

function handlers.status(_, env)
	local cat = catalog()
	local path = cat:getPath()
	local selection = selectedPhotos()
	local ids = idsOf(env, selection, 50)
	local target = cat:getTargetPhoto()
	local sources = {}
	for _, source in ipairs(cat:getActiveSources() or {}) do
		sources[#sources + 1] = describeSource(cat, source)
	end
	return {
		lightroom_version = LrApplication.versionString(),
		plugin_version = string.format("%s.%s.%s", tostring(Info.MAJOR), tostring(Info.MINOR), tostring(Info.REVISION)),
		catalog = {
			path = path,
			name = LrPathUtils.removeExtension(LrPathUtils.leafName(path)),
			photo_count = #cat:getAllPhotos(),
		},
		active_module = LrApplicationView.getCurrentModuleName(),
		selection = {
			count = #selection,
			photo_ids = ids,
			truncated = #selection > #ids or nil,
			active_photo_id = target and photoIdOf(env, target) or nil,
		},
		sources = sources,
	}
end

function handlers.get_photos(args, env)
	local scope = args.scope or "selection"
	local limit = tonumber(args.limit) or 5000
	local photos
	if scope == "selection" then
		photos = selectedPhotos()
	elseif scope == "view" then
		photos = viewPhotos()
	elseif scope == "all" then
		photos = catalog():getAllPhotos()
	else
		error('scope must be "selection", "view" or "all".')
	end
	local ids, unidentified = idsOf(env, photos, limit)
	local warnings = {}
	if unidentified > 0 then
		warnings[#warnings + 1] = string.format(
			"%d photo(s) have no LrGeniusAI photo ID yet (their file may be offline) and were left out.",
			unidentified
		)
	end
	return {
		photo_ids = ids,
		count = #ids,
		truncated = (#photos > #ids + unidentified) or nil,
	}, warnings
end

function handlers.find_photos(args, env)
	local criteria = args.criteria or {}
	local limit = tonumber(args.limit) or 1000
	local searchDesc = AgentCommands.buildSearchDesc(criteria)
	local cat = catalog()
	local photos
	if searchDesc then
		photos = cat:findPhotos({ searchDesc = searchDesc, sort = "captureTime", ascending = false })
	else
		photos = cat:getAllPhotos()
	end

	-- Capture dates, filtered here: whole days, in the computer's time zone.
	local afterY, afterM, afterD = AgentCommands.parseDay(criteria.captured_after)
	local beforeY, beforeM, beforeD = AgentCommands.parseDay(criteria.captured_before)
	if afterY or beforeY then
		local from = afterY and LrDate.timeFromComponents(afterY, afterM, afterD, 0, 0, 0, "local") or nil
		local to = beforeY and LrDate.timeFromComponents(beforeY, beforeM, beforeD, 23, 59, 59, "local") or nil
		local times = cat:batchGetRawMetadata(photos, { "dateTimeOriginal" })
		local kept = {}
		for _, photo in ipairs(photos) do
			local t = times[photo] and times[photo].dateTimeOriginal
			if type(t) == "number" and (not from or t >= from) and (not to or t <= to) then
				kept[#kept + 1] = photo
			end
		end
		photos = kept
	end

	local ids, unidentified = idsOf(env, photos, limit)
	local warnings = {}
	if unidentified > 0 then
		warnings[#warnings + 1] =
			string.format("%d matching photo(s) have no LrGeniusAI photo ID yet and were left out.", unidentified)
	end
	return { photo_ids = ids, count = #ids, truncated = (#photos > #ids + unidentified) or nil }, warnings
end

--- Lightroom's colour name for a photo's label, as the agent sends them.
--- Photos without a label come back as "gray" (LrC 15.6), not as nil.
local function labelName(raw)
	if raw == nil or raw == "" or raw == "gray" or raw == "grey" then
		return "none"
	end
	return raw
end

local RAW_INFO_KEYS = { "path", "rating", "pickStatus", "colorNameForLabel", "dateTimeOriginal", "isVirtualCopy" }
local FORMATTED_INFO_KEYS = {
	"fileName",
	"folderName",
	"title",
	"caption",
	"headline",
	"keywordTags",
	"cameraModel",
	"lens",
	"isoSpeedRating",
	"aperture",
	"shutterSpeed",
	"focalLength",
	"exposureBias",
	"dimensions",
	"croppedDimensions",
	"location",
	"city",
	"country",
	"fileType",
}

function handlers.photo_info(args, env)
	local photos, missing = AgentCommands.resolvePhotos(env, args.photo_ids)
	local cat = catalog()
	local raw = cat:batchGetRawMetadata(photos, RAW_INFO_KEYS)
	local formatted = cat:batchGetFormattedMetadata(photos, FORMATTED_INFO_KEYS)
	local flags = { [1] = "pick", [-1] = "reject", [0] = "none" }
	local out = {}
	for _, photo in ipairs(photos) do
		local r = raw[photo] or {}
		local f = formatted[photo] or {}
		local function text(key)
			local v = f[key]
			if v == nil or v == "" then
				return nil
			end
			return v
		end
		out[#out + 1] = {
			photo_id = photoIdOf(env, photo),
			file_name = text("fileName"),
			folder = text("folderName"),
			path = r.path,
			captured = type(r.dateTimeOriginal) == "number" and LrDate.timeToW3CDate(r.dateTimeOriginal) or nil,
			rating = r.rating or 0,
			flag = flags[r.pickStatus or 0] or "none",
			color_label = labelName(r.colorNameForLabel),
			title = text("title"),
			caption = text("caption"),
			headline = text("headline"),
			keywords = text("keywordTags"),
			camera = text("cameraModel"),
			lens = text("lens"),
			iso = text("isoSpeedRating"),
			aperture = text("aperture"),
			shutter_speed = text("shutterSpeed"),
			focal_length = text("focalLength"),
			exposure_bias = text("exposureBias"),
			dimensions = text("croppedDimensions") or text("dimensions"),
			location = text("location"),
			city = text("city"),
			country = text("country"),
			file_type = text("fileType"),
			virtual_copy = r.isVirtualCopy or nil,
		}
	end
	return { photos = out, missing = missing }
end

---------------------------------------------------------------------------
-- Collections
---------------------------------------------------------------------------

--- Finds the existing collection set at "A > B". Read-only: call it outside
--- a write gate, or before anything in the gate created a set. Lightroom
--- refuses to list the children of a set made in the same gate.
local function collectionSetAt(cat, path)
	local parent = nil
	for _, name in ipairs(AgentCommands.splitPath(path)) do
		local found = nil
		local children = parent and parent:getChildCollectionSets() or cat:getChildCollectionSets()
		for _, set in ipairs(children) do
			if set:getName() == name then
				found = set
				break
			end
		end
		if not found then
			return nil
		end
		parent = found
	end
	return parent
end

--- Makes (or returns, if it exists) the collection set at "A > B" inside a
--- write gate, without listing any set's children: createCollectionSet with
--- canReturnPrior hands back an existing set by name.
local function ensureCollectionSet(cat, path)
	local parent = nil
	for _, name in ipairs(AgentCommands.splitPath(path)) do
		parent = cat:createCollectionSet(name, parent, true)
		if not parent then
			error('Could not create the collection set "' .. name .. '".')
		end
	end
	return parent
end

local function findCollection(cat, name, parent)
	local children = parent and parent:getChildCollections() or cat:getChildCollections()
	for _, collection in ipairs(children) do
		if collection:getName() == name then
			return collection
		end
	end
	return nil
end

function handlers.show_photos(args, env)
	local photos, missing = AgentCommands.resolvePhotos(env, args.photo_ids)
	if #photos == 0 then
		error(missingWarning(missing) or "None of these photos are in the open catalog.")
	end
	local name = (args.collection and args.collection ~= "") and args.collection or "Agent results"
	writeAccess(UNDO_NAMES.show, function(cat)
		local set = cat:createCollectionSet(AGENT_COLLECTION_SET, nil, true)
		local collection = cat:createCollection(name, set, true)
		collection:removeAllPhotos()
		collection:addPhotos(photos)
	end)
	-- Looked up again after the write: a collection created inside a write
	-- gate cannot be used as a source until the gate has closed.
	local cat = catalog()
	local set = collectionSetAt(cat, AGENT_COLLECTION_SET)
	local collection = set and findCollection(cat, name, set)
	local warnings = { missingWarning(missing) }
	if collection then
		cat:setActiveSources({ collection })
		local ok = LrTasks.pcall(function()
			cat:setSelectedPhotos(photos[1], photos)
		end)
		if not ok then
			warnings[#warnings + 1] = "The collection is shown, but the photos could not be selected in it."
		end
	else
		warnings[#warnings + 1] = "The collection was filled but could not be shown; open it in the Collections panel."
	end
	return {
		collection = AGENT_COLLECTION_SET .. " > " .. name,
		shown = #photos,
		skipped = #missing,
	}, warnings
end

function handlers.collection(args, env)
	local photos, missing = AgentCommands.resolvePhotos(env, args.photo_ids)
	if #photos == 0 then
		error(missingWarning(missing) or "None of these photos are in the open catalog.")
	end
	local name = args.collection
	if type(name) ~= "string" or name == "" then
		error("collection must be a name.")
	end
	local remove = args.op == "remove"
	local where = (args.collection_set and args.collection_set ~= "") and args.collection_set or nil
	local changed = 0
	if remove then
		-- Found before the gate opens; nothing is created on this path.
		local cat = catalog()
		local parent = where and collectionSetAt(cat, where) or nil
		if where and not parent then
			error('There is no collection set "' .. where .. '".')
		end
		local collection = findCollection(cat, name, parent)
		if not collection then
			error('There is no collection "' .. name .. '" to remove photos from.')
		end
		writeAccess(UNDO_NAMES.collection .. " (remove)", function()
			collection:removePhotos(photos)
			changed = #photos
		end)
	else
		writeAccess(UNDO_NAMES.collection .. " (add)", function(cat)
			local parent = where and ensureCollectionSet(cat, where) or nil
			-- canReturnPrior: an existing collection of that name comes back.
			local collection = cat:createCollection(name, parent, true)
			if not collection then
				error('Could not create the collection "' .. name .. '".')
			end
			collection:addPhotos(photos)
			changed = #photos
		end)
	end
	return {
		collection = ((where and where ~= "") and (where .. " > ") or "") .. name,
		[remove and "removed" or "added"] = changed,
		skipped = #missing,
	}, { missingWarning(missing) }
end

function handlers.list_collections()
	local cat = catalog()
	local out = {}
	local function walk(parent, prefix, depth)
		if #out >= 1000 or depth > 10 then
			return
		end
		local sets = parent and parent:getChildCollectionSets() or cat:getChildCollectionSets()
		local collections = parent and parent:getChildCollections() or cat:getChildCollections()
		for _, collection in ipairs(collections) do
			local smart = collection:isSmartCollection()
			out[#out + 1] = {
				name = collection:getName(),
				path = prefix .. collection:getName(),
				kind = smart and "smart_collection" or "collection",
				-- Counting a smart collection runs its search; skip that cost.
				photo_count = (not smart) and #collection:getPhotos() or nil,
			}
		end
		for _, set in ipairs(sets) do
			out[#out + 1] = { name = set:getName(), path = prefix .. set:getName(), kind = "collection_set" }
			walk(set, prefix .. set:getName() .. " > ", depth + 1)
		end
	end
	walk(nil, "", 0)
	return { collections = out, truncated = #out >= 1000 or nil }
end

---------------------------------------------------------------------------
-- Metadata
---------------------------------------------------------------------------

--- Finds a keyword by "Parent > Child" path without creating anything.
local function findKeyword(cat, path)
	local parts = AgentCommands.splitPath(path)
	local candidates = cat:getKeywords()
	local found = nil
	for _, name in ipairs(parts) do
		found = nil
		for _, keyword in ipairs(candidates or {}) do
			if keyword:getName():lower() == name:lower() then
				found = keyword
				break
			end
		end
		if not found then
			return nil
		end
		candidates = found:getChildren()
	end
	return found
end

function handlers.update_photos(args, env)
	local photos, missing = AgentCommands.resolvePhotos(env, args.photo_ids)
	if #photos == 0 then
		error(missingWarning(missing) or "None of these photos are in the open catalog.")
	end
	local locked
	photos, locked = editable(photos)
	local warnings = { missingWarning(missing) }
	if #locked > 0 then
		warnings[#warnings + 1] = string.format(
			"%d photo(s) are locked by a Lightroom AI operation in progress and were not changed.",
			#locked
		)
	end
	if #photos == 0 then
		error("Every photo is locked by a Lightroom AI operation in progress. Try again when it finishes.")
	end

	local metadata = {}
	for field, value in pairs(type(args.metadata) == "table" and args.metadata or {}) do
		local key = AgentCommands.METADATA_KEYS[field]
		if not key then
			error('The metadata field "' .. tostring(field) .. '" is not supported.')
		end
		metadata[key] = value
	end

	local flag = args.flag and FLAG_VALUES[args.flag] or nil
	if args.flag and flag == nil then
		error('flag must be "pick", "reject" or "none".')
	end

	local removeKeywords = {}
	local unknownKeywords = {}
	for _, path in ipairs(type(args.remove_keywords) == "table" and args.remove_keywords or {}) do
		local keyword = findKeyword(catalog(), path)
		if keyword then
			removeKeywords[#removeKeywords + 1] = keyword
		else
			unknownKeywords[#unknownKeywords + 1] = path
		end
	end
	if #unknownKeywords > 0 then
		warnings[#warnings + 1] = "Not in the catalog, so not removed: " .. table.concat(unknownKeywords, ", ")
	end

	local changed = {}
	writeAccess(UNDO_NAMES.update, function(cat)
		local addKeywords = {}
		for _, path in ipairs(type(args.add_keywords) == "table" and args.add_keywords or {}) do
			local parent = nil
			for _, name in ipairs(AgentCommands.splitPath(path)) do
				parent = cat:createKeyword(name, {}, true, parent, true)
			end
			if parent then
				addKeywords[#addKeywords + 1] = parent
			end
		end
		for _, photo in ipairs(photos) do
			if args.rating ~= nil then
				local rating = tonumber(args.rating)
				photo:setRawMetadata("rating", (rating and rating > 0) and rating or nil)
				changed.rating = true
			end
			if flag ~= nil then
				photo:setRawMetadata("pickStatus", flag)
				changed.flag = true
			end
			if args.color_label ~= nil then
				photo:setRawMetadata("colorNameForLabel", args.color_label)
				changed.color_label = true
			end
			if args.title ~= nil then
				photo:setRawMetadata("title", args.title)
				changed.title = true
			end
			if args.caption ~= nil then
				photo:setRawMetadata("caption", args.caption)
				changed.caption = true
			end
			for key, value in pairs(metadata) do
				photo:setRawMetadata(key, value)
				changed.metadata = true
			end
			for _, keyword in ipairs(addKeywords) do
				photo:addKeyword(keyword)
				changed.add_keywords = true
			end
			for _, keyword in ipairs(removeKeywords) do
				photo:removeKeyword(keyword)
				changed.remove_keywords = true
			end
		end
	end)
	local fields = {}
	for field in pairs(changed) do
		fields[#fields + 1] = field
	end
	table.sort(fields)
	return {
		updated = #photos,
		changed = fields,
		skipped = #missing + #locked,
		undo = 'Edit > Undo "' .. UNDO_NAMES.update .. '" reverts this call.',
	},
		warnings
end

---------------------------------------------------------------------------
-- Develop
---------------------------------------------------------------------------

-- What get_develop_settings returns when the agent names no keys: the
-- settings an editor reaches for first. The full table has hundreds.
local DEFAULT_DEVELOP_KEYS = {
	"ProcessVersion",
	"WhiteBalance",
	"Temperature",
	"Tint",
	"IncrementalTemperature",
	"IncrementalTint",
	"Exposure2012",
	"Contrast2012",
	"Highlights2012",
	"Shadows2012",
	"Whites2012",
	"Blacks2012",
	"Texture",
	"Clarity2012",
	"Dehaze",
	"Vibrance",
	"Saturation",
	"ParametricShadows",
	"ParametricDarks",
	"ParametricLights",
	"ParametricHighlights",
	"Sharpness",
	"LuminanceSmoothing",
	"ColorNoiseReduction",
	"PostCropVignetteAmount",
	"GrainAmount",
	"ConvertToGrayscale",
	"CameraProfile",
	"LensProfileEnable",
	"AutoLateralCA",
	"CropAngle",
	"HasCrop",
}

function handlers.get_develop_settings(args, env)
	local photos, missing = AgentCommands.resolvePhotos(env, { args.photo_id })
	if #photos == 0 then
		error(missingWarning(missing) or "The photo is not in the open catalog.")
	end
	local all = photos[1]:getDevelopSettings() or {}
	local keys = (type(args.keys) == "table" and #args.keys > 0) and args.keys or DEFAULT_DEVELOP_KEYS
	local settings, unknown = {}, {}
	for _, key in ipairs(keys) do
		local value = all[key]
		if value == nil then
			if args.keys then
				unknown[#unknown + 1] = key
			end
		elseif type(value) ~= "table" then
			settings[key] = value
		else
			settings[key] = AgentCommands.toJsonSafe(value)
		end
	end
	local warnings = {}
	if #unknown > 0 then
		warnings[1] = "Not set on this photo (or not a develop setting): " .. table.concat(unknown, ", ")
	end
	return { photo_id = args.photo_id, settings = settings }, warnings
end

-- Develop changes go in chunks: each chunk is one write gate, and between
-- chunks Lightroom stays responsive, progress reaches the agent and a
-- cancelled call stops. (macOS coalesces back-to-back gates into one undo
-- step anyway; the snapshots make every photo revertible either way.)
local DEVELOP_CHUNK = 50

function handlers.apply_develop_settings(args, env)
	local photos, missing = AgentCommands.resolvePhotos(env, args.photo_ids)
	if #photos == 0 then
		error(missingWarning(missing) or "None of these photos are in the open catalog.")
	end
	local locked
	photos, locked = editable(photos)
	local warnings = { missingWarning(missing) }
	if #locked > 0 then
		warnings[#warnings + 1] =
			string.format("%d photo(s) are locked by a Lightroom AI operation and were not changed.", #locked)
	end
	if type(args.settings) ~= "table" or next(args.settings) == nil then
		error("settings must name at least one develop setting.")
	end
	local relative = args.relative == true
	local snapshotName = "Before LrGeniusAI agent edit "
		.. LrDate.timeToUserFormat(LrDate.currentTime(), "%Y-%m-%d %H:%M:%S")

	-- A setting the photo does not have is a typo or a setting of another
	-- process version; applying it would do nothing, silently.
	local known = photos[1] and photos[1]:getDevelopSettings() or {}
	local settings, unknown = {}, {}
	for key, value in pairs(args.settings) do
		if known[key] == nil then
			unknown[#unknown + 1] = key
		else
			settings[key] = value
		end
	end
	if #unknown > 0 then
		table.sort(unknown)
		warnings[#warnings + 1] = "Not develop settings of these photos, ignored: " .. table.concat(unknown, ", ")
	end
	if next(settings) == nil then
		error("None of the settings exist on these photos: " .. table.concat(unknown, ", "))
	end

	local applied, skipped = 0, {}
	local index = 1
	while index <= #photos do
		local chunk = {}
		for i = index, math.min(index + DEVELOP_CHUNK - 1, #photos) do
			chunk[#chunk + 1] = photos[i]
		end
		-- Relative changes need each photo's current values, read before the
		-- gate opens.
		local plans = {}
		for _, photo in ipairs(chunk) do
			local current = relative and (photo:getDevelopSettings() or {}) or {}
			local plan = {}
			for key, value in pairs(settings) do
				local newValue, reason = AgentCommands.developValue(current[key], value, relative)
				if newValue == nil then
					skipped[#skipped + 1] = key .. " (" .. reason .. ")"
				else
					plan[key] = newValue
				end
			end
			plans[#plans + 1] = plan
		end
		writeAccess(UNDO_NAMES.develop, function()
			for i, photo in ipairs(chunk) do
				if next(plans[i]) ~= nil then
					-- false: a snapshot already taken under this name (a second call
					-- within the same second) holds the true "before"; keep it.
					photo:createDevelopSnapshot(snapshotName, false)
					photo:applyDevelopSettings(plans[i], "LrGeniusAI agent")
					applied = applied + 1
				end
			end
		end)
		index = index + #chunk
		if env.progress and env.progress(index - 1, #photos, "Applying develop settings") then
			warnings[#warnings + 1] =
				string.format("Stopped after %d of %d photos because the agent cancelled the call.", index - 1, #photos)
			break
		end
	end
	if #skipped > 0 then
		warnings[#warnings + 1] = "Some relative changes could not apply: "
			.. table.concat(skipped, ", ", 1, math.min(#skipped, 10))
	end
	return {
		updated = applied,
		snapshot = snapshotName,
		relative = relative,
		undo = 'Each photo has a develop snapshot named "' .. snapshotName .. '"; Edit > Undo also reverts the call.',
	},
		warnings
end

---------------------------------------------------------------------------
-- Previews, indexing, logs
---------------------------------------------------------------------------

function handlers.render_preview(args, env)
	local photos, missing = AgentCommands.resolvePhotos(env, { args.photo_id })
	if #photos == 0 then
		error(missingWarning(missing) or "The photo is not in the open catalog.")
	end
	local photo = photos[1]
	local size = math.max(256, math.min(2048, tonumber(args.size) or 1024))
	local folder = LrPathUtils.child(LrPathUtils.getStandardFilePath("temp"), "lrgenius-agent-previews")
	LrFileUtils.createAllDirectories(folder)
	local name = "preview-"
		.. tostring(photo.localIdentifier)
		.. "-"
		.. tostring(math.floor(LrDate.currentTime() * 1000))
	local session = LrExportSession({
		photosToExport = { photo },
		exportSettings = {
			LR_export_destinationType = "specificFolder",
			LR_export_destinationPathPrefix = folder,
			LR_export_useSubfolder = false,
			LR_format = "JPEG",
			LR_jpeg_quality = 0.85,
			LR_export_colorSpace = "sRGB",
			LR_size_doConstrain = true,
			LR_size_maxHeight = size,
			LR_size_maxWidth = size,
			LR_size_resizeType = "longEdge",
			LR_size_units = "pixels",
			LR_outputSharpeningOn = false,
			LR_minimizeEmbeddedMetadata = true,
			LR_useWatermark = false,
			LR_includeVideoFiles = false,
			LR_renamingTokensOn = true,
			LR_tokens = "{{custom_token}}",
			LR_tokenCustomString = name,
			LR_collisionHandling = "overwrite",
		},
	})
	local path, failure
	for _, rendition in session:renditions() do
		local ok, pathOrMessage = rendition:waitForRender()
		if ok then
			path = pathOrMessage
		else
			failure = pathOrMessage
		end
	end
	if not path then
		error("Lightroom could not render the photo: " .. tostring(failure or "unknown reason"))
	end
	return { path = path, file_name = photo:getFormattedMetadata("fileName") }
end

local INDEX_TASKS = { embeddings = true, faces = true, species = true, cull = true }

function handlers.analyze_photos(args, env)
	local tasks = {}
	for _, task in ipairs(type(args.tasks) == "table" and args.tasks or { "embeddings" }) do
		if task == "metadata" then
			error(
				"AI titles, captions and keywords are written by Analyze & Index's own dialog, which "
					.. "offers review before anything is saved. Ask the user to run it, or write metadata "
					.. "yourself with update_photos after looking at the photos."
			)
		end
		if not INDEX_TASKS[task] then
			error('Unknown task "' .. tostring(task) .. '".')
		end
		tasks[#tasks + 1] = task
	end
	local photos, missing = AgentCommands.resolvePhotos(env, args.photo_ids)
	if #photos == 0 then
		error(missingWarning(missing) or "None of these photos are in the open catalog.")
	end
	if not SearchIndexAPI.confirmModelsReadyForTasks then
		error("This plugin build cannot check the AI models; update the plugin.")
	end

	-- Only the photos that still lack a result, unless asked to redo.
	local toProcess = photos
	local warnings = { missingWarning(missing) }
	if not args.redo then
		local ids = idsOf(env, photos)
		local pending = SearchIndexAPI.checkUnprocessedPhotoIds(ids, tasks)
		if type(pending) == "table" then
			local wanted = {}
			for _, id in ipairs(pending) do
				wanted[id] = true
			end
			toProcess = {}
			for _, photo in ipairs(photos) do
				local id = photoIdOf(env, photo)
				if id and wanted[id] then
					toProcess[#toProcess + 1] = photo
				end
			end
		end
	end
	if #toProcess == 0 then
		return { processed = 0, already_done = #photos, tasks = tasks }, warnings
	end

	local scope = LrProgressScope({
		title = AgentCommands.describe({ action = "analyze_photos", args = args }),
		functionContext = env.context,
	})
	local wantsFaces = false
	for _, task in ipairs(tasks) do
		wantsFaces = wantsFaces or task == "faces"
	end
	local options = {
		tasks = tasks,
		enableMetadata = false,
		enableFaces = wantsFaces,
		species_prefilter = prefs.speciesPrefilter ~= false,
		species_min_confidence = prefs.speciesMinConfidence or Defaults.speciesMinConfidence,
		submit_keywords = false,
		submit_face_tags = false,
		submit_folder_names = false,
		regenerate_metadata = args.redo == true,
	}
	-- The pipeline reports to its own progress bar; this passes each indexed
	-- photo on to the agent, and stops the run when the agent gives up.
	local analyzed = 0
	options.onPhotoAnalyzed = function()
		analyzed = analyzed + 1
		if env.progress and env.progress(analyzed, #toProcess, "Indexing photos") then
			scope:cancel()
		end
	end
	local status, processed, failed, _, combinedError, combinedWarnings =
		SearchIndexAPI.analyzeAndIndexSelectedPhotos(toProcess, scope, options, false)
	scope:done()
	if combinedWarnings and combinedWarnings ~= "" then
		warnings[#warnings + 1] = combinedWarnings
	end
	if status == "allfailed" then
		error(combinedError or "Indexing failed for every photo.")
	end
	if combinedError and combinedError ~= "" then
		warnings[#warnings + 1] = combinedError
	end
	return {
		status = status,
		processed = processed,
		failed = failed,
		already_done = #photos - #toProcess,
		tasks = tasks,
	},
		warnings
end

--- The last lines of a text file, without reading a huge log whole.
local function tailFile(path, maxBytes)
	local ok, content = LrTasks.pcall(function()
		local fh = io and io.open(path, "rb")
		if not fh then
			return LrFileUtils.readFile(path)
		end
		local size = fh:seek("end")
		fh:seek("set", math.max(0, size - maxBytes))
		local text = fh:read("*a")
		fh:close()
		return text
	end)
	if ok then
		return content
	end
	return nil
end

function handlers.read_log(args)
	local path = Util.getLogfilePath()
	if not path or not LrFileUtils.exists(path) then
		error("The plugin's log file was not found" .. (path and (" at " .. path) or "") .. ".")
	end
	local text = tailFile(path, 2 * 1024 * 1024) or ""
	local needle = (type(args.contains) == "string" and args.contains ~= "") and args.contains:lower() or nil
	local lines = {}
	for line in text:gmatch("[^\r\n]+") do
		if not needle or line:lower():find(needle, 1, true) then
			lines[#lines + 1] = line
		end
	end
	local want = math.max(1, math.min(2000, tonumber(args.lines) or 100))
	local out = {}
	for i = math.max(1, #lines - want + 1), #lines do
		out[#out + 1] = lines[i]
	end
	return { path = path, lines = out }
end

---------------------------------------------------------------------------
-- Testing and developer commands
---------------------------------------------------------------------------

function handlers.run_tests(args, env)
	return PluginTests.run({
		suite = args.suite,
		filter = args.filter,
		includeWrites = args.include_writes == true,
		allowChanges = env.allowChanges,
		progress = env.progress,
		env = env,
	})
end

--- Compiles Lua source. Lightroom runs Lua 5.1 (loadstring); the headless
--- tests run a newer Lua (load).
local function compile(code, chunkName)
	if type(loadstring) == "function" then
		return loadstring(code, chunkName)
	end
	if type(load) == "function" then
		return load(code, chunkName, "t")
	end
	return nil, "this Lua cannot compile code at runtime"
end

-- Lightroom's Lua (checked with LrC 15.6) has no print, no setfenv and no
-- getfenv, so a compiled chunk cannot be handed an environment of its own: it
-- runs against the plugin's shared globals. What eval adds to those (a print
-- that is captured) goes in as a chunk argument, bound to a local by a prefix
-- on the first line, which keeps line numbers in error messages right.
local EVAL_PREFIX = "local print, _PLUGIN = ...; "

function handlers.eval_lua(args)
	local output = {}
	local function capture(...)
		local parts = {}
		for i = 1, select("#", ...) do
			parts[#parts + 1] = tostring((select(i, ...)))
		end
		output[#output + 1] = table.concat(parts, "\t")
	end
	-- Run in an environment of its own that sees every global, so print()
	-- can be captured without replacing the real one.
	local chunk, compileError = compile(EVAL_PREFIX .. (args.code or ""), "=agent")
	if not chunk then
		error("The code does not compile: " .. tostring(compileError))
	end
	local results = { LrTasks.pcall(chunk, capture, _PLUGIN) }
	if not results[1] then
		error("The code raised an error: " .. tostring(results[2]))
	end
	local values = {}
	for i = 2, #results do
		values[#values + 1] = AgentCommands.toJsonSafe(results[i])
	end
	return { value = values[1], values = (#values > 1) and values or nil, output = output }
end

function handlers.reload_modules(args)
	local reloaded, failed = {}, {}
	for _, name in ipairs(type(args.modules) == "table" and args.modules or {}) do
		if type(name) ~= "string" or not name:match("^[%w_]+$") then
			failed[#failed + 1] = { module = tostring(name), error = "not a module name" }
		else
			local path = LrPathUtils.child(_PLUGIN.path, name .. ".lua")
			local ok, err = LrTasks.pcall(function()
				if not LrFileUtils.exists(path) then
					error("no such file: " .. path)
				end
				local chunk, compileError = compile(LrFileUtils.readFile(path), "@" .. path)
				if not chunk then
					error(compileError)
				end
				-- Modules define globals (Util, SearchIndexAPI, ...). Running the
				-- file replaces those tables; copying the new contents into the
				-- old tables keeps every reference other modules hold valid.
				local before = {}
				for key, value in pairs(_G) do
					if type(value) == "table" then
						before[key] = value
					end
				end
				chunk()
				for key, old in pairs(before) do
					local new = rawget(_G, key)
					if type(new) == "table" and new ~= old then
						for k, v in pairs(new) do
							old[k] = v
						end
						_G[key] = old
					end
				end
			end)
			if ok then
				reloaded[#reloaded + 1] = name
			else
				failed[#failed + 1] = { module = name, error = tostring(err) }
			end
		end
	end
	return {
		reloaded = reloaded,
		failed = failed,
		note = "Module-level state (caches, upvalues) starts fresh. Task*.lua files are read each time their menu item runs.",
	}
end

---------------------------------------------------------------------------
-- Dispatch
---------------------------------------------------------------------------

--- True for the commands that change photos.
function AgentCommands.changesPhotos(action)
	return CHANGES[action] == true
end

--- Runs one claimed command. Never raises: every outcome becomes the reply
--- the backend expects ({ ok, result, error, warnings, refused }).
function AgentCommands.run(command, env)
	local action = command and command.action
	local handler = handlers[action]
	if not handler then
		return {
			ok = false,
			error = 'This plugin does not know the command "' .. tostring(action) .. '". Updating the plugin may help.',
		}
	end
	if CHANGES[action] and not env.allowChanges then
		return {
			ok = false,
			refused = "changes",
			error = "The user has set the agent bridge to read-only.",
		}
	end
	if DEVELOPER[action] and not env.devTools then
		return { ok = false, refused = "dev_tools", error = "Developer commands are off." }
	end

	local ok, result, warnings = LrTasks.pcall(handler, command.args or {}, env)
	-- Drop the nil holes `{ missingWarning(missing) }` leaves behind.
	local cleaned = {}
	for _, w in pairs(type(warnings) == "table" and warnings or {}) do
		if type(w) == "string" and w ~= "" then
			cleaned[#cleaned + 1] = w
		end
	end
	if not ok then
		local message = tostring(result):gsub("^.-:%d+: ", "")
		return { ok = false, error = message, warnings = cleaned }
	end
	return { ok = true, result = AgentCommands.toJsonSafe(result or {}), warnings = cleaned }
end

return AgentCommands
