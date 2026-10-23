-- The index cleanup's schedule, wording and unattended flow (IndexCleanup.lua).
-- The cleanup itself is the backend's; SearchIndexAPI.syncCleanup is replaced
-- by a double that answers the way the backend does.
require("APISearchIndex")
require("IndexCleanup")

local HOUR = 60 * 60
local DAY = 24 * HOUR

local function iso(time)
	return "t" .. tostring(time)
end

describe("IndexCleanup.isDue", function()
	local now = 1000 * DAY

	it("is due for a catalog that never ran", function()
		assert.is_true(IndexCleanup.isDue(nil, now, false))
		assert.is_true(IndexCleanup.isDue({}, now, true))
	end)

	it("runs once a day, and retries a failed run after an hour", function()
		assert.is_false(IndexCleanup.isDue({ lastAttemptAt = now - 23 * HOUR, lastRunAt = now - 23 * HOUR }, now))
		assert.is_true(IndexCleanup.isDue({ lastAttemptAt = now - 25 * HOUR, lastRunAt = now - 25 * HOUR }, now))
		-- Failed 30 minutes ago, last success two days ago.
		assert.is_false(IndexCleanup.isDue({ lastAttemptAt = now - 30 * 60, lastRunAt = now - 2 * DAY }, now))
		assert.is_true(IndexCleanup.isDue({ lastAttemptAt = now - 2 * HOUR, lastRunAt = now - 2 * DAY }, now))
	end)

	it("runs soon after a search found missing photos, but not twice within minutes", function()
		local state = { lastAttemptAt = now - 30 * 60, lastRunAt = now - 30 * 60 }
		assert.is_true(IndexCleanup.isDue(state, now, true))
		state.lastAttemptAt = now - 5 * 60
		assert.is_false(IndexCleanup.isDue(state, now, true))
	end)
end)

describe("IndexCleanup.summarize", function()
	it("says so when there was nothing to do", function()
		local text = IndexCleanup.summarize({ disassociated = 0, readopted = 0, purged = 0, waiting = 0 }, iso)
		assert.matches("up to date", text, 1, true)
	end)

	it("counts what was removed, what another catalog keeps and when the rest goes for good", function()
		local text = IndexCleanup.summarize({
			disassociated = 12,
			orphaned = 9,
			readopted = 1,
			purged = 4,
			waiting = 10,
			grace_days = 30,
			next_purge_at = 978307200 + 500,
			warnings = {},
		}, iso)
		assert.matches("Removed 12 photos that are no longer in this catalog", text, 1, true)
		assert.matches("3 of them are still in another catalog", text, 1, true)
		assert.matches("1 photo is back in this catalog and found its AI data again", text, 1, true)
		-- The backend's Unix time, shown as Lightroom time.
		assert.matches("10 photos are in no catalog any more. Their AI data is kept for 30 days", text, 1, true)
		assert.matches("(the next on t500)", text, 1, true)
		assert.matches("4 photos that had been in no catalog for more than 30 days were deleted", text, 1, true)
		assert.is_nil(text:find("up to date", 1, true))
	end)

	it("lists the problems the backend reported", function()
		local text = IndexCleanup.summarize({
			disassociated = 1,
			orphaned = 1,
			waiting = 1,
			grace_days = 30,
			warnings = { "3 photos could not be deleted for good (io error)." },
		}, iso)
		assert.matches("Some of it did not work:\n3 photos could not be deleted for good", text, 1, true)
	end)
end)

describe("IndexCleanup.describeState", function()
	it("covers never run, turned off, failed, waiting for an answer and done", function()
		assert.are.equal("Not run for this catalog yet.", IndexCleanup.describeState(nil, true, iso))
		assert.are.equal(
			"Automatic cleanup is off. Not run for this catalog yet.",
			IndexCleanup.describeState({}, false, iso)
		)
		local failed = IndexCleanup.describeState({ lastAttemptAt = 5, error = "Backend not reachable" }, true, iso)
		assert.matches(
			"on t5, did not finish (it is tried again automatically):\nBackend not reachable",
			failed,
			1,
			true
		)
		local held = IndexCleanup.describeState({ pendingConfirmation = 600, pendingIndexed = 1000 }, true, iso)
		assert.matches("Waiting for you: 600 of the 1000 indexed photos", held, 1, true)
		local done = IndexCleanup.describeState(
			{ lastAttemptAt = 9, lastRunAt = 9, released = 1, waiting = 2, nextPurgeAt = 77, warnings = 0 },
			true,
			iso
		)
		assert.are.equal(
			"Last run t9: 1 deleted photo removed.\n2 photos in no catalog, kept for now - deleted for good from t77.",
			done
		)
	end)

	it("keeps each line short enough for the Plug-in Manager, which does not wrap", function()
		local long = IndexCleanup.describeState({ lastAttemptAt = 1, error = string.rep("x", 400) }, true, iso)
		for line in long:gmatch("[^\n]+") do
			assert.is_true(#line <= 95, line)
		end
	end)
end)

describe("IndexCleanup unattended run", function()
	local saved, calls, confirms, bezels, now

	before_each(function()
		saved = {}
		for _, name in ipairs({ "prefs", "LrTasks", "LrDate", "LrDialogs" }) do
			saved[name] = { value = _G[name] }
		end
		saved.syncCleanup = SearchIndexAPI.syncCleanup
		saved.getCatalogIdentifier = Util.getCatalogIdentifier
		calls, confirms, bezels, now = {}, {}, {}, 1000 * DAY
		_G.prefs = { autoIndexCleanup = true }
		_G.LrTasks = {
			pcall = pcall,
			startAsyncTask = function(fn)
				fn()
			end,
		}
		_G.LrDate = {
			currentTime = function()
				return now
			end,
			timeToUserFormat = function(time)
				return "d" .. tostring(time)
			end,
		}
		_G.LrDialogs = {
			confirm = function(title)
				confirms[#confirms + 1] = title
				return "ok"
			end,
			showBezel = function(text)
				bezels[#bezels + 1] = text
			end,
		}
		Util.getCatalogIdentifier = function()
			return "cat_test"
		end
	end)

	after_each(function()
		SearchIndexAPI.syncCleanup = saved.syncCleanup
		Util.getCatalogIdentifier = saved.getCatalogIdentifier
		saved.syncCleanup, saved.getCatalogIdentifier = nil, nil
		for name, original in pairs(saved) do
			_G[name] = original.value
		end
	end)

	local function backend(answers)
		SearchIndexAPI.syncCleanup = function(options)
			calls[#calls + 1] = options
			local answer = table.remove(answers, 1)
			return answer[1], answer[2], answer[3]
		end
	end

	local held = { false, "most of it", { needsConfirmation = true, wouldRemove = 600, indexed = 1000 } }
	local function cleaned()
		return {
			true,
			{ disassociated = 600, orphaned = 600, waiting = 600, next_purge_at = 978307200 + now + 30 * DAY },
		}
	end

	it("asks before removing most of the catalog, then confirms and records the run", function()
		backend({ held, cleaned() })
		assert.is_true(IndexCleanup.requestSoon())
		assert.are.equal(2, #calls)
		assert.is_false(calls[1].confirmLargeRemoval)
		assert.is_true(calls[2].confirmLargeRemoval)
		assert.same({ "Remove 600 photos from the search index?" }, confirms)
		assert.same({ "LrGeniusAI removed 600 deleted photos from the search index" }, bezels)
		local state = prefs.indexCleanupState.cat_test
		assert.are.equal(now, state.lastRunAt)
		assert.are.equal(600, state.released)
		assert.are.equal(now + 30 * DAY, state.nextPurgeAt)
		assert.is_nil(state.pendingConfirmation)
	end)

	it("remembers a 'Not now' for a week instead of asking on every run", function()
		backend({ held })
		_G.LrDialogs.confirm = function(title)
			confirms[#confirms + 1] = title
			return "cancel"
		end
		IndexCleanup.requestSoon()
		assert.are.equal(1, #calls)
		assert.are.equal(600, prefs.indexCleanupState.cat_test.pendingConfirmation)

		-- Within the week the catalog is not even scanned again.
		now = now + DAY + HOUR
		assert.is_false(IndexCleanup.requestSoon())
		assert.are.equal(1, #calls)
		assert.are.equal(1, #confirms, "asked again within the week")

		now = now + 7 * DAY
		backend({ held })
		assert.is_true(IndexCleanup.requestSoon())
		assert.are.equal(2, #calls)
		assert.are.equal(2, #confirms)
	end)

	it("keeps a failure for the status line rather than a dialog", function()
		backend({ { false, "Backend not reachable" } })
		IndexCleanup.requestSoon()
		assert.same({}, confirms)
		assert.same({}, bezels)
		assert.are.equal("Backend not reachable", prefs.indexCleanupState.cat_test.error)
		assert.matches("did not finish", IndexCleanup.statusText(), 1, true)
	end)

	it("shows nothing when there was nothing to clean up", function()
		backend({ { true, { disassociated = 0, readopted = 0, purged = 0, waiting = 0 } } })
		IndexCleanup.requestSoon()
		assert.same({}, bezels)
		assert.are.equal("Last run d" .. tostring(now) .. ": nothing to remove.", IndexCleanup.statusText())
	end)

	it("does nothing when the automatic cleanup is off, or ran minutes ago", function()
		backend({})
		prefs.autoIndexCleanup = false
		assert.is_false(IndexCleanup.requestSoon())
		prefs.autoIndexCleanup = true
		prefs.indexCleanupState = { cat_test = { lastAttemptAt = now - 60 } }
		assert.is_false(IndexCleanup.requestSoon())
		assert.are.equal(0, #calls)
	end)
end)
