-- Unit tests for the ranking options of Advanced Search (Util.lua): what the
-- dialog's sort, minimum-aesthetic and genre controls turn into, and how a
-- ranked search names its collection.
--
-- Run from the repo root with:  busted

local Util = require("Util")

describe("Util.minAestheticFromSlider", function()
	it("is off at the bottom of the slider", function()
		assert.is_nil(Util.minAestheticFromSlider(0))
		assert.is_nil(Util.minAestheticFromSlider(1)) -- 0.5, below anything the model says
		assert.is_nil(Util.minAestheticFromSlider(nil))
		assert.is_nil(Util.minAestheticFromSlider("x"))
	end)

	it("runs 1.0 to 9.0 in half-points", function()
		assert.are.equal(1.0, Util.minAestheticFromSlider(2))
		assert.are.equal(7.0, Util.minAestheticFromSlider(14))
		assert.are.equal(7.5, Util.minAestheticFromSlider(15))
		assert.are.equal(9.0, Util.minAestheticFromSlider(18))
	end)

	it("clamps a position above the slider", function()
		assert.are.equal(9.0, Util.minAestheticFromSlider(40))
	end)

	it("labels itself for the dialog", function()
		assert.are.equal("Off", Util.minAestheticLabel(0))
		assert.are.equal("7.5", Util.minAestheticLabel(15))
	end)
end)

describe("Util.searchRankingOptions", function()
	it("keeps relevance for a search with a term unless told otherwise", function()
		assert.are.equal("relevance", Util.searchRankingOptions({ hasTerm = true }).sort)
		assert.are.equal("relevance", Util.searchRankingOptions({ hasTerm = true, sortBy = "nonsense" }).sort)
		assert.are.equal("aesthetic", Util.searchRankingOptions({ hasTerm = true, sortBy = "aesthetic" }).sort)
		assert.are.equal("combined", Util.searchRankingOptions({ hasTerm = true, sortBy = "combined" }).sort)
	end)

	it("orders by aesthetic without a term, whatever the popup says", function()
		-- Relevance is closeness to the term; with none there is nothing to be
		-- close to, and the backend would refuse the request.
		for _, sortBy in ipairs({ "relevance", "combined", "aesthetic", "nonsense" }) do
			assert.are.equal("aesthetic", Util.searchRankingOptions({ hasTerm = false, sortBy = sortBy }).sort)
		end
		assert.are.equal("aesthetic", Util.searchRankingOptions({}).sort)
	end)

	it("sends a minimum and a genre only when they are set", function()
		local none = Util.searchRankingOptions({ hasTerm = true, minAestheticPosition = 0, genre = "all" })
		assert.is_nil(none.minAesthetic)
		assert.is_nil(none.genres)
		local set = Util.searchRankingOptions({ hasTerm = true, minAestheticPosition = 14, genre = "macro" })
		assert.are.equal(7.0, set.minAesthetic)
		assert.are.same({ "macro" }, set.genres)
		assert.is_nil(Util.searchRankingOptions({ hasTerm = true, genre = "" }).genres)
	end)

	it("sends a weight only when combining", function()
		assert.are.equal(0.5, Util.searchRankingOptions({ hasTerm = true, sortBy = "combined" }).aestheticWeight)
		assert.is_nil(Util.searchRankingOptions({ hasTerm = true, sortBy = "aesthetic" }).aestheticWeight)
		assert.is_nil(Util.searchRankingOptions({ hasTerm = false, sortBy = "combined" }).aestheticWeight)
	end)
end)

describe("Util.searchCollectionName", function()
	local time = "2026-10-19T06:00:00"

	it("keeps the old name for a plain search", function()
		assert.are.equal(
			"'dog' @ " .. time,
			Util.searchCollectionName({ term = "dog", sort = "relevance", time = time })
		)
	end)

	it("names a search without a term 'Best N' with its filters", function()
		local name = Util.searchCollectionName({
			term = "",
			maxResults = 50,
			sort = "aesthetic",
			minAesthetic = 7,
			genreLabel = "Landscape",
			time = time,
		})
		assert.are.equal("Best 50 \194\183 \226\137\1657.0 \194\183 Landscape @ " .. time, name)
	end)

	it("says how a search with a term was ordered, when it was", function()
		assert.are.equal(
			"'dog' \194\183 by aesthetic @ " .. time,
			Util.searchCollectionName({ term = "dog", sort = "aesthetic", time = time })
		)
		assert.are.equal(
			"'dog' \194\183 relevance + aesthetic \194\183 Macro @ " .. time,
			Util.searchCollectionName({ term = "dog", sort = "combined", genreLabel = "Macro", time = time })
		)
	end)

	it("treats a blank term as none", function()
		local name = Util.searchCollectionName({ term = "   ", maxResults = 20, time = time })
		assert.are.equal("Best 20 @ " .. time, name)
	end)
end)
