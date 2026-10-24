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
-- @return table `{ scope, relevance_strictness, max_results, sort, min_aesthetic_position }`
--
function SearchBrowser.defaultsFromPrefs(p)
	p = p or {}
	local sort = p.searchSort
	if sort ~= "relevance" and sort ~= "aesthetic" and sort ~= "combined" then
		sort = "relevance"
	end
	return {
		scope = SCOPE_FROM_PREF[p.searchScope] or "all",
		relevance_strictness = tonumber(p.relevanceStrictness) or 50,
		max_results = tonumber(p.maxResults) or 300,
		sort = sort,
		min_aesthetic_position = tonumber(p.searchMinAestheticPosition) or 0,
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
-- The context the task posts for the page (`/v1/ui/search/context`).
--
-- @param args table `{ catalogId, viewIds, viewLabel, viewUnresolved,
--   selectionIds, selectionUnresolved, prefs }`.
-- @return table
--
function SearchBrowser.context(args)
	args = args or {}
	local p = args.prefs or {}
	return {
		catalog_id = args.catalogId,
		scopes = {
			view = {
				label = args.viewLabel or "",
				photo_ids = args.viewIds or {},
				unresolved = args.viewUnresolved or 0,
			},
			selection = {
				photo_ids = args.selectionIds or {},
				unresolved = args.selectionUnresolved or 0,
			},
		},
		defaults = SearchBrowser.defaultsFromPrefs(p),
		history = type(p.searchHistory) == "table" and p.searchHistory or {},
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
-- The line for results the page showed that this catalog cannot: photos the
-- index still has for this catalog but Lightroom no longer does (deleted ones
-- the index cleanup has not caught up with yet). nil when there are none.
--
-- @param requested number Results the page sent.
-- @param found number Photos found in the catalog.
-- @param cleanupStarted boolean Whether the cleanup was just started.
-- @return string|nil
--
function SearchBrowser.missingMessage(requested, found, cleanupStarted)
	local missing = (tonumber(requested) or 0) - (tonumber(found) or 0)
	if missing <= 0 then
		return nil
	end
	local text = string.format(
		"%d of the %d results are not in this catalog any more, so the collection holds %d photo(s). "
			.. "They were deleted in Lightroom after they were indexed.",
		missing,
		requested,
		found
	)
	if cleanupStarted then
		text = text .. " They are being taken out of the search index now, in the background."
	end
	return text
end

return SearchBrowser
