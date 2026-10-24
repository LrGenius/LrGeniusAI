-- Unit tests for the Quality flags field: when it is written, and what the
-- user is told about photos that cannot have flags yet.
--
-- The rule these protect: a plug-in property write advances the photo's Date
-- Edited, so an unchanged value must never be written again.
--
-- Run from the repo root with:  busted

local Util = require("Util")

describe("Util.qualityFlagsChange", function()
	local measured = function(text)
		return { measured = true, flags = {}, text = text }
	end

	it("writes flags into a field that was never set", function()
		local value, why = Util.qualityFlagsChange(nil, measured("blurred, shadows clipped"))
		assert.are.equal("blurred, shadows clipped", value)
		assert.are.equal("changed", why)
	end)

	it("does not rewrite flags the field already holds", function()
		local value, why = Util.qualityFlagsChange("motion blur", measured("motion blur"))
		assert.is_nil(value)
		assert.are.equal("unchanged", why)
	end)

	it("does not write an empty value into a field that was never set", function()
		-- A clean photo on its first run: writing "" would touch Date Edited
		-- for nothing, on almost every photo of the catalog.
		local value, why = Util.qualityFlagsChange(nil, measured(""))
		assert.is_nil(value)
		assert.are.equal("unchanged", why)
	end)

	it("clears flags that no longer apply", function()
		local value, why = Util.qualityFlagsChange("blurred", measured(""))
		assert.are.equal("", value)
		assert.are.equal("changed", why)
	end)

	it("replaces flags that changed", function()
		assert.are.equal("highlights clipped", (Util.qualityFlagsChange("blurred", measured("highlights clipped"))))
	end)

	it("leaves the field alone for a photo indexed before the signals existed", function()
		-- Whatever an earlier run wrote stays: "not measured" is not "clean".
		local value, why = Util.qualityFlagsChange("blurred", { measured = false })
		assert.is_nil(value)
		assert.are.equal("unmeasured", why)
	end)

	it("leaves the field alone when the backend sent no block at all", function()
		local value, why = Util.qualityFlagsChange("blurred", nil)
		assert.is_nil(value)
		assert.are.equal("absent", why)
	end)

	it("treats a measured block without text as no flags", function()
		assert.are.equal("", (Util.qualityFlagsChange("blurred", { measured = true })))
	end)
end)

describe("Util.qualityFlagsPlan", function()
	it("plans writes only for the photos whose field changes, and counts the rest", function()
		local writes, counts = Util.qualityFlagsPlan({
			{ current = nil, quality = { measured = true, text = "blurred" } },
			{ current = "blurred", quality = { measured = true, text = "blurred" } },
			{ current = nil, quality = { measured = true, text = "" } },
			{ current = "shadows clipped", quality = { measured = true, text = "" } },
			{ current = nil, quality = { measured = false } },
			{ current = nil, quality = nil },
		})
		assert.are.same({ { index = 1, value = "blurred" }, { index = 4, value = "" } }, writes)
		assert.are.same({ written = 2, unchanged = 2, unmeasured = 1, absent = 1, failed = 0 }, counts)
	end)

	it("plans nothing for an empty batch", function()
		local writes, counts = Util.qualityFlagsPlan({})
		assert.are.same({}, writes)
		assert.are.equal(0, counts.written)
	end)
end)

describe("Util.qualityFlagsUnmeasuredMessage", function()
	it("says nothing when every photo was measured", function()
		assert.is_nil(Util.qualityFlagsUnmeasuredMessage(0))
		assert.is_nil(Util.qualityFlagsUnmeasuredMessage(nil))
	end)

	it("counts the photos and says what to run", function()
		local message = Util.qualityFlagsUnmeasuredMessage(12)
		assert.truthy(message:find("12 photo(s)", 1, true))
		assert.truthy(message:find("Analyze & Index Photos", 1, true))
		-- The scope that would quietly skip them is named as the one not to use.
		assert.truthy(message:find("New or unprocessed photos", 1, true))
	end)
end)
