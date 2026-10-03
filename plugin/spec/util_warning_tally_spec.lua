-- Tests for the per-run warning tally used by TaskAiEditPhotos.
--
-- A run-wide cause ("white balance was not transferred" on every JPEG) used to
-- be listed once per photo, which pushed every other report past the five the
-- dialog shows. The tally keeps each text once with a photo count, and still
-- counts what it does not show.

local Util = require("Util")

describe("Util.responseWarnings", function()
	it("reads the warnings array", function()
		assert.are.same({ "a", "b" }, Util.responseWarnings({ warnings = { "a", "b" }, warning = "a\nb" }))
	end)

	it("splits the legacy joined warning string", function()
		assert.are.same({ "first", "second" }, Util.responseWarnings({ warning = "first\n\n second \r\n" }))
	end)

	it("returns an empty list for anything else", function()
		assert.are.same({}, Util.responseWarnings(nil))
		assert.are.same({}, Util.responseWarnings({}))
		assert.are.same({}, Util.responseWarnings("error"))
	end)
end)

describe("Util warning tally", function()
	it("reports one cause for many photos once, with the photo count", function()
		local tally = Util.newWarningTally()
		for i = 1, 12 do
			Util.tallyWarnings(tally, { "White balance was not transferred" }, "IMG_" .. i .. ".jpg", i)
		end

		assert.are.equal(1, Util.warningTallySize(tally))
		assert.are.same({ "- White balance was not transferred (12 photos)" }, Util.formatWarningTally(tally, 5))
	end)

	it("names the photo when only one is affected", function()
		local tally = Util.newWarningTally()
		Util.tallyWarnings(tally, { "Mask skipped" }, "IMG_7.CR3", 7)

		assert.are.same({ "- Mask skipped (IMG_7.CR3)" }, Util.formatWarningTally(tally))
	end)

	it("counts a photo once per text across repeats and calls", function()
		local tally = Util.newWarningTally()
		Util.tallyWarnings(tally, { "same", "same" }, "a.jpg", 1)
		Util.tallyWarnings(tally, { "same" }, "a.jpg", 1)
		Util.tallyWarnings(tally, { "same" }, "b.jpg", 2)

		assert.are.same({ "- same (2 photos)" }, Util.formatWarningTally(tally))
	end)

	it("keeps photos with the same file name apart by key", function()
		local tally = Util.newWarningTally()
		Util.tallyWarnings(tally, { "w" }, "IMG_0001.CR3", 1)
		Util.tallyWarnings(tally, { "w" }, "IMG_0001.CR3", 2)

		assert.are.same({ "- w (2 photos)" }, Util.formatWarningTally(tally))
	end)

	it("shows a handful in first-seen order and counts the rest", function()
		local tally = Util.newWarningTally()
		for i = 1, 8 do
			Util.tallyWarnings(tally, { "warning " .. i }, "p" .. i, i)
		end

		local lines = Util.formatWarningTally(tally, 5)
		assert.are.equal(6, #lines)
		assert.are.equal("- warning 1 (p1)", lines[1])
		assert.are.equal("- warning 5 (p5)", lines[5])
		assert.is_truthy(lines[6]:find("3 more warnings", 1, true))
	end)

	it("lists a run-wide cause first in a mixed run", function()
		-- 6 raw photos, each with its own one-off note, then 12 JPEGs sharing
		-- the white-balance note: first-seen order would hide the latter.
		local tally = Util.newWarningTally()
		for i = 1, 6 do
			Util.tallyWarnings(tally, { "one-off " .. i }, "RAW_" .. i .. ".CR3", i)
		end
		for i = 7, 18 do
			Util.tallyWarnings(tally, { "White balance was not transferred" }, "IMG_" .. i .. ".jpg", i)
		end

		local lines = Util.formatWarningTally(tally, 5)
		assert.are.equal(6, #lines)
		assert.are.equal("- White balance was not transferred (12 photos)", lines[1])
		assert.are.equal("- one-off 1 (RAW_1.CR3)", lines[2])
		assert.are.equal("- one-off 4 (RAW_4.CR3)", lines[5])
		assert.is_truthy(lines[6]:find("2 more warnings", 1, true))
	end)

	it("keeps first-seen order among equal counts", function()
		local tally = Util.newWarningTally()
		for i = 1, 5 do
			Util.tallyWarnings(tally, { "single " .. i }, "p" .. i, i)
		end
		for i = 6, 9 do
			Util.tallyWarnings(tally, { "shared later", "also shared" }, "p" .. i, i)
		end

		local lines = Util.formatWarningTally(tally, 5)
		assert.are.equal("- shared later (4 photos)", lines[1])
		assert.are.equal("- also shared (4 photos)", lines[2])
		assert.are.equal("- single 1 (p1)", lines[3])
		assert.are.equal("- single 3 (p3)", lines[5])
		assert.is_truthy(lines[6]:find("2 more warnings", 1, true))
	end)

	it("keeps a later one-off visible next to a shared confidence note", function()
		-- 8 photos share the (number-free) confidence note and the JPEG
		-- white-balance note; a 9th photo's distinct warning must still show.
		local tally = Util.newWarningTally()
		for i = 1, 8 do
			Util.tallyWarnings(tally, {
				"Moderate style match confidence. Review the result before applying.",
				"White balance was not transferred",
			}, "IMG_" .. i .. ".jpg", i)
		end
		Util.tallyWarnings(tally, { "Something else" }, "IMG_9.jpg", 9)

		local lines = Util.formatWarningTally(tally, 5)
		assert.are.equal(3, #lines)
		assert.are.equal("- Something else (IMG_9.jpg)", lines[3])
	end)

	it("says 'warning' for a single hidden one", function()
		local tally = Util.newWarningTally()
		Util.tallyWarnings(tally, { "one", "two" }, nil)

		local lines = Util.formatWarningTally(tally, 1)
		assert.are.equal("- one", lines[1])
		assert.is_truthy(lines[2]:find("1 more warning ", 1, true))
	end)

	it("ignores empty input and empty texts", function()
		local tally = Util.newWarningTally()
		Util.tallyWarnings(tally, nil, "a")
		Util.tallyWarnings(tally, { "" }, "a")
		Util.tallyWarnings(nil, { "x" }, "a")

		assert.are.equal(0, Util.warningTallySize(tally))
		assert.are.same({}, Util.formatWarningTally(tally))
	end)
end)
