-- Regression tests for Util.appendToPrefsList.
--
-- `prefs.knownTopLevelKeywords` used to be extended with
-- `table.insert(prefs.knownTopLevelKeywords, ...)`. LrPrefs saves a key when it
-- is assigned, and the SDK warns it may not notice a change made inside a table
-- already stored under one, so every top-level keyword the user chose was
-- remembered until Lightroom quit and then lost.
--
-- The stand-in below sees exactly what LrPrefs is documented to see: writes to
-- its keys. A change made inside a table it holds goes unrecorded, which is
-- the failure these tests are here to catch.

require("Util")

local function prefsStandIn(initial)
	local stored = initial or {}
	local writes = {}
	-- Left empty so that every assignment reaches __newindex, not only the
	-- first one for each key.
	local store = setmetatable({}, {
		__index = stored,
		__newindex = function(_, key, value)
			table.insert(writes, key)
			stored[key] = value
		end,
	})
	return store, writes
end

describe("Util.appendToPrefsList", function()
	it("assigns the extended list back to its key", function()
		local store, writes = prefsStandIn({ known = { "LrGeniusAI" } })

		assert.is_true(Util.appendToPrefsList(store, "known", "Family"))

		assert.are.same({ "known" }, writes)
		assert.are.same({ "LrGeniusAI", "Family" }, store.known)
	end)

	it("leaves the table it started from alone", function()
		-- Init.lua seeds the preference with Defaults.defaultTopLevelKeywords
		-- itself; adding to it in place edited the defaults too.
		local defaults = { "LrGeniusAI", "Ollama" }
		local store = prefsStandIn({ known = defaults })

		Util.appendToPrefsList(store, "known", "Family")

		assert.are.same({ "LrGeniusAI", "Ollama" }, defaults)
		assert.are_not.equal(defaults, store.known)
	end)

	it("writes nothing when the value is already there", function()
		local store, writes = prefsStandIn({ known = { "LrGeniusAI", "Family" } })

		assert.is_false(Util.appendToPrefsList(store, "known", "Family"))

		assert.are.same({}, writes)
		assert.are.same({ "LrGeniusAI", "Family" }, store.known)
	end)

	it("starts the list when nothing is stored yet", function()
		local store, writes = prefsStandIn()

		assert.is_true(Util.appendToPrefsList(store, "known", "Family"))

		assert.are.same({ "known" }, writes)
		assert.are.same({ "Family" }, store.known)
	end)

	it("replaces a stored value that is not a list", function()
		local store = prefsStandIn({ known = "LrGeniusAI" })

		assert.is_true(Util.appendToPrefsList(store, "known", "Family"))

		assert.are.same({ "Family" }, store.known)
	end)

	it("adds nothing for a nil value", function()
		-- table.insert(list, nil) was a no-op; so is this, and it does not
		-- write the key either.
		local store, writes = prefsStandIn({ known = { "LrGeniusAI" } })

		assert.is_false(Util.appendToPrefsList(store, "known", nil))

		assert.are.same({}, writes)
		assert.are.same({ "LrGeniusAI" }, store.known)
	end)

	it("keeps every addition across several calls", function()
		local store, writes = prefsStandIn({ known = {} })

		Util.appendToPrefsList(store, "known", "Family")
		Util.appendToPrefsList(store, "known", "Travel")

		assert.are.same({ "known", "known" }, writes)
		assert.are.same({ "Family", "Travel" }, store.known)
	end)
end)
