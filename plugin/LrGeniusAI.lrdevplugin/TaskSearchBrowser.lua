--[[
    Search (browser): opens the search page the backend serves at
    /v1/ui/search and performs the Lightroom half of it.

    The page searches by itself — the same /v1/search the Advanced Search
    dialog uses, re-run live as the options change, with a preview of every
    result. Lightroom keeps what a browser cannot do: telling the page what
    the current view and selection are, and turning the results into a
    collection. While this task runs it polls /v1/ui/actions?page=search and
    performs whatever the page queued; cancelling the progress bar ends it.

    The Advanced Search dialog (TaskSemanticSearch.lua) stays as it was.
]]

--- How often the page's action queue is polled.
local POLL_INTERVAL_SECONDS = 0.5

--- Seconds without a sign of the page before the address is shown.
local PAGE_OPEN_WARN_SECONDS = 25

--- Poll errors in a row that count as "the backend is gone" rather than a blip.
local MAX_CONSECUTIVE_POLL_ERRORS = 6

--- The ID of each photo; photos whose ID cannot be resolved are counted, not
--- sent — the page says how many could not be searched.
local function idsOf(photos)
	local ids, unresolved = {}, 0
	for _, photo in ipairs(photos or {}) do
		local photoId = SearchIndexAPI.getPhotoIdForPhoto(photo)
		if photoId then
			ids[#ids + 1] = photoId
		else
			unresolved = unresolved + 1
		end
	end
	return ids, unresolved
end

--- Names of the sources Lightroom is showing, for the page's "Current view".
local function activeSourceNames(catalog)
	local names = {}
	for _, source in ipairs(catalog:getActiveSources() or {}) do
		if type(source) == "string" then
			names[#names + 1] = (source == "kAllPhotos" or source == catalog.kAllPhotos) and "All Photographs"
				or (source == "kPreviousImport" and "Previous Import" or nil)
		else
			local ok, name = LrTasks.pcall(function()
				return source:getName()
			end)
			if ok then
				names[#names + 1] = name
			end
		end
	end
	return names
end

--- Reads the view and selection and posts them as the page's context.
--- Returns true, or false plus an error message.
local function postContext(scope)
	scope:setCaption("Reading the current view and selection...")
	local catalog = LrApplication.activeCatalog()
	-- An unreadable view (a collection set, nothing selected in the sources)
	-- is an empty one for the page, which then offers no "Current view".
	local viewPhotos = PhotoSelector.getPhotosInScope("view") or {}
	local selectedPhotos = PhotoSelector.getPhotosInScope("selected") or {}
	local viewIds, viewUnresolved = idsOf(viewPhotos)
	local selectionIds, selectionUnresolved = idsOf(selectedPhotos)
	local context = SearchBrowser.context({
		catalogId = SearchIndexAPI.catalogIdForRequests(),
		viewIds = viewIds,
		viewLabel = SearchBrowser.viewLabel(activeSourceNames(catalog)),
		viewUnresolved = viewUnresolved,
		selectionIds = selectionIds,
		selectionUnresolved = selectionUnresolved,
		prefs = prefs,
	})
	local result, err = SearchIndexAPI.setSearchUiContext(context)
	if not result then
		return false, Util.userFacingError(tostring(err))
	end
	return true
end

--- Creates the collection for a `show_search_results` action, in rank order,
--- under the "Search Results" set the Advanced Search dialog uses.
local function showResults(action)
	local ids = SearchBrowser.resultIds(action)
	if #ids == 0 then
		ErrorHandler.handleError(
			"Nothing to show",
			"The search page asked to show its results, but they held no usable photo."
		)
		return
	end

	-- Remembered like a search made in the dialog, so its history has it too.
	if type(action.term) == "string" and Util.trim(action.term) ~= "" then
		prefs.searchHistory = Util.pushHistory(prefs.searchHistory, action.term, Defaults.inputHistoryLength)
	end

	local photos = SearchIndexAPI.findPhotosByPhotoIds(ids)
	local missing = SearchBrowser.missingMessage(#ids, #photos, #photos < #ids and IndexCleanup.requestSoon())
	if #photos == 0 then
		LrDialogs.message("Not in this catalog", missing or "None of the results are in this catalog.", "warning")
		return
	end

	local catalog = LrApplication.activeCatalog()
	local name =
		Util.searchCollectionName(SearchBrowser.collectionNameArgs(action, LrDate.timeToW3CDate(LrDate.currentTime())))
	local collection
	local write = catalog:withWriteAccessDo("Create Search Results Collection", function()
		local set = catalog:createCollectionSet("Search Results", nil, true)
		if set then
			collection = catalog:createCollection(name, set, false)
			if collection then
				-- `findPhotosByPhotoIds` keeps the order it was given, which is
				-- the rank order the page showed.
				collection:addPhotos(photos)
			end
		end
	end, Defaults.catalogWriteAccessOptions)
	if write == "aborted" then
		ErrorHandler.handleError(
			"Could not create the collection",
			"The catalog was busy and the write timed out. Click Show in Lightroom on the page again."
		)
		return
	end
	if not collection then
		ErrorHandler.handleError(
			"Could not create the collection",
			'Lightroom did not create "' .. name .. '" in the "Search Results" collection set.'
		)
		return
	end

	catalog:setActiveSources({ collection })
	LrApplicationView.gridView()
	if missing then
		LrDialogs.message("Some results are not in this catalog", missing, "warning")
	end
	-- Same advice and the same "don't show again" as the Advanced Search
	-- dialog: the SDK cannot set a collection's sort order.
	LrDialogs.messageWithDoNotShow({
		message = string.format('%d photo(s) added to "%s".', #photos, name),
		info = "Set the sort order to 'Custom Order' to see them in the order of the search.",
		actionPrefKey = "LrGeniusAI_AdvancedSearch_SortOrder",
	})
end

--- Turns one action of the page into Lightroom work.
local function runAction(action, scope)
	if type(action) ~= "table" then
		return
	end
	if action.action == "show_search_results" then
		showResults(action)
		return
	end
	if action.action == "refresh_search_context" then
		local ok, err = postContext(scope)
		if not ok then
			ErrorHandler.handleError("Could not update the search page", err)
		end
		return
	end
	-- The backend only queues actions it knows, so this is a page from a
	-- newer build talking to an older plugin.
	ErrorHandler.handleError(
		"Unsupported request from the Search page",
		string.format(
			'This plugin does not know the action "%s". Updating the LrGeniusAI plugin should fix it.',
			tostring(action.action)
		)
	)
end

--- Polls the page's actions until the user cancels the progress bar.
local function pollForActions(scope, pageUrl)
	local startedAt = LrDate.currentTime()
	local everSawPage = false
	local warnedAboutMissingPage = false
	local consecutiveErrors = 0
	local waiting = "Waiting for the Search page in your browser..."

	while not scope:isCanceled() do
		local resp, err = SearchIndexAPI.takeUiActions("search")
		if err then
			consecutiveErrors = consecutiveErrors + 1
			if consecutiveErrors >= MAX_CONSECUTIVE_POLL_ERRORS then
				ErrorHandler.handleError(
					"Lost the connection to the LrGeniusAI backend",
					"Search is no longer listening for the browser page. Close the page and open Search (browser) again. ("
						.. Util.userFacingError(tostring(err))
						.. ")"
				)
				return
			end
		else
			consecutiveErrors = 0
			resp = (type(resp) == "table") and resp or {}
			for _, action in ipairs((type(resp.actions) == "table") and resp.actions or {}) do
				scope:setCaption("Working on a request from the Search page...")
				local ok, actionErr = LrTasks.pcall(runAction, action, scope)
				if not ok then
					ErrorHandler.handleError("Could not carry out the Search request", tostring(actionErr))
				end
				scope:setCaption(waiting)
			end
			if resp.page_open then
				everSawPage = true
			end
			-- The browser never came up (blocked pop-up, no default browser).
			-- Once only: a page in a background tab has its heartbeat
			-- throttled, and asking again later would nag a working page.
			if
				not everSawPage
				and not warnedAboutMissingPage
				and (LrDate.currentTime() - startedAt) > PAGE_OPEN_WARN_SECONDS
			then
				warnedAboutMissingPage = true
				LrDialogs.message(
					"Search did not open in your browser",
					"Open this address manually to continue:\n\n" .. pageUrl,
					"warning"
				)
			end
		end
		LrTasks.sleep(POLL_INTERVAL_SECONDS)
	end
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskSearchBrowser", function(context)
		LrDialogs.attachErrorDialogToFunctionContext(context)
		if not Util.waitForServerDialog({}) then
			return
		end

		local scope = LrProgressScope({
			title = "Search (open in your browser)",
			functionContext = context,
		})

		local ok, err = postContext(scope)
		if not ok then
			scope:done()
			ErrorHandler.handleError("Could not prepare the search page", err)
			return
		end

		local pageUrl = SearchIndexAPI.getSearchUiUrl()
		-- Without the #fragment: it holds the access token.
		log:info("Opening the Search page at " .. (pageUrl:gsub("#.*", "")))
		LrHttp.openUrlInBrowser(pageUrl)
		scope:setCaption("Waiting for the Search page in your browser...")

		local pollOk, pollErr = LrTasks.pcall(pollForActions, scope, pageUrl)
		scope:done()
		if not pollOk then
			ErrorHandler.handleError("Error", tostring(pollErr))
		end
	end)
end)
