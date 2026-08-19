-- Unit tests for the model-readiness preflight helpers in Util.lua.
--
-- These decide whether a run is about to produce silently degraded data
-- because a model it needs is not downloaded yet. The mapping is pure logic,
-- so it is tested headless here rather than only inside Lightroom.
--
-- Run from the repo root with:  busted

local Util = require("Util")

--- The `families` array as /assets/status reports it, with the given ids unready.
local function families(unready)
	local notReady = {}
	for _, id in ipairs(unready) do
		notReady[id] = true
	end
	return {
		{ id = "clip", name = "Smart photo search", ready = not notReady.clip, approx_bytes = 2310000000 },
		{ id = "bioclip", name = "Species identification", ready = not notReady.bioclip, approx_bytes = 876000000 },
		{ id = "face", name = "Face detection", ready = not notReady.face, approx_bytes = 94200000 },
	}
end

describe("Util.requiredModelFamilies", function()
	it("maps each task to the model the backend loads for it", function()
		assert.are.same({ "clip" }, Util.requiredModelFamilies({ "embeddings" }))
		assert.are.same({ "face" }, Util.requiredModelFamilies({ "faces" }))
		assert.are.same({ "bioclip" }, Util.requiredModelFamilies({ "species" }))
	end)

	it("requires the face model for a cull run", function()
		-- The fast cull ingest still scores face quality, which is the whole
		-- point of face-aware ranking. Missing it does not fail the run, it
		-- just quietly makes the picks worse — exactly what the gate is for.
		assert.are.same({ "face" }, Util.requiredModelFamilies({ "cull" }))
	end)

	it("requires nothing on-device for LLM or cloud tasks", function()
		assert.are.same({}, Util.requiredModelFamilies({ "metadata" }))
		assert.are.same({}, Util.requiredModelFamilies({ "vertexai" }))
		assert.are.same({}, Util.requiredModelFamilies({ "metadata", "vertexai" }))
	end)

	it("reports a stable order regardless of task order", function()
		local expected = { "clip", "face", "bioclip" }
		assert.are.same(expected, Util.requiredModelFamilies({ "embeddings", "faces", "species" }))
		assert.are.same(expected, Util.requiredModelFamilies({ "species", "faces", "embeddings" }))
	end)

	it("does not list a family twice when two tasks need it", function()
		assert.are.same({ "face" }, Util.requiredModelFamilies({ "faces", "cull" }))
	end)

	it("tolerates missing or malformed input", function()
		assert.are.same({}, Util.requiredModelFamilies(nil))
		assert.are.same({}, Util.requiredModelFamilies({}))
		assert.are.same({}, Util.requiredModelFamilies("faces"))
		assert.are.same({}, Util.requiredModelFamilies({ "nonsense" }))
	end)
end)

describe("Util.missingModelFamilies", function()
	it("names the required families that are not ready", function()
		local missing = Util.missingModelFamilies(families({ "face" }), { "clip", "face" })
		assert.are.same({ { id = "face", name = "Face detection", approx_bytes = 94200000 } }, missing)
	end)

	it("ignores an unready family the run does not need", function()
		-- A photographer indexing only for search should not be stopped by the
		-- species model they never asked for.
		assert.are.same({}, Util.missingModelFamilies(families({ "bioclip", "face" }), { "clip" }))
	end)

	it("returns nothing when everything the run needs is ready", function()
		assert.are.same({}, Util.missingModelFamilies(families({}), { "clip", "face", "bioclip" }))
	end)

	it("keeps the required order so the dialog reads the same every run", function()
		local missing =
			Util.missingModelFamilies(families({ "clip", "face", "bioclip" }), { "clip", "face", "bioclip" })
		assert.are.same({ "clip", "face", "bioclip" }, { missing[1].id, missing[2].id, missing[3].id })
	end)

	it("treats a family the backend never reports as present", function()
		-- An older backend that predates a family must not block a run it can
		-- perform perfectly well.
		local older = { { id = "clip", name = "Smart photo search", ready = true } }
		assert.are.same({}, Util.missingModelFamilies(older, { "clip", "bioclip" }))
	end)

	it("falls back to the family id when the backend sends no name", function()
		local unnamed = { { id = "face", ready = false } }
		assert.are.same(
			{ { id = "face", name = "face", approx_bytes = 0 } },
			Util.missingModelFamilies(unnamed, { "face" })
		)
	end)

	it("tolerates missing or malformed input", function()
		assert.are.same({}, Util.missingModelFamilies(nil, { "face" }))
		assert.are.same({}, Util.missingModelFamilies(families({ "face" }), nil))
		assert.are.same({}, Util.missingModelFamilies({ "junk", 7 }, { "face" }))
	end)
end)

describe("Util.formatDownloadSize", function()
	it("reports a small model in MB, not a rounded-away fraction of a GB", function()
		assert.are.equal("94 MB", Util.formatDownloadSize(94200000))
	end)

	it("reports a large model in GB", function()
		assert.are.equal("2.3 GB", Util.formatDownloadSize(2310000000))
	end)

	it("tolerates missing or nonsensical sizes", function()
		assert.are.equal("0 MB", Util.formatDownloadSize(nil))
		assert.are.equal("0 MB", Util.formatDownloadSize(-5))
	end)
end)

describe("Util.searchCoverage", function()
	local stats = { photos = { total = 14070, with_embedding = 9066 } }

	it("counts photos a text search cannot reach", function()
		local coverage = Util.searchCoverage(stats, 14070)
		assert.are.equal(14070, coverage.total)
		assert.are.equal(9066, coverage.searchable)
		assert.are.equal(5004, coverage.unsearchable)
	end)

	it("measures against the catalog, not against what the backend has seen", function()
		-- Photos that were never indexed are absent from the backend's own
		-- total, so using it as the denominator would hide exactly the photos
		-- the warning exists for.
		local coverage = Util.searchCoverage(stats, 20000)
		assert.are.equal(20000, coverage.total)
		assert.are.equal(10934, coverage.unsearchable)
	end)

	it("falls back to the backend total when the catalog count is unknown", function()
		local coverage = Util.searchCoverage(stats, nil)
		assert.are.equal(14070, coverage.total)
		assert.are.equal(5004, coverage.unsearchable)
	end)

	it("reports full coverage when everything is indexed", function()
		local coverage = Util.searchCoverage({ photos = { total = 12, with_embedding = 12 } }, 12)
		assert.are.equal(0, coverage.unsearchable)
	end)

	it("never reports a negative gap after photos are removed", function()
		-- The backend keeps rows for deleted photos, so embeddings can outnumber
		-- the catalog.
		local coverage = Util.searchCoverage(stats, 5000)
		assert.are.equal(5000, coverage.searchable)
		assert.are.equal(0, coverage.unsearchable)
	end)

	it("returns nil when the stats response is unusable", function()
		assert.is_nil(Util.searchCoverage(nil, 100))
		assert.is_nil(Util.searchCoverage({}, 100))
		assert.is_nil(Util.searchCoverage({ photos = {} }, 100))
		assert.is_nil(Util.searchCoverage({ photos = { with_embedding = 5 } }, nil))
	end)
end)
