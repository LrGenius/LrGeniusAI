--[[
    Add Missing Search Previews: gives photos that were indexed before
    LrGeniusAI stored previews the small picture the browser search page shows,
    without indexing them again.

    The backend says which photos of the chosen scope are indexed but have no
    preview (/v1/photos/thumbnails/missing). Those are exported as small JPEGs,
    a few dozen photos per export session, and each goes to
    /v1/photos/thumbnail, which scales it to the preview size. Nothing else
    about a photo is touched.

    Export, not `photo:requestJpegThumbnail`: Lightroom answers that a few
    times and then stops delivering, so a run over a catalog ends up exporting
    anyway, after a timeout per photo.
]]

--- Long edge of the export. The backend stores 256 px, so this leaves room
--- for its own downscale.
local RENDER_LONG_EDGE = 512

--- Photo ids per question to the backend, so a whole catalog does not go out
--- as one request body.
local LOOKUP_CHUNK = 2000

--- Failures named one by one in the final dialog; the rest are counted.
local MAX_LISTED_FAILURES = 8

local function chooseScope(context)
	local f = LrView.osFactory()
	local props = LrBinding.makePropertyTable(context)
	props.scope = prefs.fillPreviewsScope or "selected"
	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:static_text({
			title = "Photos indexed before LrGeniusAI stored previews have no picture on the\n"
				.. "browser search page. This exports a small picture of each of them and\n"
				.. "adds it. Nothing else about the photos changes.",
		}),
		f:row({
			f:static_text({ title = "Photos:" }),
			f:popup_menu({
				value = LrView.bind("scope"),
				items = {
					{ title = "Selected photos", value = "selected" },
					{ title = "Current view", value = "view" },
					{ title = "All photos in the catalog", value = "all" },
				},
			}),
		}),
	})
	local result = LrDialogs.presentModalDialog({
		title = "Add Missing Search Previews",
		contents = contents,
		actionVerb = "Add Previews",
	})
	if result ~= "ok" then
		return nil
	end
	prefs.fillPreviewsScope = props.scope
	return props.scope
end

--- photoId -> photo for `photos`; photos whose id cannot be worked out are
--- counted. Several photos can share an id (virtual copies of one file): the
--- first one stands for the file, which is what the index stores.
local function photosById(photos, scope)
	local byId, ids, unresolved = {}, {}, 0
	for i, photo in ipairs(photos) do
		if scope:isCanceled() then
			return nil
		end
		local photoId = SearchIndexAPI.getPhotoIdForPhoto(photo)
		if photoId then
			if byId[photoId] == nil then
				byId[photoId] = photo
				table.insert(ids, photoId)
			end
		else
			unresolved = unresolved + 1
		end
		if i % 200 == 0 then
			scope:setCaption("Identifying photos (" .. i .. " of " .. #photos .. ")...")
			LrTasks.yield()
		end
	end
	return byId, ids, unresolved
end

local function missingIds(ids, scope)
	local missing, indexed = {}, 0
	for start = 1, #ids, LOOKUP_CHUNK do
		if scope:isCanceled() then
			return nil
		end
		local chunk = {}
		for i = start, math.min(start + LOOKUP_CHUNK - 1, #ids) do
			table.insert(chunk, ids[i])
		end
		local response, err = SearchIndexAPI.getPhotosMissingThumbnails(chunk)
		if not response then
			return nil, err
		end
		indexed = indexed + (tonumber(response.indexed) or 0)
		for _, id in ipairs(response.photo_ids) do
			table.insert(missing, id)
		end
	end
	return missing, nil, indexed
end

local function run(context)
	local chosen = chooseScope(context)
	if not chosen then
		return
	end
	if not SearchIndexAPI.pingServer() then
		ErrorHandler.handleError(
			"The backend server is not responding",
			"Open Plug-in Manager > LrGeniusAI and use Restart Backend, then try again."
		)
		return
	end

	local scope = LrProgressScope({ title = "Adding search previews", functionContext = context })
	scope:setCaption("Collecting photos...")
	local photos, selectErr = PhotoSelector.getPhotosInScope(chosen)
	if not photos then
		scope:done()
		ErrorHandler.handleError("Could not collect the photos", selectErr)
		return
	end
	if #photos == 0 then
		scope:done()
		LrDialogs.message("Add Missing Search Previews", "There are no photos in this scope.", "info")
		return
	end

	local byId, ids, unresolved = photosById(photos, scope)
	if not byId then
		scope:done()
		return
	end
	scope:setCaption("Asking the backend which photos have no preview...")
	local missing, lookupErr, indexed = missingIds(ids, scope)
	if not missing then
		scope:done()
		if lookupErr then
			ErrorHandler.handleError("Could not ask the backend which photos lack a preview", lookupErr)
		end
		return
	end
	if #missing == 0 then
		scope:done()
		local note = indexed == 0 and "None of these photos is indexed yet. Analyze & Index Photos adds the preview."
			or "All " .. indexed .. " indexed photos in this scope already have a preview."
		LrDialogs.message("Add Missing Search Previews", note, "info")
		return
	end

	local total = #missing
	local added, failures = 0, {}
	local function upload(photoId, path)
		local ok, err = SearchIndexAPI.uploadPhotoThumbnail(photoId, path)
		LrTasks.pcall(function()
			LrFileUtils.delete(path)
		end)
		if ok then
			added = added + 1
		else
			local photo = byId[photoId]
			local name = photo and photo:getFormattedMetadata("fileName") or photoId
			table.insert(failures, name .. ": " .. tostring(err))
		end
	end

	local done = 0
	local function step(caption)
		done = done + 1
		scope:setPortionComplete(done, total)
		scope:setCaption(caption .. " (" .. done .. " of " .. total .. ")")
	end

	-- Small exports, a few dozen per session.
	local chunkSize = Util.exportChunkSize(prefs.exportChunkSize)
	local index = 1
	while index <= #missing and not scope:isCanceled() do
		local chunkIds, chunkPhotos = {}, {}
		for i = index, math.min(index + chunkSize - 1, #missing) do
			table.insert(chunkIds, missing[i])
			table.insert(chunkPhotos, byId[missing[i]])
		end
		index = index + #chunkIds
		local paths, exportFailures = SearchIndexAPI.exportPhotosForIndexing(chunkPhotos, {
			LR_size_maxHeight = RENDER_LONG_EDGE,
			LR_jpeg_quality = 0.8, -- 0 to 1, not percent
		}, function()
			return scope:isCanceled()
		end)
		for i, photoId in ipairs(chunkIds) do
			local photo = chunkPhotos[i]
			if paths[photo] then
				upload(photoId, paths[photo])
			elseif not scope:isCanceled() then
				table.insert(
					failures,
					photo:getFormattedMetadata("fileName")
						.. ": the export failed ("
						.. tostring(exportFailures[photo] or "Lightroom gave no reason")
						.. ")"
				)
			end
			if not scope:isCanceled() then
				step("Exporting previews")
			end
		end
	end

	local canceled = scope:isCanceled()
	scope:done()

	local summary = added .. " of " .. total .. " photos got a preview."
	if canceled then
		summary = "Stopped. " .. summary
	end
	if unresolved > 0 then
		summary = summary .. "\n" .. unresolved .. " photo(s) were skipped: their ID could not be worked out."
	end

	if #failures > 0 then
		local listed = {}
		for i = 1, math.min(#failures, MAX_LISTED_FAILURES) do
			table.insert(listed, failures[i])
		end
		if #failures > MAX_LISTED_FAILURES then
			table.insert(listed, "... and " .. (#failures - MAX_LISTED_FAILURES) .. " more (see the plug-in log).")
		end
		for _, f in ipairs(failures) do
			log:error("Search preview not added: " .. f)
		end
		ErrorHandler.handleError(
			#failures .. " photo(s) did not get a search preview",
			summary .. "\n\n" .. table.concat(listed, "\n")
		)
	else
		LrDialogs.message("Add Missing Search Previews", summary, "info")
	end
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskFillSearchPreviews", function(context)
		local ok, err = LrTasks.pcall(run, context)
		if not ok then
			ErrorHandler.handleError("Adding search previews failed", tostring(err))
		end
	end)
end)
