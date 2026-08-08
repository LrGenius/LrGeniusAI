-- Unit tests for the grouped-indexing helpers in Util.lua.
--
-- Grouping photos into one request is only safe for the in-process llama.cpp
-- backend, and a partly-failed group has to be unpicked per photo. Both
-- decisions are pure logic, so they are tested headless here rather than only
-- inside Lightroom.
--
-- Run from the repo root with:  busted

local Util = require("Util")

describe("Util.groupedBatchSize", function()
	it("groups photos for llamacpp when originals go by reference", function()
		assert.are.equal(4, Util.groupedBatchSize("llamacpp", true, true, nil))
	end)

	it("honours an explicit override", function()
		assert.are.equal(2, Util.groupedBatchSize("llamacpp", true, true, 2))
		assert.are.equal(8, Util.groupedBatchSize("llamacpp", true, true, "8"))
	end)

	it("never groups for a remote provider", function()
		-- Remote providers are billed and rate-limited per request and share
		-- no prompt prefix, so a group would only make failures coarser.
		for _, provider in ipairs({ "chatgpt", "gemini", "ollama", "lmstudio" }) do
			assert.are.equal(1, Util.groupedBatchSize(provider, true, true, 8))
		end
	end)

	it("does not group when the export path is in use", function()
		-- The export path hands the server one temp JPEG at a time.
		assert.are.equal(1, Util.groupedBatchSize("llamacpp", false, true, 8))
	end)

	it("does not group when the backend cannot read local files", function()
		assert.are.equal(1, Util.groupedBatchSize("llamacpp", true, false, 8))
	end)

	it("is case-insensitive about the provider name", function()
		assert.are.equal(4, Util.groupedBatchSize("LlamaCpp", true, true, nil))
	end)

	it("falls back to the default for nonsense sizes", function()
		-- An unusable override means "unset", the same way the server treats a
		-- bad llm_batch_size. Zero especially must not become a batch of none.
		assert.are.equal(4, Util.groupedBatchSize("llamacpp", true, true, 0))
		assert.are.equal(4, Util.groupedBatchSize("llamacpp", true, true, -3))
		assert.are.equal(4, Util.groupedBatchSize("llamacpp", true, true, "lots"))
	end)

	it("truncates a fractional size", function()
		assert.are.equal(3, Util.groupedBatchSize("llamacpp", true, true, 3.7))
	end)

	it("returns 1 when no provider is selected", function()
		assert.are.equal(1, Util.groupedBatchSize(nil, true, true, 8))
	end)
end)

describe("Util.resultsByPhotoId", function()
	it("indexes successes and failures by photo id", function()
		local byId = Util.resultsByPhotoId({
			{ photo_id = "a", success = true },
			{ photo_id = "b", success = false, error = "boom" },
		})
		assert.is_true(byId.a.success)
		assert.is_false(byId.b.success)
		assert.are.equal("boom", byId.b.error)
	end)

	it("reports an unmentioned photo as absent, not as a success", function()
		-- A photo missing from results never got far enough to be judged;
		-- treating absence as success would silently skip its retry.
		local byId = Util.resultsByPhotoId({ { photo_id = "a", success = true } })
		assert.is_nil(byId.z)
	end)

	it("treats a missing success flag as failure", function()
		local byId = Util.resultsByPhotoId({ { photo_id = "a" } })
		assert.is_false(byId.a.success)
	end)

	it("skips malformed entries", function()
		local byId = Util.resultsByPhotoId({ "nope", { no_id = true }, { photo_id = "a", success = true } })
		assert.is_true(byId.a.success)
	end)

	it("returns an empty map for nil or non-table input", function()
		assert.are.same({}, Util.resultsByPhotoId(nil))
		assert.are.same({}, Util.resultsByPhotoId("nope"))
	end)
end)
