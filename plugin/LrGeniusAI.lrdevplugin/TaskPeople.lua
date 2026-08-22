--[[
    People: opens the People page in the browser and performs the Lightroom
    half of it.

    The interface itself — the person grid, names, re-clustering, filtering —
    is the backend's /ui/people page. Lightroom keeps only what a browser
    cannot do: turning a selection of people into a collection and switching
    the Library to it. While this task runs it polls /ui/actions and performs
    whatever the page queued there; cancelling the progress bar ends it.
]]

--- Wie oft die Aktions-Queue des Browsers abgefragt wird.
local POLL_INTERVAL_SECONDS = 0.5

--- Nach so vielen Sekunden ohne Lebenszeichen der Seite: Hinweis mit der URL.
local PAGE_OPEN_WARN_SECONDS = 25

--- So viele Poll-Fehler hintereinander gelten als "Backend weg" (nicht als Aussetzer).
local MAX_CONSECUTIVE_POLL_ERRORS = 6

--- Sammelt Backend-Warnungen einer Aktion; dedupliziert, damit eine
--- laufweite Ursache nicht einmal pro Person auftaucht.
local function addWarning(warnings, message)
	if type(message) ~= "string" or message == "" then
		return
	end
	for _, existing in ipairs(warnings) do
		if existing == message then
			return
		end
	end
	warnings[#warnings + 1] = message
end

--- Zeigt gesammelte Warnungen; ein paar im Klartext, der Rest als Anzahl.
local function reportWarnings(warnings)
	if #warnings == 0 then
		return
	end
	local SHOWN = 5
	local lines = {}
	for i = 1, math.min(SHOWN, #warnings) do
		lines[#lines + 1] = warnings[i]
	end
	if #warnings > SHOWN then
		lines[#lines + 1] = string.format("...and %d more.", #warnings - SHOWN)
	end
	LrDialogs.message("Some face data came back incomplete", table.concat(lines, "\n"), "warning")
end

--- Baut sortierte Foto-ID-Liste aus API-Antwort.
local function photoIdsFromPersonResponse(resp)
	if type(resp) ~= "table" then
		return {}
	end
	return type(resp.photo_ids) == "table" and resp.photo_ids
		or (type(resp.photo_uuids) == "table" and resp.photo_uuids or {})
end

--- Union: jedes Foto, das mindestens eine der Personen enthält (Reihenfolge: erstes Auftreten).
local function unionPhotoIdsForEntries(entries, warnings)
	local seen = {}
	local photoIdsOrdered = {}
	for _, ent in ipairs(entries) do
		local pid = ent and ent.person_id
		if pid and pid ~= "" then
			local resp, err = SearchIndexAPI.getPhotosForPerson(pid)
			if err or type(resp) ~= "table" then
				ErrorHandler.handleError("Could not get photos for person", err or "No data")
				return nil
			end

			if resp and resp.warning then
				log:warn("GetPhotosForPerson warning: " .. tostring(resp.warning))
				addWarning(warnings, resp.warning)
			end

			local ids = photoIdsFromPersonResponse(resp)
			for _, photoId in ipairs(ids) do
				if not seen[photoId] then
					seen[photoId] = true
					table.insert(photoIdsOrdered, photoId)
				end
			end
		end
	end
	return photoIdsOrdered
end

--- Schnittmenge: Fotos, in denen alle Personen gemeinsam vorkommen (Reihenfolge wie erste Person).
local function intersectPhotoIdsForEntries(entries, warnings)
	if #entries == 0 then
		return {}
	end
	local sets = {}
	local firstIds = nil
	for _, ent in ipairs(entries) do
		local pid = ent and ent.person_id
		if not pid or pid == "" then
			return {}
		end
		local resp, err = SearchIndexAPI.getPhotosForPerson(pid)
		if err or type(resp) ~= "table" then
			ErrorHandler.handleError("Could not get photos for person", err or "No data")
			return nil
		end
		if resp.warning then
			log:warn("GetPhotosForPerson warning: " .. tostring(resp.warning))
			addWarning(warnings, resp.warning)
		end
		local ids = photoIdsFromPersonResponse(resp)
		if not firstIds then
			firstIds = ids
		end
		local t = {}
		for _, id in ipairs(ids) do
			t[id] = true
		end
		sets[#sets + 1] = t
	end
	if #sets == 0 then
		return {}
	end
	local acc = sets[1]
	for i = 2, #sets do
		local nxt = sets[i]
		local newAcc = {}
		for id in pairs(acc) do
			if nxt[id] then
				newAcc[id] = true
			end
		end
		acc = newAcc
	end
	local ordered = {}
	for _, id in ipairs(firstIds) do
		if acc[id] then
			table.insert(ordered, id)
		end
	end
	return ordered
end

--- Führt "Show in Library" aus. entries: { person_id, person_name? }[]; matchMode: "union" | "intersection".
local function doShowInLibrary(entries, matchMode)
	if not entries or #entries == 0 then
		LrDialogs.message("No people selected", "Select one or more people on the People page, then try again.")
		return
	end

	local mode = (matchMode == "intersection") and "intersection" or "union"
	local warnings = {}
	local photoIdsOrdered
	if mode == "intersection" and #entries >= 2 then
		photoIdsOrdered = intersectPhotoIdsForEntries(entries, warnings)
	else
		photoIdsOrdered = unionPhotoIdsForEntries(entries, warnings)
	end

	if photoIdsOrdered == nil then
		return
	end

	-- A degraded lookup still changes what lands in the collection, so it is
	-- reported before the result dialog claims success.
	reportWarnings(warnings)

	if #photoIdsOrdered == 0 then
		if mode == "intersection" and #entries >= 2 then
			LrDialogs.message("No photos", "No photos contain all selected people together.")
		else
			LrDialogs.message("No photos", "No photos found for this person.")
		end
		return
	end

	local catalog = LrApplication.activeCatalog()
	local photos = SearchIndexAPI.findPhotosByPhotoIds(photoIdsOrdered)
	if #photos == 0 then
		LrDialogs.message("Not in catalog", "Photos for this person are not in the current catalog.")
		return
	end

	local nameParts = {}
	for _, ent in ipairs(entries) do
		local n = ent and ent.person_name
		if type(n) == "string" and n ~= "" then
			nameParts[#nameParts + 1] = n
		elseif ent and ent.person_id and ent.person_id ~= "" then
			nameParts[#nameParts + 1] = ent.person_id
		end
	end
	local label = table.concat(nameParts, ", ")
	if #label > 100 then
		label = string.format("People (%d)", #entries)
	end
	local collectionName = string.format("%s @ %s", label, LrDate.timeToW3CDate(LrDate.currentTime()))

	local collectionSet, collection
	catalog:withWriteAccessDo("Create Collection Set", function()
		collectionSet = catalog:createCollectionSet("People", nil, true)
	end, Defaults.catalogWriteAccessOptions)
	if not collectionSet then
		ErrorHandler.handleError("Collection set error", "Could not create collection set for people.")
		return
	end

	catalog:withWriteAccessDo("Create Collection", function()
		collection = catalog:createCollection(collectionName, collectionSet, false)
	end, Defaults.catalogWriteAccessOptions)
	if not collection then
		ErrorHandler.handleError("Collection error", "Could not create collection for this person.")
		return
	end

	catalog:withWriteAccessDo("Add Photos to Collection", function()
		collection:addPhotos(photos)
	end, Defaults.catalogWriteAccessOptions)

	catalog:setActiveSources({ collection })
	LrApplicationView.gridView()
	LrDialogs.message("Done", string.format('%d photo(s) added to collection "%s".', #photos, collectionName))
end

--- Übersetzt eine Aktion der Browser-Seite in Lightroom-Arbeit.
local function runAction(action)
	if type(action) ~= "table" then
		return
	end

	if action.action == "show_in_library" then
		local names = (type(action.person_names) == "table") and action.person_names or {}
		local entries = {}
		for _, personId in ipairs(type(action.person_ids) == "table" and action.person_ids or {}) do
			if type(personId) == "string" and personId ~= "" then
				local personName = names[personId]
				entries[#entries + 1] = {
					person_id = personId,
					person_name = (type(personName) == "string" and personName ~= "") and personName or nil,
				}
			end
		end
		if #entries == 0 then
			ErrorHandler.handleError(
				"Nothing to show",
				"The People page asked to show a selection, but it contained no usable person."
			)
			return
		end
		doShowInLibrary(entries, action.match_mode)
		return
	end

	-- The backend only queues actions it knows, so this is a page from a
	-- newer build talking to an older plugin. Say so rather than looking
	-- like the click did nothing.
	ErrorHandler.handleError(
		"Unsupported request from the People page",
		string.format(
			'This plugin does not know the action "%s". Updating the LrGeniusAI plugin should fix it.',
			tostring(action.action)
		)
	)
end

--- Poll-Schleife: holt Aktionen der Seite, bis der Nutzer den Fortschrittsbalken abbricht.
local function pollForActions(scope, pageUrl)
	local startedAt = LrDate.currentTime()
	local everSawPage = false
	local warnedAboutMissingPage = false
	local consecutiveErrors = 0

	while not scope:isCanceled() do
		local resp, err = SearchIndexAPI.takeUiActions()

		if err then
			consecutiveErrors = consecutiveErrors + 1
			if consecutiveErrors >= MAX_CONSECUTIVE_POLL_ERRORS then
				ErrorHandler.handleError(
					"Lost the connection to the LrGeniusAI backend",
					"People is no longer listening for the browser page. Close the page and open People again. ("
						.. tostring(err)
						.. ")"
				)
				return
			end
		else
			consecutiveErrors = 0
			-- `_request` can hand back a 2xx with nothing decodable in it;
			-- treat that as "no actions this round" rather than indexing nil.
			resp = (type(resp) == "table") and resp or {}
			local actions = (type(resp.actions) == "table") and resp.actions or {}
			for _, action in ipairs(actions) do
				scope:setCaption("Working on a request from the People page...")
				local ok, actionErr = LrTasks.pcall(runAction, action)
				if not ok then
					ErrorHandler.handleError("Could not carry out the People request", tostring(actionErr))
				end
				scope:setCaption("Waiting for the People page in your browser...")
			end

			if resp.page_open then
				everSawPage = true
			end

			-- The browser never came up (blocked pop-up, no default browser).
			-- Without this the progress bar would just sit there forever.
			--
			-- Once only, and only while the page has never been seen at all:
			-- a page that is open but in a background tab has its heartbeat
			-- throttled by the browser, and re-testing `page_open` later would
			-- pop this dialog at someone whose page is working fine.
			if
				not everSawPage
				and not warnedAboutMissingPage
				and (LrDate.currentTime() - startedAt) > PAGE_OPEN_WARN_SECONDS
			then
				warnedAboutMissingPage = true
				LrDialogs.message(
					"People did not open in your browser",
					"Open this address manually to continue:\n\n" .. pageUrl,
					"warning"
				)
			end
		end

		LrTasks.sleep(POLL_INTERVAL_SECONDS)
	end
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskPeople", function(context)
		if not Util.waitForServerDialog() then
			return
		end

		local pageUrl = SearchIndexAPI.getPeopleUiUrl()
		log:info("Opening the People page at " .. pageUrl)
		LrHttp.openUrlInBrowser(pageUrl)

		local scope = LrProgressScope({
			title = "People (open in your browser)",
			functionContext = context,
		})
		scope:setCaption("Waiting for the People page in your browser...")

		local ok, err = LrTasks.pcall(pollForActions, scope, pageUrl)
		scope:done()

		if not ok then
			ErrorHandler.handleError("Error", tostring(err))
		end
	end)
end)
