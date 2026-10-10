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

describe("DevelopExperiments.settingsFamily", function()
	it("reads the family from the white-balance keys, not the file format", function()
		assert.are.equal("raw", X.settingsFamily({ Temperature = 5500, Tint = 0 }))
		assert.are.equal("non-raw", X.settingsFamily({ IncrementalTemperature = 0, IncrementalTint = 0 }))
		assert.is_nil(X.settingsFamily({ Exposure2012 = 0 }))
		assert.is_nil(X.settingsFamily(nil))
	end)
end)

describe("DevelopExperiments.describeWbModeOutcome", function()
	it("reports a recomputed raw temperature and tint with before and after", function()
		local text = X.describeWbModeOutcome(
			"Daylight",
			"raw",
			{ WhiteBalance = "As Shot", Temperature = 4300, Tint = 2 },
			{ WhiteBalance = "Daylight", Temperature = 5500, Tint = 10 }
		)
		assert.are.equal("WhiteBalance = Daylight; Lightroom recomputed Temperature 4300 -> 5500, Tint 2 -> 10", text)
	end)

	it("says so when the values stay put", function()
		local text = X.describeWbModeOutcome(
			"Auto",
			"non-raw",
			{ WhiteBalance = "As Shot", IncrementalTemperature = 0, IncrementalTint = 0 },
			{ WhiteBalance = "Auto", IncrementalTemperature = 0, IncrementalTint = 0 }
		)
		assert.are.equal(
			"WhiteBalance = Auto; IncrementalTemperature/IncrementalTint not recomputed (still 0 / 0)",
			text
		)
	end)

	it("flags a mode that was not taken and a copy already on that mode", function()
		local text = X.describeWbModeOutcome(
			"Auto",
			"raw",
			{ WhiteBalance = "Auto", Temperature = 5000, Tint = 0 },
			{ WhiteBalance = "As Shot", Temperature = 5000, Tint = 0 }
		)
		assert.is_truthy(text:find("WhiteBalance was not taken (reads As Shot)", 1, true), text)
		assert.is_truthy(text:find("so this proves nothing", 1, true), text)
	end)

	it("notices a value resolved only after the call returned", function()
		local text = X.describeWbModeOutcome(
			"Auto",
			"raw",
			{ WhiteBalance = "As Shot", Temperature = 4300, Tint = 2 },
			{ WhiteBalance = "Auto", Temperature = 4800, Tint = 5 },
			{
				immediate = { WhiteBalance = "Auto", Temperature = 4300, Tint = 2 },
				settle = { seconds = 3.5, changed = true },
			}
		)
		assert.is_truthy(text:find("Tint 2 -> 5 after 3.5 s", 1, true), text)
		assert.is_truthy(text:find("still showed Temperature = 4300", 1, true), text)
		assert.is_truthy(text:find("asynchronously", 1, true), text)
	end)

	it("words a wait without a change as bounded, not final", function()
		local text = X.describeWbModeOutcome(
			"Auto",
			"raw",
			{ WhiteBalance = "Custom", Temperature = 3000, Tint = 40 },
			{ WhiteBalance = "Auto", Temperature = 3000, Tint = 40 },
			{ settle = { seconds = 15, changed = false } }
		)
		assert.are.equal("WhiteBalance = Auto; Temperature/Tint not recomputed within 15 s (still 3000 / 40)", text)
		local canceled = X.describeWbModeOutcome(
			"Auto",
			"raw",
			{ WhiteBalance = "Custom", Temperature = 3000, Tint = 40 },
			{ WhiteBalance = "Auto", Temperature = 3000, Tint = 40 },
			{ settle = { seconds = 2, changed = false, canceled = true } }
		)
		assert.is_truthy(canceled:find("within 2 s (run canceled)", 1, true), canceled)
	end)

	it("says when the values were already in the first readback", function()
		local text = X.describeWbModeOutcome(
			"Daylight",
			"raw",
			{ WhiteBalance = "Custom", Temperature = 3000, Tint = 40 },
			{ WhiteBalance = "Daylight", Temperature = 5500, Tint = 10 },
			{ settle = { seconds = 0, changed = true } }
		)
		assert.is_truthy(text:find("(already in the readback right after the call)", 1, true), text)
	end)

	it("calls a flattened Auto flattened, not rejected", function()
		local before = { WhiteBalance = "Custom", Temperature = 3000, Tint = 40 }
		local after = { WhiteBalance = "Custom", Temperature = 4850, Tint = 4 }
		local text = X.describeWbModeOutcome("Auto", "raw", before, after, { flatten = true })
		assert.are.equal(
			"Auto was flattened to WhiteBalance = Custom; with Temperature 3000 -> 4850, Tint 40 -> 4",
			text
		)
		-- Without the flag, or without a change, it is still "not taken".
		assert.is_truthy(X.describeWbModeOutcome("Auto", "raw", before, after):find("was not taken", 1, true))
		assert.is_truthy(
			X.describeWbModeOutcome("Auto", "raw", before, before, { flatten = true }):find("was not taken", 1, true)
		)
	end)

	it("reports the other family's keys appearing", function()
		local text = X.describeWbModeOutcome(
			"Auto",
			"non-raw",
			{ WhiteBalance = "As Shot", IncrementalTemperature = 0, IncrementalTint = 0 },
			{ WhiteBalance = "Auto", IncrementalTemperature = 0, IncrementalTint = 0, Temperature = 5000 }
		)
		assert.is_truthy(text:find("Temperature appeared (5000)", 1, true), text)
	end)
end)

describe("DevelopExperiments white-balance precondition", function()
	it("is a distinctive Custom white balance in the photo's key family", function()
		assert.are.same({ WhiteBalance = "Custom", Temperature = 3000, Tint = 40 }, X.wbPrecondition("raw"))
		assert.are.same(
			{ WhiteBalance = "Custom", IncrementalTemperature = -40, IncrementalTint = 40 },
			X.wbPrecondition("non-raw")
		)
		-- A fresh table each time, so a caller cannot corrupt the next one.
		local first = X.wbPrecondition("raw")
		first.Temperature = 1
		assert.are.equal(3000, X.wbPrecondition("raw").Temperature)
	end)

	it("checks the readback against what was written", function()
		local pre = X.wbPrecondition("raw")
		assert.is_true(X.preconditionHeld(pre, { WhiteBalance = "Custom", Temperature = 3000, Tint = 40, Temp = 1 }))
		assert.is_false(X.preconditionHeld(pre, { WhiteBalance = "As Shot", Temperature = 3000, Tint = 40 }))
		assert.is_false(X.preconditionHeld(pre, nil))
	end)

	it("detects a change of the family's temperature or tint only", function()
		local before = { WhiteBalance = "Custom", Temperature = 3000, Tint = 40, IncrementalTemperature = 0 }
		assert.is_false(X.wbPairChanged("raw", before, { WhiteBalance = "Auto", Temperature = 3000, Tint = 40 }))
		assert.is_true(X.wbPairChanged("raw", before, { Temperature = 3000, Tint = 41 }))
		assert.is_false(X.wbPairChanged("non-raw", { IncrementalTemperature = -40, IncrementalTint = 40 }, {
			IncrementalTemperature = -40,
			IncrementalTint = 40,
			Temperature = 5000,
		}))
	end)
end)

describe("DevelopExperiments full readback", function()
	local settings = {
		Exposure2012 = 0,
		CameraProfile = "Adobe Standard",
		ConvertToGrayscale = false,
		ToneCurvePV2012 = { 0, 0, 255, 255 },
		LensBlur = {},
		RetouchAreas = { { a = 1 }, { a = 2 } },
		Look = {
			Name = "Adobe Color",
			UUID = "B952C231111CD8E0ECCF14B86BAA7077",
			Amount = 1,
			Parameters = { Version = "1", ToneCurve = { 1, 2 } },
		},
		MaskGroupBasedCorrections = {
			{ CorrectionName = "Sky", CorrectionMasks = { { What = "Mask/Image", MaskDigest = "X" } } },
		},
	}

	it("keeps scalars verbatim and describes tables by shape", function()
		local summary = X.summarizeSettings(settings)
		assert.are.equal(0, summary.Exposure2012)
		assert.are.equal("Adobe Standard", summary.CameraProfile)
		assert.are.equal(false, summary.ConvertToGrayscale)
		assert.are.same(
			{ type = "table", shape = "array", length = 4, keyCount = 4, values = { 0, 0, 255, 255 } },
			summary.ToneCurvePV2012
		)
		assert.are.same({ type = "table", shape = "empty", length = 0, keyCount = 0 }, summary.LensBlur)
		assert.are.equal(2, summary.RetouchAreas.length)
		assert.is_nil(summary.RetouchAreas.values)
	end)

	it("summarises the Look without copying its Parameters", function()
		local summary = X.summarizeSettings(settings)
		local look = summary.Look
		assert.are.equal("Look", look.type)
		assert.are.equal("Adobe Color", look.Name)
		assert.is_true(look.hasParameters)
		assert.are.equal(2, look.parameterKeyCount)
		assert.is_nil(look.Parameters)
		assert.are.same({ "Amount", "Name", "Parameters", "UUID" }, look.keys)
	end)

	it("summarises the masks like the other experiments do", function()
		local masks = X.summarizeSettings(settings).MaskGroupBasedCorrections
		assert.are.equal(1, masks.length)
		assert.are.equal("computed", masks.corrections[1].state)
	end)

	it("describes a missing Look as nil", function()
		assert.is_nil(X.summarizeLook(nil))
		assert.are.same({}, X.summarizeSettings(nil))
	end)
end)

describe("DevelopExperiments Look helpers", function()
	it("picks the first stub whose name differs from the photo's Look", function()
		assert.are.equal("Adobe Vivid", X.pickLook(X.STUB_LOOKS, "Adobe Color").Name)
		assert.are.equal("Adobe Landscape", X.pickLook(X.STUB_LOOKS, "Adobe Vivid").Name)
		assert.are.equal("Adobe Vivid", X.pickLook(X.STUB_LOOKS, nil).Name)
		assert.is_nil(X.pickLook({ { Name = "A" } }, "A"))
	end)

	it("reads names through an accessor for preset entries", function()
		local entries = { { look = { Name = "A" } }, { look = { Name = "B" } } }
		local picked = X.pickLook(entries, "A", function(entry)
			return entry.look.Name
		end)
		assert.are.equal("B", picked.look.Name)
	end)

	it("builds a stub in the preset form, or bare on request", function()
		local stub = X.stubLook(X.STUB_LOOKS[1])
		assert.is_true(stub.Stubbed)
		assert.are.same(
			{ Name = "Adobe Vivid", UUID = "EA1DE074F188405965EF399C72C221D9", Amount = 1, Stubbed = true },
			stub
		)
		assert.are.same(
			{ Name = "Adobe Vivid", UUID = "EA1DE074F188405965EF399C72C221D9", Amount = 1 },
			X.stubLook(X.STUB_LOOKS[1], true)
		)
		assert.is_false(X.isFullLook(stub))
		assert.is_true(X.isFullLook({ Name = "X", Parameters = { Version = 1 } }))
		assert.is_false(X.isFullLook({ Name = "X", Parameters = {} }))
		assert.is_false(X.isFullLook({ Name = "", Parameters = { Version = 1 } }))
	end)
end)

describe("DevelopExperiments E11 sources", function()
	local full = { Name = "Adobe Landscape", UUID = "L", Amount = 1, Parameters = { Version = 1 } }

	it("keeps an Adobe Standard base profile and replaces anything else", function()
		assert.are.equal("Adobe Standard v2", X.e11CameraProfile("Adobe Standard v2"))
		assert.are.equal("Adobe Standard", X.e11CameraProfile("Adobe Standard"))
		assert.are.equal("Adobe Standard", X.e11CameraProfile("Camera Standard v2"))
		assert.are.equal("Adobe Standard", X.e11CameraProfile(nil))
	end)

	it("rejects Looks that cannot serve as the full Look", function()
		assert.is_nil(X.lookRejection(full, "Adobe Color", "R6", "R6"))
		assert.is_truthy(X.lookRejection({ Name = "Adobe Vivid", UUID = "V" }, nil):find("no full Look", 1, true))
		assert.is_truthy(X.lookRejection(full, "Adobe Landscape"):find("same Look", 1, true))
		local adaptive = { Name = "Adaptive Color", Parameters = { Version = 1 }, isAdobeAdaptive = true }
		assert.is_truthy(X.lookRejection(adaptive, nil):find("Adaptive", 1, true))
	end)

	it("accepts a camera-restricted Look only between photos of the same camera", function()
		local restricted = {
			Name = "R6M2 Standard V4",
			Parameters = { Version = 1 },
			CameraModelRestriction = "Canon EOS R6 Mark II",
		}
		-- The restriction uses Adobe's camera name, the photo its EXIF model.
		assert.is_nil(X.lookRejection(restricted, "Adobe Color", "Canon EOS R6m2", "Canon EOS R6m2"))
		assert.is_truthy(
			X.lookRejection(restricted, "Adobe Color", "Canon EOS R6m2", "Canon EOS R5"):find("restricted to", 1, true)
		)
		assert.is_truthy(X.lookRejection(restricted, "Adobe Color", nil, nil):find("restricted to", 1, true))
	end)

	it("picks the first usable donor and lists the ones passed over", function()
		local donors = {
			{ photo = "a.jpg", camera = "X", look = { Name = "", Parameters = {} } },
			{ photo = "b.cr3", camera = "X", look = { Name = "Adobe Color", Parameters = { Version = 1 } } },
			{ photo = "c.cr3", camera = "X", look = full },
		}
		local donor, rejected = X.pickDonorLook(donors, "Adobe Color", "X")
		assert.are.equal("c.cr3", donor.photo)
		assert.are.equal(2, #rejected)
		assert.are.equal("a.jpg", rejected[1].photo)
		assert.is_nil((X.pickDonorLook({}, "Adobe Color", "X")))
	end)

	it("summarises the Look flags that decide whether it can be reused", function()
		local summary = X.summarizeLook({
			Name = "R6M2 Standard V4",
			Stubbed = true,
			isAdobeAdaptive = false,
			CameraModelRestriction = "Canon EOS R6 Mark II",
		})
		assert.is_true(summary.Stubbed)
		assert.is_false(summary.isAdobeAdaptive)
		assert.are.equal("Canon EOS R6 Mark II", summary.CameraModelRestriction)
	end)

	it("reports unreadable presets instead of hiding them in the count", function()
		assert.is_truthy(X.describePresetScan({ scanned = 446 }):find("none of the 446", 1, true))
		local text = X.describePresetScan({ scanned = 10, failedReads = 10, firstError = "Foo: boom" })
		assert.is_truthy(text:find("10 preset(s) could not be read, first error: Foo: boom", 1, true), text)
	end)
end)

describe("DevelopExperiments.describeLookOutcome", function()
	local before = {
		CameraProfile = "Adobe Standard",
		look = { Name = "Adobe Color", UUID = "C", hasParameters = true },
	}
	local stubApplied = {
		CameraProfile = "Adobe Standard",
		look = { Name = "Adobe Vivid", UUID = "V", hasParameters = false },
	}

	it("calls a stub honoured and says whether Lightroom filled in Parameters", function()
		local after =
			{ CameraProfile = "Adobe Standard", look = { Name = "Adobe Vivid", UUID = "V", hasParameters = true } }
		local text = X.describeLookOutcome(stubApplied, before, after, true)
		assert.is_truthy(text:find("^honoured; Lightroom filled in the profile's Parameters"), text)

		local bare =
			{ CameraProfile = "Adobe Standard", look = { Name = "Adobe Vivid", UUID = "V", hasParameters = false } }
		assert.is_truthy(
			X.describeLookOutcome(stubApplied, before, bare, true):find("still has no Parameters", 1, true)
		)
	end)

	it("calls an unchanged Look ignored", function()
		local text = X.describeLookOutcome(stubApplied, before, before, true)
		assert.is_truthy(text:find("^ignored"), text)
	end)

	it("reports a different result, a wrong CameraProfile, errors and unreadable results", function()
		local other = { CameraProfile = "Camera Standard", look = { Name = "Other", UUID = "O", hasParameters = true } }
		local text = X.describeLookOutcome(stubApplied, before, other, true)
		assert.is_truthy(text:find("^changed to something else"), text)
		assert.is_truthy(text:find("CameraProfile reads Camera Standard", 1, true), text)

		assert.are.equal("error: boom", X.describeLookOutcome(stubApplied, before, nil, false, "boom"))
		assert.is_truthy(
			X.describeLookOutcome(stubApplied, before, nil, true, nil, "no read"):find("result is unknown", 1, true)
		)
	end)

	it("checks that a full Look kept its Parameters", function()
		local applied =
			{ CameraProfile = "Adobe Standard", look = { Name = "Vintage", UUID = "W", hasParameters = true } }
		local dropped =
			{ CameraProfile = "Adobe Standard", look = { Name = "Vintage", UUID = "W", hasParameters = false } }
		assert.is_truthy(X.describeLookOutcome(applied, before, dropped, true):find("Parameters dropped", 1, true))
	end)

	it("renders a Look state", function()
		assert.are.equal(
			"CameraProfile Adobe Standard, Look Adobe Color (C, Parameters: yes)",
			X.describeLookState(before)
		)
		assert.are.equal("CameraProfile nil, no Look", X.describeLookState({}))
	end)
end)

describe("DevelopExperiments E2 timing", function()
	it("words an update that led to a ready mask", function()
		local text = X.describeTiming({
			source = "update",
			updateOk = true,
			updateSeconds = 0.43,
			updateCallSeconds = 0.41,
			state = "computed",
			readySeconds = 6.4,
		})
		assert.are.equal("update call 0.4 s, mask ready after 6.4 s", text)
	end)

	it("separates the write-gate wait from the call when they differ", function()
		local text = X.describeTiming({
			source = "update",
			updateOk = true,
			updateSeconds = 3.2,
			updateCallSeconds = 0.4,
			gateWaitSeconds = 2.8,
			state = "failed",
			readySeconds = 9,
		})
		assert.are.equal(
			"update call 0.4 s (after 2.8 s waiting for write access), mask computed (nothing found) after 9.0 s",
			text
		)
	end)

	it("marks a time as a lower bound when Lightroom was already computing", function()
		local base =
			{ source = "update", updateOk = true, updateCallSeconds = 0.4, state = "computed", readySeconds = 0.4 }
		local waited = {}
		for k, v in pairs(base) do
			waited[k] = v
		end
		waited.waitedBeforeUpdate = 12
		assert.is_truthy(
			X.describeTiming(waited):find(
				"lower bound: Lightroom was already computing before the update (the photo was locked for 12.0 s",
				1,
				true
			)
		)
		local locked = {}
		for k, v in pairs(base) do
			locked[k] = v
		end
		locked.lockedDuringAutoWait = true
		assert.is_truthy(X.describeTiming(locked):find("locked during the unasked wait", 1, true))
		assert.is_falsy(X.describeTiming(base):find("lower bound", 1, true))
	end)

	it("does not count a vanished AI tool as a ready mask", function()
		local text = X.describeTiming({
			source = "update",
			updateOk = true,
			updateCallSeconds = 0.4,
			state = "no-ai-mask",
			readySeconds = 3.4,
		})
		assert.are.equal("update call 0.4 s, the AI mask tool disappeared after 3.4 s", text)
		assert.is_false(X.timingIsTerminal({ state = "no-ai-mask" }))
		assert.is_truthy(
			X.describeTiming({ source = "auto", state = "no-ai-mask", readySeconds = 2 }):find("disappeared", 1, true)
		)
		assert.is_truthy(X.describePoll({ state = "no-ai-mask", seconds = 3 }):find("no longer an AI mask", 1, true))
	end)

	it("covers timeouts, cancellations, failures and the automatic case", function()
		assert.are.equal(
			"update call 0.2 s, mask still pending after 60.2 s",
			X.describeTiming({
				source = "update",
				updateOk = true,
				updateSeconds = 0.2,
				state = "pending",
				readySeconds = 60.2,
			})
		)
		assert.is_truthy(X.describeTiming({
			source = "update",
			updateOk = true,
			updateSeconds = 0.2,
			state = "canceled",
			readySeconds = 3,
		}):find("run canceled", 1, true))
		assert.are.equal(
			"update call 0.1 s, and it failed",
			X.describeTiming({ source = "update", updateOk = false, updateSeconds = 0.1 })
		)
		assert.are.equal(
			"without updateAISettings() the mask was ready after 12.0 s",
			X.describeTiming({ source = "auto", state = "computed", readySeconds = 12 })
		)
		assert.are.equal("not measured (canceled)", X.describeTiming({ source = "none", reason = "canceled" }))
	end)

	it("puts every photo on one run-level line", function()
		local line = X.describeTimings({
			{
				photo = "a.cr3",
				source = "update",
				updateOk = true,
				updateSeconds = 0.4,
				state = "computed",
				readySeconds = 5,
			},
			{ photo = "b.jpg", source = "none", reason = "no virtual copy" },
		})
		assert.are.equal(
			"E2 timing per photo - a.cr3: update call 0.4 s, mask ready after 5.0 s (includes loading the AI model if this was the first AI use this session); b.jpg: not measured (no virtual copy)",
			line
		)
		-- The tag goes to the first photo that was measured at all.
		local later = X.describeTimings({
			{ photo = "a", source = "none", reason = "x" },
			{ photo = "b", source = "auto", state = "computed", readySeconds = 1 },
			{ photo = "c", source = "auto", state = "computed", readySeconds = 1 },
		})
		local _, tags = later:gsub("includes loading the AI model", "")
		assert.are.equal(1, tags)
		assert.is_truthy(later:find("b: without updateAISettings() the mask was ready after 1.0 s (includes", 1, true))
		assert.is_truthy(X.describeTimings({}):find("no photo", 1, true))
		assert.is_true(X.timingIsTerminal({ state = "failed" }))
		assert.is_false(X.timingIsTerminal({ state = "pending" }))
	end)
end)

describe("DevelopExperiments.presetFileDeletable", function()
	local NAME = "LrGenius Experiment E4f"
	local DIR = "/Users/me/Library/Application Support/Adobe/CameraRaw/Settings/Plugin Develop Presets"
	local known = { "/Users/me/presets/LrGenius Experiment E4.xmp" }

	it("accepts the preset's own file next to the other experiment files", function()
		assert.is_true((X.presetFileDeletable("/Users/me/presets/" .. NAME .. ".xmp", NAME, known)))
		-- A numbered or uuid-suffixed name is still this preset.
		assert.is_true((X.presetFileDeletable("/Users/me/presets/" .. NAME .. " 2.xmp", NAME, known)))
		assert.is_true((X.presetFileDeletable("/Users/me/presets/" .. NAME .. "-ABC.lrtemplate", NAME, known)))
	end)

	it("accepts a file inside the documented plugin-preset folder without known files", function()
		assert.is_true((X.presetFileDeletable(DIR .. "/" .. NAME .. ".xmp", NAME, {})))
		assert.is_true((X.presetFileDeletable(DIR .. "/com.lrgeniusai/" .. NAME .. ".xmp", NAME, nil)))
	end)

	it("compares Windows paths without regard to case or separator", function()
		local winKnown = { "C:\\Users\\Me\\Presets\\LrGenius Experiment E4.xmp" }
		assert.is_true((X.presetFileDeletable("c:/users/me/presets/" .. NAME .. ".xmp", NAME, winKnown)))
		assert.is_true((X.presetFileDeletable("C:\\Users\\Me\\Presets\\" .. NAME .. ".xmp", NAME, winKnown)))
	end)

	it("refuses anything that is not clearly this preset's file", function()
		local cases = {
			{ nil, "no preset file path" },
			{ "", "no preset file path" },
			{ "error: attempt to call", "no preset file path" },
			{ "presets/" .. NAME .. ".xmp", "not absolute" },
			{ "/Users/me/presets/../presets/" .. NAME .. ".xmp", "relative component" },
			{ "/Users/me/presets/./" .. NAME .. ".xmp", "relative component" },
			{ "/Users/me/presets/" .. NAME, "not a preset file" },
			{ "/Users/me/presets/" .. NAME .. ".lrcat", "not a preset file" },
			{ "/Users/me/presets/" .. NAME .. "x.xmp", "does not match" },
			{ "/Users/me/presets/LrGenius Experiment E4.xmp", "does not match" },
			{ "/Users/me/presets/Other.xmp", "does not match" },
			{ "/Users/me/elsewhere/" .. NAME .. ".xmp", "neither next to" },
			{ "/" .. NAME .. ".xmp", "neither next to" },
		}
		for _, case in ipairs(cases) do
			local ok, why = X.presetFileDeletable(case[1], NAME, known)
			assert.is_false(ok, tostring(case[1]))
			assert.is_truthy(why:find(case[2], 1, true), tostring(case[1]) .. " -> " .. why)
		end
	end)

	it("refuses a path that belongs to another experiment step", function()
		local other = "/Users/me/presets/" .. NAME .. ".xmp"
		local ok, why = X.presetFileDeletable(other, NAME, { other })
		assert.is_false(ok)
		assert.is_truthy(why:find("another experiment step", 1, true))
	end)

	it("refuses without a name to match", function()
		assert.is_false((X.presetFileDeletable("/Users/me/presets/x.xmp", "", known)))
		assert.is_false((X.presetFileDeletable("/Users/me/presets/x.xmp", nil, known)))
	end)

	it("splits both separators", function()
		assert.are.same({ "/a/b", "c.xmp" }, { X.splitPath("/a/b/c.xmp") })
		assert.are.same({ "C:\\a", "c.xmp" }, { X.splitPath("C:\\a\\c.xmp") })
		assert.are.same({ nil, "c.xmp" }, { X.splitPath("c.xmp") })
	end)
end)

describe("DevelopExperiments.describeE4f", function()
	local function base()
		return {
			name = "LrGenius Experiment E4f",
			expectedContrast = 25,
			before = {},
			create = { ok = true },
			apply = {
				ok = true,
				contrast = 25,
				maskFound = true,
				maskState = "computed after 3 s",
				maskPollState = "computed",
			},
			delete = { ok = true, existsAfter = false },
			after = {
				namedCount = 1,
				listedByUuid = true,
				lookup = "found LrGenius Experiment E4f",
				setting = { ok = true, contrast = 25 },
				first = { contrast = 25, maskFound = true, maskState = "computed" },
			},
			second = { ok = true, contrast = 25, maskFound = true, fileBack = false },
			readd = {
				ok = true,
				file = "/p/LrGenius Experiment E4f.xmp",
				fileExists = true,
				samePath = true,
				sameUuid = false,
				contrast = 15,
				expectedContrast = 15,
				fileContrast = 15,
				fileHasNewContrast = true,
				namedCount = 2,
				cleanup = { ok = true, existsAfter = false },
			},
		}
	end

	local function joined(r)
		return table.concat(X.describeE4f(r), "\n")
	end

	it("calls apply-then-delete viable within the session and says what Lightroom still shows", function()
		local lines = X.describeE4f(base())
		assert.are.equal(4, #lines)
		assert.is_truthy(lines[1]:find("no preset named", 1, true))
		assert.is_truthy(lines[2]:find("E4f apply then delete is VIABLE within this session", 1, true))
		assert.is_truthy(lines[2]:find("restart behaviour is a manual check", 1, true))
		assert.is_truthy(lines[2]:find("mask state before the delete: computed after 3 s, after: computed", 1, true))
		assert.is_falsy(lines[2]:find("still pending", 1, true))
		assert.is_truthy(lines[3]:find("getDevelopPresetsForPlugin still lists it: yes", 1, true))
		assert.is_truthy(lines[3]:find("preset:getSetting(): works (Contrast2012 25)", 1, true))
		assert.is_truthy(lines[3]:find("second copy: still works", 1, true))
		assert.is_truthy(lines[3]:find("'Plugin Develop Presets' folder", 1, true))
		assert.is_truthy(lines[4]:find("created a file again", 1, true))
		assert.is_truthy(
			lines[4]:find("holds the new settings (Contrast2012 15, the deleted preset held 25): yes", 1, true)
		)
		assert.is_truthy(lines[4]:find("(getSetting reads 15, the file holds 15)", 1, true))
		assert.is_truthy(lines[4]:find("presets of that name now listed: 2", 1, true))
		assert.is_truthy(lines[4]:find("the re-added file was deleted.", 1, true))
	end)

	it("calls it not viable when the delete changed the applied edit", function()
		local r = base()
		r.after.first = { contrast = 0, maskFound = false, maskState = "missing" }
		local line = X.describeE4f(r)[2]
		assert.is_truthy(line:find("NOT viable", 1, true))
		assert.is_truthy(line:find("Contrast2012 25 -> 0, subject mask present -> missing", 1, true))
		assert.is_truthy(line:find("after: missing", 1, true))
	end)

	it("calls it not viable when a computed mask lost its computation", function()
		local r = base()
		r.after.first.maskState = "pending"
		assert.is_truthy(X.describeE4f(r)[2]:find("NOT viable", 1, true))
	end)

	it("qualifies the verdict when the mask was still pending at the delete", function()
		local r = base()
		r.apply.maskState = "still pending after 60 s"
		r.apply.maskPollState = "pending"
		r.after.first.maskState = "pending"
		local line = X.describeE4f(r)[2]
		assert.is_truthy(line:find("VIABLE within this session", 1, true))
		assert.is_truthy(line:find("still pending, not computed, when the file was deleted", 1, true))
		assert.is_truthy(line:find("covers the settings only", 1, true))
		-- A mask computed with nothing found is computed too.
		r.apply.maskPollState, r.after.first.maskState = "failed", "failed"
		assert.is_falsy(X.describeE4f(r)[2]:find("covers the settings only", 1, true))
	end)

	it("is inconclusive, not viable, when the preset had no visible effect", function()
		local r = base()
		r.apply = { ok = true, contrast = 0, maskFound = false, maskPollState = "missing" }
		r.after.first = { contrast = 0, maskFound = false, maskState = "missing" }
		local text = joined(r)
		assert.is_truthy(
			text:find(
				"E4f inconclusive: the preset had no visible effect on the first copy (Contrast2012 0, subject mask missing), so there was no edit for the delete to preserve.",
				1,
				true
			)
		)
		assert.is_falsy(text:find("VIABLE", 1, true))
		assert.is_falsy(text:find("NOT viable", 1, true))
		assert.is_falsy(text:find("did not arrive", 1, true))

		-- Without a delete to judge, the no-effect note still stands on its own.
		r.delete = { ok = false, error = "permission denied", existsAfter = true }
		r.after, r.second, r.readd = nil, nil, nil
		text = joined(r)
		assert.is_truthy(text:find("E4f the preset had no visible effect on the first copy", 1, true))
		assert.is_falsy(text:find("VIABLE", 1, true))
	end)

	it("judges on the mask alone when only the mask arrived", function()
		local r = base()
		r.apply.contrast = 0
		r.after.first.contrast = 0
		local lines = X.describeE4f(r)
		assert.is_truthy(lines[2]:find("did not arrive on the first copy", 1, true))
		assert.is_truthy(lines[2]:find("rests on the mask alone", 1, true))
		assert.is_truthy(lines[3]:find("VIABLE", 1, true))
		r.after.first.maskFound, r.after.first.maskState = false, "missing"
		assert.is_truthy(X.describeE4f(r)[3]:find("NOT viable", 1, true))
	end)

	it("reports a delete that failed or was refused as inconclusive and stops there", function()
		local r = base()
		r.delete = { ok = false, error = "permission denied", existsAfter = true }
		r.after, r.second, r.readd = nil, nil, nil
		local lines = X.describeE4f(r)
		assert.are.equal(2, #lines)
		assert.is_truthy(
			lines[2]:find("inconclusive: the preset file could not be deleted - permission denied", 1, true)
		)
		assert.is_truthy(lines[2]:find("stays in the list", 1, true))

		r.delete = { skipped = "the path is not absolute: x" }
		assert.is_truthy(X.describeE4f(r)[2]:find("was not deleted - the path is not absolute", 1, true))
		r.delete = { ok = true, existsAfter = true }
		assert.is_truthy(X.describeE4f(r)[2]:find("still there, although LrFileUtils.delete", 1, true))
	end)

	it("describes the deleted preset failing, doing nothing or working partially on a second copy", function()
		local r = base()
		r.second = { ok = false, error = "preset not found" }
		r.after.setting = { ok = false, error = "file missing" }
		r.after.namedCount, r.after.listedByUuid = 0, false
		local line = X.describeE4f(r)[3]
		assert.is_truthy(line:find("still lists it: no (0 preset(s)", 1, true))
		assert.is_truthy(line:find("preset:getSetting(): fails - file missing", 1, true))
		assert.is_truthy(line:find("second copy: fails - preset not found", 1, true))

		r.second = { ok = true, contrast = 0, maskFound = false, fileBack = true }
		line = X.describeE4f(r)[3]
		assert.is_truthy(line:find("has no effect (Contrast2012 0, the preset holds 25", 1, true))
		assert.is_truthy(line:find("wrote the preset file again", 1, true))

		r.second = { ok = true, contrast = 25, maskFound = false }
		line = X.describeE4f(r)[3]
		assert.is_truthy(
			line:find("second copy: works partially (Contrast2012 yes (25), subject mask missing)", 1, true)
		)
		assert.is_falsy(line:find("still works", 1, true))
	end)

	it("sorts the presets listed before the run by whether their file exists", function()
		local r = base()
		r.before = {
			{ uuid = "a", file = "/p/LrGenius Experiment E4f.xmp", fileExists = false },
			{ uuid = "b", file = "/p/LrGenius Experiment E4f.xmp", fileExists = false },
		}
		local lines = X.describeE4f(r)
		assert.is_truthy(lines[1]:find("2 preset(s) named", 1, true))
		assert.is_truthy(
			lines[1]:find("although no file exists at their path (/p/LrGenius Experiment E4f.xmp)", 1, true)
		)
		assert.is_truthy(lines[1]:find("somewhere other than the file", 1, true))
		assert.is_truthy(lines[2]:find("VIABLE", 1, true))

		r.before = { { uuid = "c", file = "/p/LrGenius Experiment E4f.xmp", fileExists = true } }
		local text = joined(r)
		assert.is_truthy(
			text:find(
				'1 preset(s) named "LrGenius Experiment E4f" were listed before this run with their file present',
				1,
				true
			)
		)
		assert.is_truthy(text:find("written back by Lightroom at quit", 1, true))
		assert.is_falsy(text:find("somewhere other than the file", 1, true))

		r.before = { { uuid = "d", file = "error: boom" } }
		assert.is_truthy(X.describeE4f(r)[1]:find("without a readable file path", 1, true))

		r.before = {}
		assert.is_truthy(X.describeE4f(r)[1]:find("forgot the deleted preset at the restart", 1, true))
		r.before = nil
		assert.is_truthy(X.describeE4f(r)[1]:find("VIABLE", 1, true))
	end)

	it("says nothing beyond the point where the run stopped", function()
		local r = base()
		r.before = nil
		r.apply, r.delete, r.after, r.second, r.readd = nil, nil, nil, nil, nil
		assert.are.same({}, X.describeE4f(r))
		r.create = { ok = false, error = "boom" }
		assert.are.same({ "E4f not run: the preset could not be created - boom" }, X.describeE4f(r))
		assert.are.same({}, X.describeE4f(nil))
	end)

	it("notes a canceled run after whatever it got through", function()
		local r = base()
		r.before = nil
		r.delete, r.after, r.second, r.readd = nil, nil, nil, nil
		r.canceled = true
		local lines = X.describeE4f(r)
		assert.are.equal(1, #lines)
		assert.is_truthy(lines[1]:find("E4f not finished: the run was canceled", 1, true))
	end)

	it("says whether the re-added preset holds the new or the stale settings", function()
		local r = base()
		r.readd.contrast, r.readd.fileContrast, r.readd.fileHasNewContrast = 25, 25, false
		local line = X.describeE4f(r)[4]
		assert.is_truthy(line:find("holds the new settings (Contrast2012 15, the deleted preset held 25): no", 1, true))
		assert.is_truthy(line:find("(getSetting reads 25, the file holds 25)", 1, true))

		r.readd.contrast, r.readd.fileContrast, r.readd.fileHasNewContrast = 15, nil, nil
		line = X.describeE4f(r)[4]
		assert.is_truthy(line:find("): yes (getSetting reads 15, no Contrast2012 found in the file)", 1, true))
	end)

	it("keeps a re-added file that could not be deleted in the cleanup list", function()
		local r = base()
		r.readd.cleanup = { skipped = "no match" }
		local line = X.describeE4f(r)[4]
		assert.is_truthy(line:find("was not deleted - no match and stays in the list", 1, true))
		r.readd = { ok = true, file = "/p/x.xmp", fileExists = false, sameUuid = true }
		assert.is_truthy(X.describeE4f(r)[4]:find("but no file exists at its path /p/x.xmp (same uuid: yes)", 1, true))
		r.readd = { ok = false, error = "nope" }
		assert.are.equal("E4f re-adding the same name after the delete failed - nope.", X.describeE4f(r)[4])
	end)
end)

describe("DevelopExperiments.presetFileSetting", function()
	it("reads the XMP attribute, the XMP element and the lrtemplate field", function()
		assert.are.equal(15, X.presetFileSetting('<rdf:Description crs:Contrast2012="+15"', "Contrast2012"))
		assert.are.equal(-8, X.presetFileSetting("<crs:Contrast2012>-8</crs:Contrast2012>", "Contrast2012"))
		assert.are.equal(25, X.presetFileSetting("s = {\n\tContrast2012 = 25,\n}", "Contrast2012"))
	end)

	it("does not match a longer key or a missing one", function()
		assert.is_nil(X.presetFileSetting('crs:LocalContrast2012="+40"', "Contrast2012"))
		assert.are.equal(15, X.presetFileSetting('crs:LocalContrast2012="+40" crs:Contrast2012="+15"', "Contrast2012"))
		assert.is_nil(X.presetFileSetting("", "Contrast2012"))
		assert.is_nil(X.presetFileSetting(nil, "Contrast2012"))
	end)
end)

describe("DevelopExperiments.summaryVerdicts", function()
	it("keeps the E4f answer past the cap and counts only the rest", function()
		local verdicts = {
			"a",
			"b",
			"c",
			"d",
			"e",
			"E4f until Lightroom restarts ...",
			"E4f apply then delete is VIABLE within this session",
			"f",
		}
		local shown, hidden = X.summaryVerdicts(verdicts, 4)
		assert.are.same({ "a", "b", "c", "d", "E4f apply then delete is VIABLE within this session" }, shown)
		assert.are.equal(3, hidden)
	end)

	it("recognises every E4f outcome and nothing else", function()
		assert.is_true(X.isHeadlineVerdict("E4f apply then delete is NOT viable: x"))
		assert.is_true(X.isHeadlineVerdict("E4f inconclusive: x"))
		assert.is_true(X.isHeadlineVerdict("E4f not run: x"))
		assert.is_true(X.isHeadlineVerdict("E4f not finished: the run was canceled."))
		assert.is_false(X.isHeadlineVerdict('E4f no preset named "x" was listed before this run.'))
		assert.is_false(X.isHeadlineVerdict("E4c inconclusive"))
		assert.is_false(X.isHeadlineVerdict(nil))
	end)
end)
