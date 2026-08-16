-- Unit tests for progress-caption helpers in Util.lua.
--
-- Covers: formatElapsedTime.
--
-- Run from the repo root with:  busted

local Util = require("Util")

describe("Util.formatElapsedTime", function()
	it("formats sub-minute durations with a zero-padded seconds field", function()
		assert.are.equal("0:00", Util.formatElapsedTime(0))
		assert.are.equal("0:07", Util.formatElapsedTime(7))
		assert.are.equal("0:59", Util.formatElapsedTime(59))
	end)

	it("rolls over into minutes", function()
		assert.are.equal("1:00", Util.formatElapsedTime(60))
		assert.are.equal("2:05", Util.formatElapsedTime(125))
		assert.are.equal("59:59", Util.formatElapsedTime(3599))
	end)

	it("switches to h:mm:ss past an hour", function()
		assert.are.equal("1:00:00", Util.formatElapsedTime(3600))
		assert.are.equal("1:01:01", Util.formatElapsedTime(3661))
		assert.are.equal("10:00:00", Util.formatElapsedTime(36000))
	end)

	it("truncates fractional seconds instead of rounding up", function()
		assert.are.equal("0:09", Util.formatElapsedTime(9.99))
	end)

	it("treats missing, non-numeric and negative input as zero", function()
		assert.are.equal("0:00", Util.formatElapsedTime(nil))
		assert.are.equal("0:00", Util.formatElapsedTime("not a number"))
		assert.are.equal("0:00", Util.formatElapsedTime(-5))
	end)

	it("accepts a numeric string", function()
		assert.are.equal("1:05", Util.formatElapsedTime("65"))
	end)
end)
