-- SearchBrowser.lua
-- The pure half of TaskSearchBrowser.lua (Library > Plug-in Extras > Search
-- (browser)...): what the task hands the browser search page, and how it reads
-- what the page sends back. Tested headless in
-- plugin/spec/search_browser_spec.lua; the catalog work stays in the task.

SearchBrowser = {}

--- The Advanced Search dialog's scope values, as the page names them.
local SCOPE_FROM_PREF = { all = "all", view = "view", selected = "selection" }

---
-- The page's starting options, from the Advanced Search dialog's saved
-- settings, so both start where the user left off.
--
-- @param p table|nil The plug-in prefs.
-- @return table `{ scope, relevance_strictness, max_results, sort, min_aesthetic_position, genre, metadata }`
--
function SearchBrowser.defaultsFromPrefs(p)
	p = p or {}
	local sort = p.searchSort
	if sort ~= "relevance" and sort ~= "aesthetic" and sort ~= "combined" then
		sort = "relevance"
	end
	local genre = p.searchGenre
	if type(genre) ~= "string" or genre == "" then
		genre = "all"
	end
	return {
		scope = SCOPE_FROM_PREF[p.searchScope] or "all",
		relevance_strictness = tonumber(p.relevanceStrictness) or 50,
		max_results = tonumber(p.maxResults) or 300,
		sort = sort,
		min_aesthetic_position = tonumber(p.searchMinAestheticPosition) or 0,
		-- The page offers it only if the backend still names that genre.
		genre = genre,
		metadata = p.searchInMetadata ~= false,
	}
end

---
-- What the page shows for "Current view": the names of Lightroom's active
-- sources, two at most and a count of the rest.
--
-- @param names table List of source names.
-- @return string "" when there are none.
--
function SearchBrowser.viewLabel(names)
	local shown = {}
	for _, name in ipairs(names or {}) do
		if type(name) == "string" and name ~= "" then
			shown[#shown + 1] = name
		end
	end
	if #shown <= 2 then
		return table.concat(shown, ", ")
	end
	return shown[1] .. ", " .. shown[2] .. string.format(" and %d more", #shown - 2)
end

---
-- Notes for the top of the page about what a search cannot see — the same
-- things the Advanced Search dialog says before it opens: an empty result
-- reads as an empty catalog otherwise.
--
-- @param args table `{ searchModelMissing, coverage }`; `coverage` as
--   `Util.searchCoverage` returns it, or nil when unknown.
-- @return table List of messages, empty when there is nothing to say.
--
function SearchBrowser.coverageNotices(args)
	args = args or {}
	local notices = {}
	if args.searchModelMissing then
		notices[#notices + 1] = "The AI search model is not downloaded yet, so searching by subject finds nothing. "
			.. 'Download it in File > Plug-in Manager > LrGeniusAI, under "Download AI models".'
	end
	local coverage = type(args.coverage) == "table" and args.coverage or nil
	if coverage and (tonumber(coverage.outdated) or 0) > 0 then
		notices[#notices + 1] = string.format(
			"New AI search model: %d of %d photos were indexed with the previous one and cannot be found by a "
				.. 'search yet. Run Library > Plug-in Extras > Analyze & Index Photos with "Enable smart photo '
				.. 'search" again to bring them up to date.',
			coverage.outdated,
			coverage.total
		)
	end
	if coverage and (tonumber(coverage.unindexed) or 0) > 0 then
		notices[#notices + 1] = string.format(
			"%d of %d photos have no search index yet and cannot be found by a search. Run Library > Plug-in "
				.. 'Extras > Analyze & Index Photos with "Enable smart photo search" to include them.',
			coverage.unindexed,
			coverage.total
		)
	end
	return notices
end

---
-- The context the task posts for the page (`/v1/ui/search/context`).
--
-- A view that is "All Photographs" is sent as `whole_catalog` instead of a
-- list of IDs: it is the same search as "All indexed photos", and resolving
-- the ID of every photo in the catalog first would keep the page waiting for
-- minutes.
--
-- @param args table `{ catalogId, viewIds, viewLabel, viewUnresolved,
--   viewWholeCatalog, viewCount, selectionIds, selectionUnresolved, notices,
--   prefs }`.
-- @return table
--
function SearchBrowser.context(args)
	args = args or {}
	local p = args.prefs or {}
	local view = {
		label = args.viewLabel or "",
		photo_ids = args.viewIds or {},
		unresolved = args.viewUnresolved or 0,
	}
	if args.viewWholeCatalog then
		view.whole_catalog = true
		view.photo_ids = {}
		view.count = tonumber(args.viewCount) or 0
	end
	return {
		catalog_id = args.catalogId,
		scopes = {
			view = view,
			selection = {
				photo_ids = args.selectionIds or {},
				unresolved = args.selectionUnresolved or 0,
			},
		},
		defaults = SearchBrowser.defaultsFromPrefs(p),
		history = type(p.searchHistory) == "table" and p.searchHistory or {},
		notices = type(args.notices) == "table" and args.notices or {},
	}
end

---
-- The photo IDs of a `show_search_results` action, in rank order, each once.
--
-- @param action table The action the page queued.
-- @return table List of photo IDs.
--
function SearchBrowser.resultIds(action)
	local ids, seen = {}, {}
	for _, id in ipairs(type(action) == "table" and type(action.photo_ids) == "table" and action.photo_ids or {}) do
		if type(id) == "string" and id ~= "" and not seen[id] then
			seen[id] = true
			ids[#ids + 1] = id
		end
	end
	return ids
end

---
-- The arguments for Util.searchCollectionName, from what the page sent with
-- the results: the collection is named like the Advanced Search dialog's.
--
-- @param action table The `show_search_results` action.
-- @param time string The formatted time.
-- @return table `{ term, maxResults, sort, minAesthetic, genreLabel, time }`
--
function SearchBrowser.collectionNameArgs(action, time)
	action = type(action) == "table" and action or {}
	local function text(value)
		return type(value) == "string" and value or nil
	end
	return {
		term = text(action.term) or "",
		maxResults = tonumber(action.max_results),
		sort = text(action.sort),
		minAesthetic = tonumber(action.min_aesthetic),
		genreLabel = text(action.genre_label),
		time = time,
	}
end

---
-- The line for results the page showed that this catalog cannot: nil when
-- there are none.
--
-- When the search was limited to this catalog, they are photos the index still
-- counts as this catalog's but Lightroom no longer has — deleted ones the
-- index cleanup has not caught up with yet. When it could not be limited (no
-- catalog ID), they can just as well belong to another catalog sharing this
-- backend.
--
-- @param requested number Results the page sent.
-- @param found number Photos found in the catalog.
-- @param cleanupStarted boolean Whether the cleanup was just started.
-- @param scopedToCatalog boolean Whether the search was limited to this catalog.
-- @return string|nil
--
function SearchBrowser.missingMessage(requested, found, cleanupStarted, scopedToCatalog)
	local missing = (tonumber(requested) or 0) - (tonumber(found) or 0)
	if missing <= 0 then
		return nil
	end
	local text = string.format(
		"%d of the %d results are not in this catalog, so the collection holds %d photo(s).",
		missing,
		requested,
		found
	)
	if scopedToCatalog == false then
		return text
			.. " The search could not be limited to this catalog, so they may be photos of another catalog that"
			.. " uses the same LrGeniusAI backend."
	end
	text = text .. " They were most likely deleted from Lightroom after they were indexed."
	if cleanupStarted then
		text = text .. " They are being taken out of the search index now, in the background."
	end
	return text
end

return SearchBrowser
