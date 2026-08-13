-- Unit tests for the catalog-keyword vocabulary helpers in Util.lua.
--
-- The vocabulary sent to the LLM is picked by usage frequency and capped, with
-- the category names merged in. Selection and ordering are pure logic (the
-- catalog walk that produces the counts lives in MetadataManager), so they are
-- tested headless here.
--
-- Run from the repo root with:  busted

local Util = require("Util")

local function entries(pairsList)
	local list = {}
	for _, pair in ipairs(pairsList) do
		table.insert(list, { name = pair[1], count = pair[2] })
	end
	return list
end

describe("Util.rankKeywordsByUsage", function()
	it("keeps the most-used keywords when the list exceeds the limit", function()
		local names = Util.rankKeywordsByUsage(
			entries({
				{ "rare", 1 },
				{ "common", 900 },
				{ "medium", 40 },
			}),
			2
		)
		assert.are.same({ "common", "medium" }, names)
	end)

	it("returns the selected names alphabetically, not by count", function()
		-- Stable ordering keeps the backend's cached prompt prefix valid across
		-- runs when the selected set has not changed.
		local names = Util.rankKeywordsByUsage(
			entries({
				{ "zebra", 900 },
				{ "apple", 10 },
			}),
			10
		)
		assert.are.same({ "apple", "zebra" }, names)
	end)

	it("folds duplicate names and sums their counts", function()
		-- The same word under two parents is one term to the model.
		local names = Util.rankKeywordsByUsage(
			entries({
				{ "Berlin", 3 },
				{ "Berlin", 4 },
				{ "Hamburg", 5 },
			}),
			1
		)
		assert.are.same({ "Berlin" }, names)
	end)

	it("treats names differing only in case as one keyword", function()
		local names = Util.rankKeywordsByUsage(
			entries({
				{ "Sunset", 2 },
				{ "sunset", 2 },
			}),
			10
		)
		assert.are.same({ "Sunset" }, names)
	end)

	it("pins category names so the cap cannot drop them", function()
		local names = Util.rankKeywordsByUsage(
			entries({
				{ "popular", 999 },
			}),
			2,
			{ pinned = { "People", "Places" } }
		)
		assert.are.same({ "People", "Places" }, names)
	end)

	it("does not repeat a pinned name that is also a used keyword", function()
		local names = Util.rankKeywordsByUsage(
			entries({
				{ "people", 12 },
				{ "dog", 3 },
			}),
			10,
			{ pinned = { "People" } }
		)
		assert.are.same({ "People", "dog" }, names)
	end)

	it("trims and drops blank names", function()
		local names = Util.rankKeywordsByUsage(
			entries({
				{ "  spaced  ", 5 },
				{ "   ", 5 },
				{ "", 5 },
			}),
			10
		)
		assert.are.same({ "spaced" }, names)
	end)

	it("survives missing, malformed and empty input", function()
		assert.are.same({}, Util.rankKeywordsByUsage(nil, 10))
		assert.are.same({}, Util.rankKeywordsByUsage({}, 10))
		assert.are.same({}, Util.rankKeywordsByUsage({ "not a table", { count = 3 } }, 10))
	end)

	it("defaults to a limit of 500", function()
		local many = {}
		for i = 1, 600 do
			table.insert(many, { name = string.format("kw%04d", i), count = i })
		end
		assert.are.equal(500, #Util.rankKeywordsByUsage(many, nil))
	end)
end)

describe("Util.flattenKeywordCategoryNames", function()
	it("flattens the configured flat category list", function()
		local names = Util.flattenKeywordCategoryNames({ "People", "Places" })
		table.sort(names)
		assert.are.same({ "People", "Places" }, names)
	end)

	it("collects every level of a catalog hierarchy", function()
		-- getCatalogKeywordHierarchy() returns parent -> child tables; the
		-- backend can turn a name at any depth into a category label.
		local names = Util.flattenKeywordCategoryNames({
			WHAT = { Nature = {}, Buildings = {} },
			WHERE = {},
		})
		table.sort(names)
		assert.are.same({ "Buildings", "Nature", "WHAT", "WHERE" }, names)
	end)

	it("returns an empty list for missing or non-table input", function()
		assert.are.same({}, Util.flattenKeywordCategoryNames(nil))
		assert.are.same({}, Util.flattenKeywordCategoryNames("People"))
	end)
end)
