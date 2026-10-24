-- Unit tests for SearchBrowser: what the Search (browser) task hands the
-- page, and how it reads the page's "Show in Lightroom" request.
--
-- Run from the repo root with:  busted

require("SearchBrowser")
_G.Util = require("Util")

describe("SearchBrowser.defaultsFromPrefs", function()
	it("starts where the Advanced Search dialog left off", function()
		local d = SearchBrowser.defaultsFromPrefs({
			searchScope = "selected",
			relevanceStrictness = 70,
			maxResults = 500,
			searchSort = "combined",
			searchMinAestheticPosition = 14,
		})
		assert.are.same({
			scope = "selection",
			relevance_strictness = 70,
			max_results = 500,
			sort = "combined",
			min_aesthetic_position = 14,
		}, d)
	end)

	it("falls back to the dialog's own defaults", function()
		assert.are.same({
			scope = "all",
			relevance_strictness = 50,
			max_results = 300,
			sort = "relevance",
			min_aesthetic_position = 0,
		}, SearchBrowser.defaultsFromPrefs(nil))
		assert.are.equal("relevance", SearchBrowser.defaultsFromPrefs({ searchSort = "best" }).sort)
	end)
end)

describe("SearchBrowser.context", function()
	it("carries the catalog, both scopes with what could not be resolved, and the history", function()
		local context = SearchBrowser.context({
			catalogId = "cat-1",
			viewIds = { "a", "b" },
			viewLabel = "Bavaria 2026",
			viewUnresolved = 1,
			selectionIds = { "a" },
			prefs = { searchHistory = { "heron", "sunset" } },
		})
		assert.are.equal("cat-1", context.catalog_id)
		assert.are.same({ label = "Bavaria 2026", photo_ids = { "a", "b" }, unresolved = 1 }, context.scopes.view)
		assert.are.same({ photo_ids = { "a" }, unresolved = 0 }, context.scopes.selection)
		assert.are.same({ "heron", "sunset" }, context.history)
		assert.are.equal("all", context.defaults.scope)
	end)

	it("sends empty scopes rather than nothing", function()
		local context = SearchBrowser.context({})
		assert.are.same({}, context.scopes.view.photo_ids)
		assert.are.same({}, context.scopes.selection.photo_ids)
		assert.are.same({}, context.history)
	end)
end)

describe("SearchBrowser.viewLabel", function()
	it("names up to two sources and counts the rest", function()
		assert.are.equal("", SearchBrowser.viewLabel({}))
		assert.are.equal("2026-10", SearchBrowser.viewLabel({ "2026-10" }))
		assert.are.equal("A, B", SearchBrowser.viewLabel({ "A", "", "B" }))
		assert.are.equal("A, B and 2 more", SearchBrowser.viewLabel({ "A", "B", "C", "D" }))
	end)
end)

describe("SearchBrowser.resultIds", function()
	it("keeps the rank order and drops repeats and junk", function()
		assert.are.same(
			{ "p3", "p1", "p2" },
			SearchBrowser.resultIds({ photo_ids = { "p3", "p1", "p3", "", 7, "p2" } })
		)
		assert.are.same({}, SearchBrowser.resultIds({}))
		assert.are.same({}, SearchBrowser.resultIds(nil))
	end)
end)

describe("SearchBrowser.collectionNameArgs", function()
	it("names the collection like the Advanced Search dialog does", function()
		local args = SearchBrowser.collectionNameArgs({
			term = "heron",
			sort = "combined",
			min_aesthetic = 6.5,
			genre_label = "Wildlife & animals",
			max_results = 300,
		}, "2026-10-24T10:00:00")
		local name = Util.searchCollectionName(args)
		assert.truthy(name:find("'heron'", 1, true))
		assert.truthy(name:find("relevance + aesthetic", 1, true))
		assert.truthy(name:find("6.5", 1, true))
		assert.truthy(name:find("Wildlife & animals", 1, true))
		assert.truthy(name:find("@ 2026-10-24T10:00:00", 1, true))
	end)

	it("names a search without a term as the best N", function()
		local name = Util.searchCollectionName(SearchBrowser.collectionNameArgs({ max_results = 100 }, "t"))
		assert.truthy(name:find("Best 100", 1, true))
	end)
end)

describe("SearchBrowser.missingMessage", function()
	it("says nothing when every result is in the catalog", function()
		assert.is_nil(SearchBrowser.missingMessage(10, 10, false))
	end)

	it("counts what is missing and says why, and that the cleanup is on it", function()
		local text = SearchBrowser.missingMessage(10, 7, true)
		assert.truthy(text:find("3 of the 10 results", 1, true))
		assert.truthy(text:find("holds 7 photo", 1, true))
		assert.truthy(text:find("background", 1, true))
		assert.is_nil(SearchBrowser.missingMessage(10, 7, false):find("background", 1, true))
	end)
end)
