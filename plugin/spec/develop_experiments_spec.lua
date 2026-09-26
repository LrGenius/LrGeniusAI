-- Tests for the pure helpers behind TaskDevelopExperiments.
--
-- The experiments exist to answer questions about Lightroom that nothing on
-- disk could settle, so the helpers that build what they apply and read back
-- what Lightroom did have to be right on their own: a wrong scaling or a
-- mutated input would turn a clean experiment into a misleading one.

require("DevelopExperiments")

local X = DevelopExperiments

local function counter()
	local n = 0
	return function()
		n = n + 1
		return string.format("%032X", n)
	end
end

describe("DevelopExperiments.newSyncId", function()
	it("produces 32 upper-case hex digits", function()
		local id = X.newSyncId()
		assert.are.equal(32, #id)
		assert.is_truthy(id:match("^[0-9A-F]+$"))
	end)

	it("uses the injected random source", function()
		local id = X.newSyncId(function()
			return 16
		end)
		assert.are.equal(string.rep("F", 32), id)
	end)
end)

describe("DevelopExperiments.aiMaskCorrection", function()
	it("stores exposure as EV/4 and mirrors Adobe's adaptive-preset shape", function()
		local correction = X.aiMaskCorrection({
			name = "Test",
			mask = X.MASKS.subject,
			exposureStops = 1,
		}, counter())

		assert.are.equal("Correction", correction.What)
		assert.are.equal(0.25, correction.LocalExposure2012)
		assert.are.equal(0, correction.LocalClarity2012)
		assert.are.equal(1, correction.CorrectionAmount)
		assert.are.equal(1, #correction.CorrectionMasks)

		local tool = correction.CorrectionMasks[1]
		assert.are.equal("Mask/Image", tool.What)
		assert.are.equal(1, tool.MaskSubType)
		assert.are.equal("0.500000 0.500000", tool.ReferencePoint)
		assert.are.equal(0, tool.ErrorReason)
		assert.is_nil(tool.MaskSubCategoryID)
	end)

	it("never writes computed fields or runtime ids", function()
		local correction = X.aiMaskCorrection({ name = "Test", mask = X.MASKS.hair }, counter())
		local tool = correction.CorrectionMasks[1]

		for _, key in ipairs({ "MaskDigest", "InputDigest", "FullMaskSize", "WholeImageArea", "Origin", "MaskID" }) do
			assert.is_nil(tool[key], key)
		end
		assert.is_nil(correction.CorrectionID)
		assert.are.equal(3, tool.MaskSubType)
		assert.are.equal(5, tool.MaskSubCategoryID)
	end)

	it("gives correction and tool distinct sync ids unless told otherwise", function()
		local correction = X.aiMaskCorrection({ name = "Test", mask = X.MASKS.sky }, counter())
		assert.are_not.equal(correction.CorrectionSyncID, correction.CorrectionMasks[1].MaskSyncID)

		local fixed = X.aiMaskCorrection({ name = "Test", mask = X.MASKS.sky, syncId = "ABC" }, counter())
		assert.are.equal("ABC", fixed.CorrectionSyncID)
	end)
end)

describe("DevelopExperiments.appendCorrections", function()
	it("keeps existing corrections and does not mutate the input", function()
		local existing = { { CorrectionSyncID = "A" } }
		local combined = X.appendCorrections(existing, { { CorrectionSyncID = "B" } })

		assert.are.equal(2, #combined)
		assert.are.equal(1, #existing)
		assert.are.equal(existing[1], combined[1])
	end)

	it("treats a missing mask array as empty", function()
		assert.are.equal(1, #X.appendCorrections(nil, { { CorrectionSyncID = "B" } }))
	end)
end)

describe("DevelopExperiments mask states", function()
	it("tells computed, failed and pending apart", function()
		assert.are.equal("computed", X.toolState({ What = "Mask/Image", MaskDigest = "F679" }))
		assert.are.equal("failed", X.toolState({ What = "Mask/Image", ErrorReason = 1 }))
		assert.are.equal("failed", X.toolState({ What = "Mask/Image", ErrorReason = "1" }))
		assert.are.equal("pending", X.toolState({ What = "Mask/Image", ErrorReason = 0 }))
		assert.are.equal("geometric", X.toolState({ What = "Mask/Gradient" }))
	end)

	it("reports a correction as pending while any AI tool is pending", function()
		local correction = {
			CorrectionMasks = {
				{ What = "Mask/Image", MaskDigest = "X" },
				{ What = "Mask/Image", ErrorReason = 0 },
			},
		}
		assert.are.equal("pending", X.correctionState(correction))
		assert.are.equal("missing", X.correctionState(nil))
		assert.are.equal("no-ai-mask", X.correctionState({ CorrectionMasks = { { What = "Mask/Gradient" } } }))
	end)

	it("finds corrections by sync id, including duplicates", function()
		local groups = { { CorrectionSyncID = "A" }, { CorrectionSyncID = "B" }, { CorrectionSyncID = "A" } }
		assert.are.equal(2, #X.findCorrections(groups, "A"))
		assert.are.equal(0, #X.findCorrections(nil, "A"))
	end)
end)

describe("DevelopExperiments.diff", function()
	it("distinguishes a dropped key from an unchanged one", function()
		local changes = X.diff({ Temperature = 5500, Tint = 3 }, { Temperature = 7000, Tint = 3 }, {
			"Temperature",
			"Tint",
			"Temp",
		})
		assert.are.same({ Temperature = { before = 5500, after = 7000 } }, changes)
	end)
end)

describe("DevelopExperiments.parseDimensions", function()
	it("reads both the raw table and the formatted string", function()
		assert.are.same({ 6000, 4000 }, { X.parseDimensions({ width = 6000, height = 4000 }) })
		assert.are.same({ 3072, 2304 }, { X.parseDimensions("3072 x 2304") })
		assert.are.same({}, { X.parseDimensions("unknown") })
	end)
end)

describe("DevelopExperiments.cropModelCheck", function()
	it("reproduces an unrotated crop in the sensor frame", function()
		-- A 6000x4000 sensor, crop keeps the middle half horizontally.
		local results = X.cropModelCheck({ w = 6000, h = 4000 }, { w = 3000, h = 4000 }, {
			left = 0.25,
			top = 0,
			right = 0.75,
			bottom = 1,
			angle = 0,
		})
		assert.are.equal("as reported", results[1].dimensions)
		assert.are.equal("as reported", results[1].cropped)
		assert.is_true(results[1].errorPx < 1e-6)
	end)

	it("follows the rotated-rectangle model for an angled crop", function()
		-- Corners of a rectangle 3000x2000 px rotated by +5 degrees, starting at (500, 500).
		local theta = math.rad(5)
		local w, h = 3000, 2000
		local tlx, tly = 500, 500
		local brx = tlx + w * math.cos(theta) - h * math.sin(theta)
		local bry = tly + w * math.sin(theta) + h * math.cos(theta)
		local crop = { left = tlx / 6000, top = tly / 4000, right = brx / 6000, bottom = bry / 4000, angle = 5 }

		local results = X.cropModelCheck({ w = 6000, h = 4000 }, { w = w, h = h }, crop)
		assert.is_true(results[1].errorPx < 1e-6)
		assert.are.equal("as reported", results[1].dimensions)
	end)

	it("detects dimensions reported in display orientation", function()
		-- Sensor 6000x4000, portrait photo: the SDK hands back 4000x6000.
		local results = X.cropModelCheck({ w = 4000, h = 6000 }, { w = 3000, h = 4000 }, {
			left = 0.25,
			top = 0,
			right = 0.75,
			bottom = 1,
			angle = 0,
		})
		assert.are.equal("swapped", results[1].dimensions)
		assert.is_true(results[1].errorPx < 1e-6)

		local text = X.describeCropModel(results, "DA")
		assert.is_truthy(text:find("dimensions are reported in display orientation", 1, true))
	end)

	it("does not claim a frame for an unrotated photo", function()
		local results = X.cropModelCheck({ w = 6000, h = 4000 }, { w = 6000, h = 4000 }, {})
		assert.is_truthy(X.describeCropModel(results, "AB"):find("cannot tell", 1, true))
	end)

	it("says so when nothing matches", function()
		local results = X.cropModelCheck({ w = 6000, h = 4000 }, { w = 1234, h = 777 }, {})
		assert.is_truthy(X.describeCropModel(results, "DA"):find("no hypothesis matches", 1, true))
	end)
end)

describe("DevelopExperiments orientation helpers", function()
	it("maps Lightroom's codes to EXIF numbers", function()
		assert.are.equal(1, X.exifOrientation("AB"))
		assert.are.equal(8, X.exifOrientation("DA"))
		assert.are.equal(6, X.exifOrientation("BC"))
		assert.is_nil(X.exifOrientation("??"))
		assert.is_true(X.isQuarterTurn("DA"))
		assert.is_false(X.isQuarterTurn("CD"))
	end)
end)

describe("DevelopExperiments.versionAtLeast", function()
	it("compares major and minor", function()
		assert.is_true(X.versionAtLeast({ major = 15, minor = 5 }, 15, 3))
		assert.is_false(X.versionAtLeast({ major = 15, minor = 2 }, 15, 3))
		assert.is_true(X.versionAtLeast({ major = 16, minor = 0 }, 15, 3))
		assert.is_false(X.versionAtLeast(nil, 15, 3))
	end)
end)

describe("DevelopExperiments report rendering", function()
	it("turns arbitrary values into plain data", function()
		local plain = X.plainValue({ 1, 2, { a = true }, [false] = "x" })
		assert.are.equal("x", plain["false"])
		assert.are.equal(true, plain["3"].a)

		local list = X.plainValue({ "a", "b" })
		assert.are.same({ "a", "b" }, list)
		assert.are.equal("string", type(X.plainValue(print)))
	end)

	it("renders deterministically and truncates long values", function()
		assert.are.equal('{a=1, b="x"}', X.inlineValue({ b = "x", a = 1 }))
		local long = X.inlineValue(string.rep("y", 50), 10)
		assert.is_truthy(long:find("truncated", 1, true))
	end)

	it("renders verdicts, manual checks and failed steps", function()
		local markdown = X.renderMarkdown({
			meta = { lightroom = "15.5.1" },
			experiments = {
				{
					id = "E1",
					title = "White balance",
					question = "Temp?",
					verdicts = { "Temp was ignored" },
					manualChecks = { "Look at History" },
					steps = { { photo = "a.cr3", label = "E1a", ok = false, error = "boom", data = { x = 1 } } },
				},
			},
		})
		assert.is_truthy(markdown:find("## E1 - White balance", 1, true))
		assert.is_truthy(markdown:find("- Temp was ignored", 1, true))
		assert.is_truthy(markdown:find("- [ ] Look at History", 1, true))
		assert.is_truthy(markdown:find("(FAILED)", 1, true))
		assert.is_truthy(markdown:find("Error: `boom`", 1, true))
	end)
end)

describe("DevelopExperiments crop-model ambiguity", function()
	it("calls an uncropped portrait photo ambiguous instead of guessing a frame", function()
		local results = X.cropModelCheck({ w = 4000, h = 6000 }, { w = 4000, h = 6000 }, {
			left = 0,
			top = 0,
			right = 1,
			bottom = 1,
			angle = 0,
		})
		assert.is_truthy(X.describeCropModel(results, "DA"):find("ambiguous", 1, true))
	end)

	it("calls an aspect-preserving unrotated crop ambiguous", function()
		local results = X.cropModelCheck({ w = 4000, h = 6000 }, { w = 3000, h = 4500 }, {
			left = 0.1,
			top = 0.1,
			right = 0.85,
			bottom = 0.85,
			angle = 0,
		})
		assert.is_truthy(X.describeCropModel(results, "DA"):find("ambiguous", 1, true))
	end)

	it("decides the frame from a straightened portrait crop", function()
		-- Sensor 6000x4000 (landscape), crop rotated by +3 degrees; the SDK is
		-- assumed to report the uncropped size in display orientation.
		local theta = math.rad(3)
		local w, h = 2000, 3000
		local tlx, tly = 1500, 400
		local brx = tlx + w * math.cos(theta) - h * math.sin(theta)
		local bry = tly + w * math.sin(theta) + h * math.cos(theta)
		local crop = { left = tlx / 6000, top = tly / 4000, right = brx / 6000, bottom = bry / 4000, angle = 3 }

		local results = X.cropModelCheck({ w = 4000, h = 6000 }, { w = h, h = w }, crop)
		local text = X.describeCropModel(results, "DA")
		assert.is_truthy(text:find("dimensions are reported in display orientation", 1, true), text)
		assert.is_truthy(text:find("croppedDimensions in display orientation", 1, true), text)
	end)

	it("orders ties deterministically", function()
		local a = X.cropModelCheck({ w = 4000, h = 6000 }, { w = 4000, h = 6000 }, {})
		local b = X.cropModelCheck({ w = 4000, h = 6000 }, { w = 4000, h = 6000 }, {})
		assert.are.equal(a[1].dimensions .. a[1].cropped, b[1].dimensions .. b[1].cropped)
		assert.are.equal("as reported", a[1].dimensions)
	end)
end)

describe("DevelopExperiments.gradientCorrection", function()
	it("builds a linear gradient with its own sync ids", function()
		local correction = X.gradientCorrection({ name = "Seed", exposureStops = -0.5 }, counter())
		local tool = correction.CorrectionMasks[1]

		assert.are.equal("Mask/Gradient", tool.What)
		assert.are.equal(-0.125, correction.LocalExposure2012)
		assert.is_number(tool.ZeroY)
		assert.is_number(tool.FullY)
		assert.are_not.equal(correction.CorrectionSyncID, tool.MaskSyncID)
		assert.are.equal("no-ai-mask", X.correctionState(correction))
	end)
end)

describe("DevelopExperiments.describePoll", function()
	it("words 'nothing found' as computed, not as a rejection", function()
		local text = X.describePoll({
			state = "failed",
			seconds = 4,
			timedOut = false,
			summary = { { tools = { { errorReason = 1 } } } },
		})
		assert.are.equal("computed, nothing found (ErrorReason=1) after 4 s", text)
	end)

	it("distinguishes timeouts, cancellations and success", function()
		assert.are.equal(
			"still pending after 60 s",
			X.describePoll({ state = "pending", seconds = 60, timedOut = true })
		)
		assert.are.equal(
			"not finished (run canceled after 3 s)",
			X.describePoll({ state = "canceled", seconds = 3, timedOut = false })
		)
		assert.are.equal("computed after 7 s", X.describePoll({ state = "computed", seconds = 7, timedOut = false }))
		assert.are.equal("not polled", X.describePoll(nil))
	end)
end)

describe("DevelopExperiments report details", function()
	it("lists paths in the header verbatim, without quoting or truncation", function()
		local long = string.rep("a", 300)
		local markdown = X.renderMarkdown({
			meta = { pluginPresetFilesToDelete = { "C:\\Presets\\one.lrtemplate", long } },
			experiments = {},
		})
		assert.is_truthy(markdown:find("  - C:\\Presets\\one.lrtemplate", 1, true))
		assert.is_truthy(markdown:find("  - " .. long, 1, true))
		assert.is_falsy(markdown:find("truncated", 1, true))
	end)

	it("words an unreadable poll as unknown, not as a Lightroom result", function()
		local text = X.describePoll({ state = "unreadable", seconds = 0, timedOut = false })
		assert.is_truthy(text:find("could not be read back", 1, true))
	end)
end)
