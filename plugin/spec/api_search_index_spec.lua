-- Unit tests for the pure response/summary logic in APISearchIndex.lua.
--
-- That file is the plugin's busiest source of bug fixes and had no test file
-- at all. The three functions under test here are the parts that decide what
-- the user is told: whether a reply counts as success, and which errors and
-- warnings survive into the dialog. Everything around them is HTTP and
-- Lightroom state, which is why these were pulled out to be reachable.

require("APISearchIndex")

describe("SearchIndexAPI.interpretIndexResponse", function()
	it("treats a processed response with successes as success", function()
		local response = { status = "processed", success_count = 1 }
		local ok, value = SearchIndexAPI.interpretIndexResponse(response, nil, "a.jpg")
		assert.is_true(ok)
		-- The whole response comes back, not just a flag: the caller reads
		-- `warnings` off it, and a success here can still be degraded.
		assert.are.equal(response, value)
	end)

	it("passes warnings through on success so the caller can surface them", function()
		local response = {
			status = "processed",
			success_count = 1,
			warnings = { "a.jpg faces: face model is not downloaded yet" },
		}
		local ok, value = SearchIndexAPI.interpretIndexResponse(response, nil, "a.jpg")
		assert.is_true(ok)
		assert.are.same({ "a.jpg faces: face model is not downloaded yet" }, value.warnings)
	end)

	it("fails a processed response that indexed nothing", function()
		local ok, err = SearchIndexAPI.interpretIndexResponse(
			{ status = "processed", success_count = 0, error = "unsupported format" },
			nil,
			"a.jpg"
		)
		assert.is_false(ok)
		assert.are.equal("unsupported format", err)
	end)

	it("falls back to a generic message when a failure carries no error text", function()
		local ok, err = SearchIndexAPI.interpretIndexResponse({ status = "processed", success_count = 0 }, nil, "a.jpg")
		assert.is_false(ok)
		assert.are.equal("Processing failed, and the backend reported no reason.", err)
	end)

	it("falls back when the backend sends an empty error string", function()
		-- "" is truthy in Lua, so `response.error or "..."` handed the empty
		-- string straight through and the user got a dialog with a blank line
		-- where the reason belongs (issue #313).
		local ok, err =
			SearchIndexAPI.interpretIndexResponse({ status = "processed", success_count = 0, error = "" }, nil, "a.jpg")
		assert.is_false(ok)
		assert.are.equal("Processing failed, and the backend reported no reason.", err)
	end)

	it("reports the backend's error when it actually sent one", function()
		local ok, err = SearchIndexAPI.interpretIndexResponse(
			{ status = "processed", success_count = 0, error = "model not loaded" },
			nil,
			"a.jpg"
		)
		assert.is_false(ok)
		assert.are.equal("model not loaded", err)
	end)

	it("treats a missing success_count as zero rather than as success", function()
		local ok = SearchIndexAPI.interpretIndexResponse({ status = "processed" }, nil, "a.jpg")
		assert.is_false(ok)
	end)

	it("reports the transport error when there is no response", function()
		local ok, err = SearchIndexAPI.interpretIndexResponse(nil, "connection refused", "a.jpg")
		assert.is_false(ok)
		assert.are.equal("connection refused", err)
	end)

	it("still fails when there is neither a response nor an error", function()
		local ok, err = SearchIndexAPI.interpretIndexResponse(nil, nil, "a.jpg")
		assert.is_false(ok)
		assert.are.equal("The backend did not respond, and gave no reason.", err)
	end)

	it("still fails, with a reason, when the transport error is an empty string", function()
		local ok, err = SearchIndexAPI.interpretIndexResponse(nil, "", "a.jpg")
		assert.is_false(ok)
		assert.are.equal("The backend did not respond, and gave no reason.", err)
	end)

	it("rejects a non-table body instead of indexing into it", function()
		local ok, err = SearchIndexAPI.interpretIndexResponse("<html>502</html>", nil, "a.jpg")
		assert.is_false(ok)
		assert.is_truthy(tostring(err):find("Invalid response type", 1, true))
	end)

	it("rejects an unexpected status", function()
		local ok, err = SearchIndexAPI.interpretIndexResponse({ status = "queued" }, nil, "a.jpg")
		assert.is_false(ok)
		assert.are.equal("Unexpected response status", err)
	end)
end)

describe("SearchIndexAPI.interpretEditResponse", function()
	it("accepts a success response and returns the whole body", function()
		local response = { status = "success", recipe = { Exposure2012 = 0.3 } }
		local ok, value = SearchIndexAPI.interpretEditResponse(response, nil)
		assert.is_true(ok)
		assert.are.equal(response, value)
	end)

	it("reports the server's error for a non-success status", function()
		local ok, err = SearchIndexAPI.interpretEditResponse({ status = "error", error = "no model" }, nil)
		assert.is_false(ok)
		assert.are.equal("no model", err)
	end)

	it("falls back to a generic message when the error field is absent", function()
		local ok, err = SearchIndexAPI.interpretEditResponse({ status = "error" }, nil)
		assert.is_false(ok)
		assert.are.equal("Unexpected response status", err)
	end)

	it("rejects a non-table body", function()
		local ok, err = SearchIndexAPI.interpretEditResponse("not json", nil)
		assert.is_false(ok)
		assert.is_truthy(tostring(err):find("Invalid response type", 1, true))
	end)

	it("reports the transport error when there is no response", function()
		local ok, err = SearchIndexAPI.interpretEditResponse(nil, "timeout")
		assert.is_false(ok)
		assert.are.equal("timeout", err)
	end)
end)

describe("SearchIndexAPI.condenseMessages", function()
	it("returns nil for nothing to report", function()
		assert.is_nil(SearchIndexAPI.condenseMessages({}))
		assert.is_nil(SearchIndexAPI.condenseMessages(nil))
	end)

	it("collapses a run-wide cause repeated once per photo", function()
		local repeated = {}
		for _ = 1, 40 do
			table.insert(repeated, "face model is not downloaded yet")
		end
		assert.are.equal("face model is not downloaded yet", SearchIndexAPI.condenseMessages(repeated))
	end)

	it("keeps distinct messages in first-seen order", function()
		local got = SearchIndexAPI.condenseMessages({ "b", "a", "b", "c" })
		assert.are.equal("b\na\nc", got)
	end)

	it("counts the remainder instead of dropping it", function()
		local many = {}
		for i = 1, 9 do
			table.insert(many, "problem " .. i)
		end
		local got = SearchIndexAPI.condenseMessages(many)
		local lines = {}
		for line in got:gmatch("[^\n]+") do
			table.insert(lines, line)
		end
		-- Five shown plus one counting line; the four hidden ones are named,
		-- not silently discarded.
		assert.are.equal(6, #lines)
		assert.are.equal("problem 5", lines[5])
		assert.is_truthy(lines[6]:find("4", 1, true))
	end)

	it("does not add a counting line when everything fits", function()
		local got = SearchIndexAPI.condenseMessages({ "one", "two" })
		assert.are.equal("one\ntwo", got)
	end)
end)

describe("SearchIndexAPI.summarizeRun", function()
	it("is a success when nothing failed", function()
		local status = SearchIndexAPI.summarizeRun({ processed = 10, failed = 0 }, {}, {})
		assert.are.equal("success", status)
	end)

	it("is allfailed when every processed photo failed", function()
		local status = SearchIndexAPI.summarizeRun({ processed = 4, failed = 4 }, { "boom" }, {})
		assert.are.equal("allfailed", status)
	end)

	it("is somefailed for a partial failure", function()
		local status = SearchIndexAPI.summarizeRun({ processed = 10, failed = 3 }, { "boom" }, {})
		assert.are.equal("somefailed", status)
	end)

	it("reports warnings even when the run fully succeeded", function()
		-- The degraded-success case: nothing failed, but a signal was lost.
		local status, err, warnings = SearchIndexAPI.summarizeRun(
			{ processed = 3, failed = 0 },
			{},
			{ "a.jpg faces: model missing", "a.jpg faces: model missing", "b.jpg species: model missing" }
		)
		assert.are.equal("success", status)
		assert.is_nil(err)
		assert.are.equal("a.jpg faces: model missing\nb.jpg species: model missing", warnings)
	end)

	it("returns nil messages when there is nothing to say", function()
		local _, err, warnings = SearchIndexAPI.summarizeRun({ processed = 1, failed = 0 }, {}, {})
		assert.is_nil(err)
		assert.is_nil(warnings)
	end)
end)
