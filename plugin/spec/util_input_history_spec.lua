-- Unit tests for the input history behind the editable drop-downs
-- (Util.pushHistory, Util.historyItems).

local Util = require("Util")

describe("Util.pushHistory", function()
	it("puts the new entry first", function()
		assert.are.same({ "dog", "cat" }, Util.pushHistory({ "cat" }, "dog"))
	end)

	it("starts a history from nothing", function()
		assert.are.same({ "dog" }, Util.pushHistory(nil, "dog"))
	end)

	it("moves an earlier copy up instead of listing it twice", function()
		assert.are.same({ "cat", "dog", "bird" }, Util.pushHistory({ "dog", "cat", "bird" }, "cat"))
	end)

	it("matches without regard to case or surrounding space, keeping the newest spelling", function()
		assert.are.same({ "Red Fox", "owl" }, Util.pushHistory({ "red fox", "owl" }, "  Red Fox "))
	end)

	it("does not record a blank entry", function()
		assert.are.same({ "cat" }, Util.pushHistory({ "cat" }, "   "))
		assert.are.same({ "cat" }, Util.pushHistory({ "cat" }, nil))
	end)

	it("keeps only the most recent entries", function()
		assert.are.same({ "d", "a", "b" }, Util.pushHistory({ "a", "b", "c" }, "d", 3))
	end)

	it("keeps fifteen when no length is given", function()
		local long = {}
		for i = 1, 30 do
			long[i] = "term " .. i
		end
		local result = Util.pushHistory(long, "new")
		assert.are.equal(15, #result)
		assert.are.equal("new", result[1])
		assert.are.equal("term 14", result[15])
	end)

	-- LrPrefs only saves a table that is assigned, so the stored one must not be
	-- edited in place and then relied on.
	it("returns a new table and leaves the stored one alone", function()
		local stored = { "cat" }
		local result = Util.pushHistory(stored, "dog")
		assert.are_not.equal(stored, result)
		assert.are.same({ "cat" }, stored)
	end)

	it("skips damaged entries rather than failing", function()
		assert.are.same({ "dog", "cat" }, Util.pushHistory({ 42, "cat", {} }, "dog"))
		assert.are.same({ "dog" }, Util.pushHistory("not a list", "dog"))
	end)
end)

describe("Util.historyItems", function()
	it("offers the history in order", function()
		assert.are.same({ "dog", "cat" }, Util.historyItems({ "dog", "cat" }))
	end)

	it("adds the extras the history does not already hold, in the history's spelling", function()
		assert.are.same(
			{ "Wildlife", "lrgeniusai", "Ollama" },
			Util.historyItems({ "Wildlife", "lrgeniusai" }, { "LrGeniusAI", "Ollama", "" })
		)
	end)

	it("is not capped, unlike the history it is built from", function()
		local long = {}
		for i = 1, 20 do
			long[i] = "term " .. i
		end
		assert.are.equal(20, #Util.historyItems(long))
	end)

	it("is empty when nothing has been stored", function()
		assert.are.same({}, Util.historyItems(nil, nil))
	end)
end)
