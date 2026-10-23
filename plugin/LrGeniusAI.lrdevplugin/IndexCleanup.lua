-- The index cleanup: takes the photos deleted from a catalog out of its search
-- index. It runs on its own — a couple of minutes after Lightroom starts, then
-- once a day, and soon after a search turned up photos the catalog no longer
-- has — and from Plug-in Manager > Backend Server > Clean up index now.
--
-- The work is the backend's (SearchIndexAPI.syncCleanup and the
-- /v1/photos/catalogs/cleanup route). This module decides when to run, what to
-- ask, and what to tell the user.

IndexCleanup = {}

-- Per catalog id: what the last run did, for the next run and the Plug-in
-- Manager. In the prefs rather than the catalog, so that bookkeeping never
-- waits for catalog write access.
local STATE_PREF = "indexCleanupState"
-- Who owns the background loop. See AgentBridge.lua's OWNER_PREF: a loop left
-- running by the plugin before a "Reload Plug-in" finds a newer owner and exits.
local LOOP_OWNER_PREF = "indexCleanupLoopOwner"

IndexCleanup.STARTUP_DELAY_SECONDS = 120
IndexCleanup.CHECK_INTERVAL_SECONDS = 60 * 60
IndexCleanup.AUTO_INTERVAL_SECONDS = 24 * 60 * 60
IndexCleanup.RETRY_INTERVAL_SECONDS = 60 * 60
IndexCleanup.SOON_INTERVAL_SECONDS = 10 * 60
IndexCleanup.DECLINE_SNOOZE_SECONDS = 7 * 24 * 60 * 60

-- Unix time of Lightroom's epoch (2001-01-01 00:00 GMT): the backend reports
-- Unix seconds, LrDate counts from here.
local LR_EPOCH_UNIX = 978307200

local running = false

local function number(value)
	return tonumber(value) or 0
end

local function count(n, singular, plural)
	return tostring(n) .. " " .. (n == 1 and singular or plural)
end

local function formatDate(time)
	return LrDate.timeToUserFormat(time, "%Y-%m-%d")
end

---
-- Whether an automatic run is due.
-- @param state table|nil this catalog's state
-- @param now number LrDate time
-- @param soon boolean a search just found photos the catalog no longer has
--
function IndexCleanup.isDue(state, now, soon)
	state = type(state) == "table" and state or {}
	-- The user said "Not now" to a large removal: the next run would only meet
	-- the same question, so there is none until the week is up.
	local declinedAt = tonumber(state.declinedAt)
	if
		tonumber(state.pendingConfirmation)
		and declinedAt
		and now - declinedAt < IndexCleanup.DECLINE_SNOOZE_SECONDS
	then
		return false
	end
	local lastAttempt = tonumber(state.lastAttemptAt)
	if soon then
		return lastAttempt == nil or now - lastAttempt >= IndexCleanup.SOON_INTERVAL_SECONDS
	end
	if lastAttempt and now - lastAttempt < IndexCleanup.RETRY_INTERVAL_SECONDS then
		return false
	end
	local lastRun = tonumber(state.lastRunAt)
	return lastRun == nil or now - lastRun >= IndexCleanup.AUTO_INTERVAL_SECONDS
end

---
-- What a finished run did, for the dialog after "Clean up index now".
-- @param result table the backend's answer (see SearchIndexAPI.syncCleanup)
-- @param format function|nil LrDate time -> text
--
function IndexCleanup.summarize(result, format)
	format = format or formatDate
	result = type(result) == "table" and result or {}
	local released = number(result.disassociated)
	local orphaned = number(result.orphaned)
	local readopted = number(result.readopted)
	local purged = number(result.purged)
	local waiting = number(result.waiting)
	local graceDays = number(result.grace_days)

	local paragraphs = {}
	if released == 0 and readopted == 0 and purged == 0 then
		paragraphs[#paragraphs + 1] = "The search index is up to date: every photo in it is still in this catalog."
	end
	if released > 0 then
		local text = "Removed "
			.. count(released, "photo that is", "photos that are")
			.. " no longer in this catalog from its search index."
		local shared = released - orphaned
		if shared > 0 then
			text = text
				.. " "
				.. count(shared, "of them is", "of them are")
				.. " still in another catalog that uses the same index, and stay"
				.. (shared == 1 and "s" or "")
				.. " indexed there."
		end
		paragraphs[#paragraphs + 1] = text
	end
	if readopted > 0 then
		paragraphs[#paragraphs + 1] = count(readopted, "photo is", "photos are")
			.. " back in this catalog and found "
			.. (readopted == 1 and "its" or "their")
			.. " AI data again."
	end
	if waiting > 0 then
		local text = count(waiting, "photo is", "photos are")
			.. " in no catalog any more. "
			.. (waiting == 1 and "Its" or "Their")
			.. " AI data is kept for "
			.. tostring(graceDays)
			.. " days in case "
			.. (waiting == 1 and "it comes" or "they come")
			.. " back, then deleted for good"
		local nextPurge = tonumber(result.next_purge_at)
		if nextPurge then
			text = text .. " (the next on " .. format(nextPurge - LR_EPOCH_UNIX) .. ")"
		end
		paragraphs[#paragraphs + 1] = text .. "."
	end
	if purged > 0 then
		paragraphs[#paragraphs + 1] = count(purged, "photo", "photos")
			.. " that had been in no catalog for more than "
			.. tostring(graceDays)
			.. " days "
			.. (purged == 1 and "was" or "were")
			.. " deleted from the index for good."
	end
	local warnings = SearchIndexAPI.condenseMessages(result.warnings)
	if warnings then
		paragraphs[#paragraphs + 1] = "Some of it did not work:\n" .. warnings
	end
	return table.concat(paragraphs, "\n\n")
end

---
-- The Plug-in Manager's status for this catalog: two short lines at most,
-- since a static_text in the Plug-in Manager does not wrap.
-- @param state table|nil this catalog's state
-- @param enabled boolean whether the automatic cleanup is on
-- @param format function|nil LrDate time -> text
--
function IndexCleanup.describeState(state, enabled, format)
	format = format or formatDate
	state = type(state) == "table" and state or {}
	local prefix = enabled and "" or "Automatic cleanup is off. "
	if tonumber(state.pendingConfirmation) then
		return prefix
			.. "Waiting for you: "
			.. tostring(state.pendingConfirmation)
			.. " of the "
			.. tostring(number(state.pendingIndexed))
			.. " indexed photos are no longer in this catalog.\nClick Clean up index now to review them."
	end
	if state.error and tonumber(state.lastAttemptAt) then
		local reason = tostring(state.error)
		if #reason > 90 then
			reason = reason:sub(1, 87) .. "..."
		end
		return prefix
			.. "The last cleanup, on "
			.. format(state.lastAttemptAt)
			.. ", did not finish"
			.. (enabled and " (it is tried again automatically)" or "")
			.. ":\n"
			.. reason
	end
	if not tonumber(state.lastRunAt) then
		return prefix .. "Not run for this catalog yet."
	end
	local text = prefix .. "Last run " .. format(state.lastRunAt) .. ": "
	local released = number(state.released)
	if released > 0 then
		text = text .. count(released, "deleted photo", "deleted photos") .. " removed"
	else
		text = text .. "nothing to remove"
	end
	local problems = number(state.warnings)
	if problems > 0 then
		text = text .. ", " .. count(problems, "problem", "problems") .. " (run it again for details)"
	end
	text = text .. "."
	local waiting = number(state.waiting)
	if waiting > 0 then
		text = text .. "\n" .. count(waiting, "photo", "photos") .. " in no catalog, kept for now"
		if tonumber(state.nextPurgeAt) then
			text = text .. " - deleted for good from " .. format(state.nextPurgeAt)
		end
		text = text .. "."
	end
	return text
end

local function allStates()
	local states = prefs[STATE_PREF]
	return type(states) == "table" and states or {}
end

local function stateFor(catalogId)
	local state = catalogId and allStates()[catalogId]
	return type(state) == "table" and state or {}
end

local function updateState(catalogId, change)
	if not catalogId then
		return
	end
	local states = Util.deepcopy(allStates())
	local state = type(states[catalogId]) == "table" and states[catalogId] or {}
	change(state)
	states[catalogId] = state
	-- Assigned back: LrPrefs saves a key when it is assigned, not when the
	-- table under it changes.
	prefs[STATE_PREF] = states
end

local function catalogIdentifier()
	local id = Util.getCatalogIdentifier()
	return id
end

--- The Plug-in Manager's status line for the active catalog.
function IndexCleanup.statusText()
	return IndexCleanup.describeState(stateFor(catalogIdentifier()), prefs.autoIndexCleanup ~= false)
end

-- One attempt, recorded in the catalog's state.
-- @return table { kind = "done"|"held"|"failed"|"canceled"|"busy", result, held, message }
local function attempt(confirmLargeRemoval)
	if running then
		return { kind = "busy" }
	end
	local catalogId = catalogIdentifier()
	local now = LrDate.currentTime()
	updateState(catalogId, function(state)
		state.lastAttemptAt = now
	end)
	running = true
	local ok, a, b, c = LrTasks.pcall(SearchIndexAPI.syncCleanup, { confirmLargeRemoval = confirmLargeRemoval })
	running = false
	if not ok then
		a, b, c = false, a, nil
	end
	local success, resultOrErr, held = a, b, c

	if success then
		local result = resultOrErr
		local warnings = type(result.warnings) == "table" and #result.warnings or 0
		updateState(catalogId, function(state)
			state.lastRunAt = now
			state.error = nil
			state.pendingConfirmation = nil
			state.pendingIndexed = nil
			state.declinedAt = nil
			state.released = number(result.disassociated)
			state.readopted = number(result.readopted)
			state.purged = number(result.purged)
			state.waiting = number(result.waiting)
			state.warnings = warnings
			state.nextPurgeAt = tonumber(result.next_purge_at) and (result.next_purge_at - LR_EPOCH_UNIX) or nil
		end)
		return { kind = "done", result = result }
	end
	if type(held) == "table" and held.needsConfirmation then
		updateState(catalogId, function(state)
			state.error = nil
			state.pendingConfirmation = held.wouldRemove
			state.pendingIndexed = held.indexed
		end)
		return { kind = "held", held = held, message = resultOrErr }
	end
	if resultOrErr == "canceled" then
		return { kind = "canceled" }
	end
	local message = Util.userFacingError(tostring(resultOrErr or "Unknown error"))
	updateState(catalogId, function(state)
		state.error = message
	end)
	return { kind = "failed", message = message }
end

-- Asks before releasing more than half of what the catalog has indexed. See
-- CleanupPlan::is_large_removal in the backend for why that is the line.
local function confirmLargeRemoval(held, automatic)
	local wouldRemove = number(held.wouldRemove)
	return LrDialogs.confirm(
		"Remove " .. count(wouldRemove, "photo", "photos") .. " from the search index?",
		tostring(wouldRemove)
			.. " of the "
			.. tostring(number(held.indexed))
			.. " photos indexed for this catalog are no longer in it - more than half"
			.. (automatic and ", so LrGeniusAI asks before cleaning up" or "")
			.. ".\n\nIf you deleted them from Lightroom, remove them from the index. Their AI data is kept for 30 "
			.. "days in case they come back, then deleted for good.\n\nIf you did not, something changed how "
			.. "LrGeniusAI recognises the photos, for example the catalog was restored from an older backup. "
			.. "Keep them, and nothing is removed.",
		"Remove from index",
		automatic and "Not now" or "Keep them"
	)
end

local function bezelText(result)
	local released = number(result.disassociated)
	local readopted = number(result.readopted)
	if released > 0 then
		return "LrGeniusAI removed " .. count(released, "deleted photo", "deleted photos") .. " from the search index"
	end
	if readopted > 0 then
		return "LrGeniusAI found the AI data of "
			.. count(readopted, "photo", "photos")
			.. " that "
			.. (readopted == 1 and "is" or "are")
			.. " back in the catalog"
	end
	return nil
end

-- The unattended run. Nothing here opens a dialog for a run that only failed:
-- the user did not ask for it, and it is tried again within the hour; the
-- error waits in the Plug-in Manager's status line, and anything the user can
-- act on (the backend not running, a photo ID that cannot be read) also
-- surfaces from the first task they start. Purge problems are kept there too.
-- The one question it does ask — releasing most of the catalog — is the user's
-- to answer, so it asks, at most once a week.
local function runAutomatic(soon)
	if prefs.autoIndexCleanup == false then
		return
	end
	local catalogId = catalogIdentifier()
	if not catalogId then
		return
	end
	if not IndexCleanup.isDue(stateFor(catalogId), LrDate.currentTime(), soon) then
		return
	end
	log:info("Index cleanup: automatic run" .. (soon and " (a search found photos the catalog no longer has)" or ""))
	local outcome = attempt(false)
	if outcome.kind == "held" then
		if confirmLargeRemoval(outcome.held, true) ~= "ok" then
			updateState(catalogId, function(state)
				state.declinedAt = LrDate.currentTime()
			end)
			return
		end
		updateState(catalogId, function(state)
			state.declinedAt = nil
		end)
		outcome = attempt(true)
	end
	if outcome.kind == "done" then
		local text = bezelText(outcome.result)
		if text then
			LrDialogs.showBezel(text)
		end
	elseif outcome.kind == "failed" then
		log:warn("Index cleanup: automatic run failed: " .. tostring(outcome.message))
	end
end

---
-- Starts the background schedule. Called from Init.lua, which runs on load and
-- on "Reload Plug-in".
--
function IndexCleanup.start()
	local owner = tostring(LrDate.currentTime())
	prefs[LOOP_OWNER_PREF] = owner
	LrTasks.startAsyncTask(function()
		LrTasks.sleep(IndexCleanup.STARTUP_DELAY_SECONDS)
		while prefs[LOOP_OWNER_PREF] == owner do
			local ok, err = LrTasks.pcall(runAutomatic, false)
			if not ok then
				log:error("Index cleanup: automatic run crashed: " .. tostring(err))
			end
			LrTasks.sleep(IndexCleanup.CHECK_INTERVAL_SECONDS)
		end
	end)
end

---
-- A search or similar-photo lookup found photos the catalog no longer has:
-- deleted photos still in the index. Runs a cleanup in the background unless
-- one ran in the last few minutes.
-- @return boolean whether a cleanup was started
--
function IndexCleanup.requestSoon()
	if prefs.autoIndexCleanup == false or running then
		return false
	end
	if not IndexCleanup.isDue(stateFor(catalogIdentifier()), LrDate.currentTime(), true) then
		return false
	end
	LrTasks.startAsyncTask(function()
		local ok, err = LrTasks.pcall(runAutomatic, true)
		if not ok then
			log:error("Index cleanup: requested run crashed: " .. tostring(err))
		end
	end)
	return true
end

---
-- "Clean up index now" in the Plug-in Manager.
-- @param onFinished function|nil called after the run, to refresh the status line
--
function IndexCleanup.runNow(onFinished)
	LrTasks.startAsyncTask(function()
		local outcome = attempt(false)
		if outcome.kind == "held" then
			local catalogId = catalogIdentifier()
			local choice = confirmLargeRemoval(outcome.held, false)
			-- A "Keep them" holds for the automatic run too, which then does not
			-- ask the same question again for a week.
			updateState(catalogId, function(state)
				state.declinedAt = choice ~= "ok" and LrDate.currentTime() or nil
			end)
			if choice == "ok" then
				outcome = attempt(true)
			else
				outcome = { kind = "canceled" }
			end
		end
		if onFinished then
			onFinished()
		end
		if outcome.kind == "busy" then
			LrDialogs.message(
				"Clean up index",
				"A cleanup is already running. Its progress is in the activity bar at the top left.",
				"info"
			)
		elseif outcome.kind == "done" then
			local result = outcome.result
			local hasWarnings = type(result.warnings) == "table" and #result.warnings > 0
			LrDialogs.message("Clean up index", IndexCleanup.summarize(result), hasWarnings and "warning" or "info")
		elseif outcome.kind == "failed" then
			ErrorHandler.handleError("The index cleanup did not finish", outcome.message)
		end
	end)
end
