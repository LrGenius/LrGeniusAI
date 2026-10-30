-- Pure helpers behind indexing's batched exports and the Plug-in Manager's
-- face recognition box.

local Util = require("Util")

describe("Util.exportChunkSize", function()
	it("defaults when nothing usable is saved", function()
		assert.are.equal(Util.DEFAULT_EXPORT_CHUNK_SIZE, Util.exportChunkSize(nil))
		assert.are.equal(Util.DEFAULT_EXPORT_CHUNK_SIZE, Util.exportChunkSize("lots"))
		assert.are.equal(Util.DEFAULT_EXPORT_CHUNK_SIZE, Util.exportChunkSize(0 / 0))
	end)

	it("keeps a saved size within bounds", function()
		assert.are.equal(40, Util.exportChunkSize("40"))
		assert.are.equal(7, Util.exportChunkSize(7.9))
		assert.are.equal(1, Util.exportChunkSize(-3))
		assert.are.equal(200, Util.exportChunkSize(5000))
	end)
end)

describe("Util.jpegQualityFromPercent", function()
	it("turns the percent into Lightroom's 0-1 scale", function()
		assert.are.equal(0.5, Util.jpegQualityFromPercent(50))
		assert.are.equal(0.8, Util.jpegQualityFromPercent("80"))
		assert.are.equal(1, Util.jpegQualityFromPercent(100))
	end)

	it("keeps the result within Lightroom's range", function()
		assert.are.equal(0.01, Util.jpegQualityFromPercent(0))
		assert.are.equal(0.01, Util.jpegQualityFromPercent(-20))
		assert.are.equal(1, Util.jpegQualityFromPercent(250))
	end)

	it("uses the default when nothing usable is saved", function()
		assert.are.equal(0.6, Util.jpegQualityFromPercent(nil))
		assert.are.equal(0.5, Util.jpegQualityFromPercent("high", 50))
		assert.are.equal(0.5, Util.jpegQualityFromPercent(0 / 0, 50))
	end)
end)

describe("Util.faceRecognizerSummary", function()
	local state = {
		active = "auraface",
		recognizers = {
			{ id = "facenet", name = "FaceNet", description = "Included." },
			{ id = "auraface", name = "AuraFace", description = "More accurate." },
		},
		warnings = {},
	}

	it("names the recognizer in use", function()
		assert.are.equal("In use: AuraFace. More accurate.", Util.faceRecognizerSummary(state))
	end)

	it("adds the backend's first warning", function()
		state.warnings = { "AuraFace was chosen, but its model file is missing." }
		assert.truthy(Util.faceRecognizerSummary(state):find("model file is missing", 1, true))
	end)

	it("says so when the answer is unusable", function()
		assert.truthy(Util.faceRecognizerSummary(nil):find("Unknown", 1, true))
		assert.truthy(Util.faceRecognizerSummary({ active = "x", recognizers = {} }):find("Unknown", 1, true))
	end)
end)
