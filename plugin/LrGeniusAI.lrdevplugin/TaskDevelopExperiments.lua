--- Developer task: controlled develop-settings experiments.
--
-- The AI Edit XMP research (docs/wiki/Dev-AI-Edit-XMP-Findings.md) settled most
-- questions from files on disk, but four decide the architecture and can only be
-- answered by Lightroom itself:
--
--   E13  In which orientation does the SDK report `dimensions` and
--        `croppedDimensions`, and what does `orientation` look like at runtime?
--        (read-only)
--   E1   Does `applyDevelopSettings` accept the `Temp` key AI Edit writes today,
--        or only `Temperature` / `IncrementalTemperature`?
--   E2   Can `applyDevelopSettings` add a new AI mask (subject, sky, background,
--        people part), and does Lightroom compute it - on its own or after
--        `updateAISettings()`?
--   E4   How do plugin presets behave: same-name re-adds, where the file lives
--        and what it holds, merging with existing masks, re-applying, amount.
--
-- Everything that writes goes to virtual copies named "LrG Exp ...", never to
-- the selected photos themselves. E4 additionally leaves hidden plugin presets
-- behind: they are files in Lightroom's preset folder, shared by every catalog,
-- and the SDK has no call to delete them. The final dialog lists their paths.
--
-- The result is a Markdown report plus the same data as JSON next to it (a
-- numbered name if a .json of that name already exists). Nothing is judged
-- beyond what the readback shows; a few things (history step names, what a mask
-- covers) are listed as manual checks because the SDK cannot read them.

require("DevelopExperiments")

local X = DevelopExperiments

local MAX_PHOTOS = 4
-- The first AI detection in a session loads the model. A short wait here would
-- turn "Lightroom computes on its own, slowly" into "only after
-- updateAISettings()", so the automatic window gets a comparable budget.
local AUTO_COMPUTE_WAIT_SECONDS = 30
local AI_POLL_SECONDS = 60
local AVAILABLE_WAIT_SECONDS = 60
local PRESET_NAME = "LrGenius Experiment E4"
local AMOUNT_PRESET_NAME = "LrGenius Experiment E4 Amount"

local WB_KEYS = { "WhiteBalance", "Temperature", "Tint", "IncrementalTemperature", "IncrementalTint", "Temp" }

local CONTEXT_KEYS = {
	"orientation",
	"CropLeft",
	"CropTop",
	"CropRight",
	"CropBottom",
	"CropAngle",
	"HasCrop",
	"ProcessVersion",
	"CameraProfile",
	"PerspectiveUpright",
	"LensProfileEnable",
	"EnableLensCorrections",
	"EnableMaskGroupBasedCorrections",
	"WhiteBalance",
	"Temperature",
	"Tint",
	"IncrementalTemperature",
	"IncrementalTint",
	"Temp",
	"Exposure2012",
	"Contrast2012",
}

local function round(value, digits)
	local factor = 10 ^ (digits or 1)
	return math.floor(value * factor + 0.5) / factor
end

local function photoLabel(photo)
	local ok, label = LrTasks.pcall(function()
		local name = photo:getFormattedMetadata("fileName") or "?"
		local copyName = photo:getFormattedMetadata("copyName")
		if copyName and copyName ~= "" then
			return name .. " [" .. copyName .. "]"
		end
		return name
	end)
	return ok and label or "?"
end

--- Reads the develop settings. On failure returns an empty table *and* the
-- error, so readbacks stay safe to index while writers can refuse to act on
-- a state they could not read.
local function readSettings(photo)
	local ok, settings = LrTasks.pcall(function()
		return photo:getDevelopSettings()
	end)
	if ok and type(settings) == "table" then
		return settings, nil
	end
	return {}, "could not read develop settings: " .. tostring(settings)
end

local function correctionsOf(photo)
	local settings, err = readSettings(photo)
	local groups = settings.MaskGroupBasedCorrections
	if type(groups) ~= "table" then
		return {}, err
	end
	return groups, err
end

--- Runs `fn` inside a write gate. Returns ok, error text.
--
-- With timeout parameters the SDK does not throw when write access is not
-- granted in time: it returns "aborted" and never runs `fn`. Treating that as
-- success would record "Lightroom ignored the setting" for a call that never
-- happened.
local function withWrite(catalog, actionName, fn)
	local status
	local ok, err = LrTasks.pcall(function()
		status = catalog:withWriteAccessDo(actionName, fn, Defaults.catalogWriteAccessOptions)
	end)
	if not ok then
		return false, tostring(err)
	end
	if status ~= nil and status ~= "executed" then
		return false,
			string.format(
				"Lightroom did not grant catalog write access within %s s (%s), so nothing was applied. Close other plug-in tasks and run the experiment again.",
				tostring(Defaults.catalogWriteAccessOptions.timeout),
				tostring(status)
			)
	end
	return true, nil
end

--- `needsUpdateAISettings` is 15.3+; on older builds the error text is the answer.
local function needsUpdateAI(photo)
	local ok, result = LrTasks.pcall(function()
		return photo:needsUpdateAISettings()
	end)
	if ok then
		return result
	end
	return "unavailable: " .. tostring(result)
end

local function isAvailableForEditing(photo)
	local ok, result = LrTasks.pcall(function()
		return photo:isAvailableForEditing()
	end)
	if ok then
		return result
	end
	return "unavailable"
end

local function originalIsOnline(photo)
	local ok, result = LrTasks.pcall(function()
		return photo:checkPhotoAvailability()
	end)
	return ok and result == true
end

local function currentModule()
	local ok, name = LrTasks.pcall(function()
		return LrApplicationView.getCurrentModuleName()
	end)
	return ok and name or "unknown"
end

--- Waits while an AI job holds the photo (15.3+). Writing to a locked photo
-- would fail or be skipped, and the next readback would blame the experiment.
-- @return boolean, number, string|nil true when the photo is (or cannot be
--   told to be anything but) available; the seconds waited; and when it is
--   not, why: "canceled" or "locked".
local function waitUntilAvailable(photo, progress)
	local started = LrDate.currentTime()
	while true do
		local available = isAvailableForEditing(photo)
		local waited = round(LrDate.currentTime() - started)
		if available ~= false then
			return true, waited, nil
		end
		if progress:isCanceled() then
			return false, waited, "canceled"
		end
		if waited >= AVAILABLE_WAIT_SECONDS then
			return false, waited, "locked"
		end
		LrTasks.sleep(1)
	end
end

-- A step skipped because the user canceled is not a failure of Lightroom;
-- `countFailedSteps` leaves it out.
local CANCELED_MESSAGE = "not run (canceled)"

local function unavailableMessage(waited, why)
	if why == "canceled" then
		return CANCELED_MESSAGE
	end
	return "the photo stayed locked by another AI job for " .. tostring(waited) .. " s"
end

-- Same approach as `createVirtualCopyFor` in TaskAiEditPhotos.lua:
-- `createVirtualCopies` copies whatever is selected, so select exactly this
-- photo first and verify the result before trusting it.
local function selectOnly(catalog, photo)
	local ok = LrTasks.pcall(function()
		catalog:setSelectedPhotos(photo, { photo })
	end)
	if not ok then
		return false
	end
	local selected = catalog:getTargetPhotos()
	return type(selected) == "table" and #selected == 1 and selected[1] == photo
end

local function createVirtualCopy(catalog, photo, copyName)
	if not selectOnly(catalog, photo) then
		local switched = LrTasks.pcall(function()
			catalog:setActiveSources({ catalog.kAllPhotos })
			LrTasks.sleep(0.2)
		end)
		if not switched or not selectOnly(catalog, photo) then
			return nil, "the photo could not be selected in Lightroom"
		end
	end

	local copies
	local ok, err = withWrite(catalog, "LrGenius experiments: create virtual copy", function()
		copies = catalog:createVirtualCopies(copyName)
	end)
	if not ok then
		return nil, err
	end
	if type(copies) ~= "table" or #copies ~= 1 then
		return nil, "Lightroom did not return exactly one virtual copy"
	end
	-- A copy of a virtual copy belongs to the same master, not to the copy.
	local expectedMaster = photo
	if photo:getRawMetadata("isVirtualCopy") then
		expectedMaster = photo:getRawMetadata("masterPhoto")
	end
	if copies[1]:getRawMetadata("masterPhoto") ~= expectedMaster then
		return nil, "Lightroom copied a different photo"
	end
	return copies[1], nil
end

--- Polls until the correction with `syncId` reaches a terminal state
-- ("computed", "failed", "no-ai-mask"), the run is canceled, or
-- `timeoutSeconds` pass. A missing correction is polled too: an applied preset
-- may only show up later. The timeline records every change of mask state,
-- `needsUpdateAISettings` and `isAvailableForEditing`, so a flip during the
-- wait is visible as evidence of an automatic computation.
local function pollCorrection(photo, syncId, timeoutSeconds, progress)
	local started = LrDate.currentTime()
	local timeline = {}
	local last = {}
	local state, matches, elapsed, readErr
	while true do
		local groups
		groups, readErr = correctionsOf(photo)
		matches = X.findCorrections(groups, syncId)
		-- An unreadable state is terminal: polling on would report "still
		-- missing" for a mask that may well be there.
		state = readErr and "unreadable" or X.correctionState(matches[1])
		elapsed = LrDate.currentTime() - started
		local needs = needsUpdateAI(photo)
		local available = isAvailableForEditing(photo)
		if state ~= last.state or needs ~= last.needs or available ~= last.available then
			table.insert(timeline, {
				seconds = round(elapsed),
				state = state,
				needsUpdateAISettings = needs,
				isAvailableForEditing = available,
			})
			last = { state = state, needs = needs, available = available }
		end
		if
			state == "computed"
			or state == "failed"
			or state == "no-ai-mask"
			or state == "unreadable"
			or elapsed >= timeoutSeconds
		then
			break
		end
		if progress:isCanceled() then
			state = "canceled"
			break
		end
		LrTasks.sleep(1)
	end
	return {
		state = state,
		seconds = round(elapsed),
		budgetSeconds = timeoutSeconds,
		timedOut = state ~= "computed"
			and state ~= "failed"
			and state ~= "no-ai-mask"
			and state ~= "canceled"
			and state ~= "unreadable",
		readError = readErr,
		matchingCorrections = #matches,
		timeline = timeline,
		summary = X.summarizeCorrections(matches),
		needsUpdateAISettings = needsUpdateAI(photo),
		isAvailableForEditing = isAvailableForEditing(photo),
	}
end

--- Creates an experiment record and registers it in `sink` (the report's
-- experiment list) right away, so an experiment that throws halfway still
-- shows up in the report with the steps it got through.
local function newExperiment(sink, id, title, question)
	local exp = { id = id, title = title, question = question, verdicts = {}, manualChecks = {}, steps = {} }
	table.insert(sink, exp)
	return exp
end

local function addStep(experiment, photo, label, ok, err, data)
	table.insert(experiment.steps, {
		photo = photo,
		label = label,
		ok = ok and true or false,
		error = err,
		data = X.plainValue(data),
	})
end

local function countFailedSteps(experiments)
	local failed = 0
	for _, experiment in ipairs(experiments) do
		for _, step in ipairs(experiment.steps) do
			if not step.ok and step.error ~= CANCELED_MESSAGE then
				failed = failed + 1
			end
		end
	end
	return failed
end

--- An offline original cannot feed an AI computation, so every mask result on
-- it would read as "Lightroom does not compute". Say so up front.
local function noteIfOffline(experiment, photo, label)
	if not originalIsOnline(photo) then
		table.insert(
			experiment.verdicts,
			label .. ": the original file is offline - AI mask results for this photo are inconclusive"
		)
	end
end

---------------------------------------------------------------------------
-- E13: runtime formats (read-only)
---------------------------------------------------------------------------

-- A successful call can return false or nil, and both are answers here: the
-- `ok and v or err` idiom would report them as errors.
local function readMetadataValue(fn)
	local ok, value = LrTasks.pcall(fn)
	if not ok then
		return "error: " .. tostring(value)
	end
	if value == nil then
		return "(nil)"
	end
	return X.plainValue(value)
end

local function metadataKeys(fn)
	local ok, all = LrTasks.pcall(fn)
	if ok and type(all) == "table" then
		return X.sortedKeys(all)
	end
	return ok and "(not a table)" or ("error: " .. tostring(all))
end

local function runE13(photos, sink)
	local exp = newExperiment(
		sink,
		"E13",
		"Runtime formats of orientation, dimensions and crop",
		"In which orientation does the SDK report dimensions and croppedDimensions, and what does the develop setting `orientation` look like?"
	)
	for _, photo in ipairs(photos) do
		local label = photoLabel(photo)
		local raw = {}
		for _, key in ipairs({
			"fileFormat",
			"dimensions",
			"croppedDimensions",
			"isCropped",
			"orientation",
			"width",
			"height",
			"aspectRatio",
			"isVirtualCopy",
		}) do
			raw[key] = readMetadataValue(function()
				return photo:getRawMetadata(key)
			end)
		end
		local formatted = {}
		for _, key in ipairs({ "dimensions", "croppedDimensions", "fileType" }) do
			formatted[key] = readMetadataValue(function()
				return photo:getFormattedMetadata(key)
			end)
		end

		local settings, readErr = readSettings(photo)
		local develop = X.pick(settings, CONTEXT_KEYS)
		if type(settings.Look) == "table" then
			develop.LookName = settings.Look.Name
		end
		develop.maskGroups = X.summarizeCorrections(settings.MaskGroupBasedCorrections)

		local cropModel
		local w, h = X.parseDimensions(raw.dimensions)
		local cw, ch = X.parseDimensions(raw.croppedDimensions)
		if w and cw then
			cropModel = X.cropModelCheck({ w = w, h = h }, { w = cw, h = ch }, {
				left = settings.CropLeft,
				top = settings.CropTop,
				right = settings.CropRight,
				bottom = settings.CropBottom,
				angle = settings.CropAngle,
			})
			table.insert(
				exp.verdicts,
				string.format(
					"%s: orientation = %s; %s",
					label,
					X.inlineValue(X.plainValue(settings.orientation)),
					X.describeCropModel(cropModel, settings.orientation)
				)
			)
		else
			table.insert(
				exp.verdicts,
				label
					.. ": could not parse dimensions "
					.. X.inlineValue(raw.dimensions)
					.. " / "
					.. X.inlineValue(raw.croppedDimensions)
			)
		end

		addStep(exp, label, "read metadata and develop settings", readErr == nil, readErr, {
			rawMetadata = raw,
			formattedMetadata = formatted,
			-- Settles whether `orientation` is a raw key at all: the SDK
			-- returns every available field for a nil key.
			rawMetadataKeys = metadataKeys(function()
				return photo:getRawMetadata(nil)
			end),
			develop = develop,
			cropModel = cropModel,
		})
	end
	table.insert(
		exp.manualChecks,
		"The frame verdict needs a portrait-orientation photo (rotated in camera) whose crop is straightened or changes the aspect ratio; otherwise it says 'ambiguous'."
	)
	return exp
end

---------------------------------------------------------------------------
-- E1: white balance keys
---------------------------------------------------------------------------

local function isRawFile(photo)
	local ok, format = LrTasks.pcall(function()
		return photo:getRawMetadata("fileFormat")
	end)
	return ok and (format == "RAW" or format == "DNG"), ok and format or nil
end

--- Every variant gets a fresh virtual copy, so each starts from the master's
-- white balance instead of whatever the previous variant left behind.
local function e1Variants(isRaw)
	if isRaw then
		return {
			{ id = "E1a", settings = { Temp = 7000 } },
			{ id = "E1b", settings = { Temperature = 7100 } },
			{ id = "E1c", settings = { WhiteBalance = "Custom", Temperature = 7200, Tint = 12 } },
			-- A no-op under "As Shot" must not be mistaken for the key being
			-- rejected: try `Temp` again with the mode set in the same call.
			{ id = "E1d", settings = { WhiteBalance = "Custom", Temp = 6500 }, probe = "Temperature", expect = 6500 },
		}
	end
	return {
		{ id = "E1a", settings = { Temp = 20 } },
		{ id = "E1b", settings = { IncrementalTemperature = 21, IncrementalTint = 11 } },
		{ id = "E1c", settings = { Temperature = 22 } },
		{
			id = "E1d",
			settings = { WhiteBalance = "Custom", Temp = 15 },
			probe = "IncrementalTemperature",
			expect = 15,
		},
	}
end

local function e1Outcome(variant, ok, err, readErr, after, changed)
	if not ok then
		return "raised an error: " .. tostring(err)
	end
	if readErr then
		return "applied, but the result is unknown - " .. tostring(readErr)
	end
	if variant.probe then
		if after[variant.probe] == variant.expect then
			return string.format("Temp honoured: %s = %s", variant.probe, tostring(after[variant.probe]))
		end
		return string.format(
			"Temp ignored even with WhiteBalance=Custom in the same call (%s = %s, WhiteBalance = %s)",
			variant.probe,
			tostring(after[variant.probe]),
			tostring(after.WhiteBalance)
		)
	end
	if next(changed) == nil then
		return "accepted without error, but changed nothing"
	end
	return "changed " .. X.inlineValue(X.plainValue(changed), 400)
end

local function runE1(catalog, photos, progress, sink)
	local exp = newExperiment(
		sink,
		"E1",
		"White balance keys via applyDevelopSettings",
		"Does applyDevelopSettings accept the `Temp` key AI Edit writes today, or only `Temperature` (raw) / `IncrementalTemperature` (non-raw)?"
	)
	for _, photo in ipairs(photos) do
		local label = photoLabel(photo)
		local isRaw, format = isRawFile(photo)
		for _, variant in ipairs(e1Variants(isRaw)) do
			if progress:isCanceled() then
				break
			end
			progress:setCaption("E1: " .. label .. " " .. variant.id)
			local copy, copyErr = createVirtualCopy(catalog, photo, "LrG Exp " .. variant.id)
			if not copy then
				addStep(exp, label, variant.id .. " create virtual copy", false, copyErr, nil)
				table.insert(exp.verdicts, string.format("%s %s: skipped - %s", label, variant.id, tostring(copyErr)))
			else
				local copyLabel = photoLabel(copy)
				local beforeSettings, beforeErr = readSettings(copy)
				local before = X.pick(beforeSettings, WB_KEYS)
				local ok, err = withWrite(catalog, "LrGenius experiment " .. variant.id, function()
					copy:applyDevelopSettings(variant.settings, "LrGenius " .. variant.id, false)
				end)
				local afterSettings, afterErr = readSettings(copy)
				local after = X.pick(afterSettings, WB_KEYS)
				local readErr = beforeErr or afterErr
				local changed = X.diff(before, after, WB_KEYS)
				local applied = X.inlineValue(variant.settings)
				addStep(exp, copyLabel, variant.id .. " apply " .. applied, ok and not readErr, err or readErr, {
					fileFormat = format,
					applied = variant.settings,
					before = before,
					after = after,
					changed = changed,
				})
				table.insert(
					exp.verdicts,
					string.format(
						"%s (%s) %s %s from WhiteBalance=%s: %s",
						label,
						tostring(format),
						variant.id,
						applied,
						tostring(before.WhiteBalance),
						e1Outcome(variant, ok, err, readErr, after, changed)
					)
				)
			end
		end
	end
	table.insert(
		exp.manualChecks,
		"History panel of each 'LrG Exp E1a/E1b/E1c/E1d' copy: is its single step named 'LrGenius E1x' (optHistoryName honoured) or a generic 'Multiple Settings'?"
	)
	table.insert(exp.manualChecks, "Basic panel of each 'LrG Exp E1x' copy: does it show that variant's white balance?")
	return exp
end

---------------------------------------------------------------------------
-- E2: AI masks through applyDevelopSettings
---------------------------------------------------------------------------

local function applyCorrections(catalog, photo, additions, actionName, historyName)
	local existing, readErr = correctionsOf(photo)
	if readErr then
		-- Writing the additions onto an unread state would drop every mask the
		-- copy inherited from its master.
		return false, readErr, 0
	end
	local ok, err = withWrite(catalog, actionName, function()
		photo:applyDevelopSettings({
			EnableMaskGroupBasedCorrections = true,
			MaskGroupBasedCorrections = X.appendCorrections(existing, additions),
		}, historyName, false)
	end)
	return ok, err, #existing
end

local function updateAI(catalog, photo, actionName)
	return withWrite(catalog, actionName, function()
		photo:updateAISettings()
	end)
end

local function runE2Extras(exp, catalog, copy, copyLabel, progress)
	local extras = {
		{
			correction = X.aiMaskCorrection({ name = "LrGenius E2 Sky -1 EV", mask = X.MASKS.sky, exposureStops = -1 }),
			needs = "visible sky",
		},
		{
			correction = X.aiMaskCorrection({
				name = "LrGenius E2 Background -1 EV",
				mask = X.MASKS.background,
				exposureStops = -1,
			}),
			needs = "a clear subject",
		},
		{
			correction = X.aiMaskCorrection({ name = "LrGenius E2 Hair +1 EV", mask = X.MASKS.hair, exposureStops = 1 }),
			needs = "a visible person",
		},
	}
	local corrections = {}
	for _, extra in ipairs(extras) do
		table.insert(corrections, extra.correction)
	end

	local available, waited, why = waitUntilAvailable(copy, progress)
	local okExtra, errExtra = false, unavailableMessage(waited, why)
	if available then
		okExtra, errExtra = applyCorrections(
			catalog,
			copy,
			corrections,
			"LrGenius experiment E2d",
			"LrGenius E2d sky, background, hair masks"
		)
	end
	local okUpdate, errUpdate = false, errExtra
	if okExtra then
		okUpdate, errUpdate = updateAI(catalog, copy, "LrGenius experiment E2d update")
	end
	addStep(exp, copyLabel, "E2d apply sky, background, hair and updateAISettings()", okExtra and okUpdate, errUpdate, {
		applied = okExtra,
		applyError = errExtra,
		updated = okUpdate,
		updateError = errUpdate,
	})

	for _, extra in ipairs(extras) do
		local name = extra.correction.CorrectionName
		if not okExtra then
			table.insert(exp.verdicts, copyLabel .. ": E2d " .. name .. " not applied: " .. tostring(errExtra))
		elseif not okUpdate then
			table.insert(
				exp.verdicts,
				copyLabel .. ": E2d " .. name .. " applied, but updateAISettings() failed: " .. tostring(errUpdate)
			)
		else
			local polled = pollCorrection(copy, extra.correction.CorrectionSyncID, AI_POLL_SECONDS, progress)
			addStep(exp, copyLabel, "E2d poll " .. name, true, nil, polled)
			local verdict = copyLabel .. ": E2d " .. name .. " is " .. X.describePoll(polled)
			if polled.state == "failed" then
				verdict = verdict .. " - only meaningful on a photo with " .. extra.needs
			end
			table.insert(exp.verdicts, verdict)
		end
	end
end

local function runE2OnPhoto(exp, catalog, photo, progress)
	local label = photoLabel(photo)
	local copy, copyErr = createVirtualCopy(catalog, photo, "LrG Exp E2")
	if not copy then
		addStep(exp, label, "create virtual copy", false, copyErr, nil)
		table.insert(exp.verdicts, label .. ": skipped - " .. tostring(copyErr))
		return
	end
	local copyLabel = photoLabel(copy)
	local module = currentModule()
	noteIfOffline(exp, photo, copyLabel)
	local needsBefore = needsUpdateAI(copy)

	-- E2a: one subject mask, +1 EV so it is obvious on screen.
	local subject = X.aiMaskCorrection({
		name = "LrGenius E2 Subject +1 EV",
		mask = X.MASKS.subject,
		exposureStops = 1,
	})
	local availableFirst, waitedFirst, whyFirst = waitUntilAvailable(copy, progress)
	local ok, err, countBefore = false, unavailableMessage(waitedFirst, whyFirst), 0
	if availableFirst then
		ok, err, countBefore =
			applyCorrections(catalog, copy, { subject }, "LrGenius experiment E2a", "LrGenius E2a subject mask")
	end
	local after = correctionsOf(copy)
	local found = X.findCorrections(after, subject.CorrectionSyncID)
	addStep(exp, copyLabel, "E2a applyDevelopSettings with a new subject mask", ok, err, {
		module = module,
		correctionsBefore = countBefore,
		correctionsAfter = #after,
		foundBySyncId = #found,
		added = X.summarizeCorrections(found),
		needsUpdateAISettingsBefore = needsBefore,
		needsUpdateAISettingsAfter = needsUpdateAI(copy),
	})
	if not ok then
		table.insert(exp.verdicts, copyLabel .. ": E2a not applied: " .. tostring(err))
		return
	end
	if #found == 0 then
		table.insert(
			exp.verdicts,
			string.format(
				"%s: E2a applyDevelopSettings returned without error but dropped the new subject mask (corrections %d -> %d)",
				copyLabel,
				countBefore,
				#after
			)
		)
		return
	end
	table.insert(
		exp.verdicts,
		string.format(
			"%s: E2a new subject mask %s (corrections %d -> %d, run from the %s module)",
			copyLabel,
			#found == 1 and "was kept" or ("found " .. #found .. " times"),
			countBefore,
			#after,
			tostring(module)
		)
	)

	-- E2b: does Lightroom compute it without being asked?
	local auto = pollCorrection(copy, subject.CorrectionSyncID, AUTO_COMPUTE_WAIT_SECONDS, progress)
	addStep(exp, copyLabel, "E2b wait without updateAISettings", true, nil, auto)
	table.insert(
		exp.verdicts,
		string.format(
			"%s: E2b without updateAISettings (waited up to %d s) the subject mask is %s",
			copyLabel,
			AUTO_COMPUTE_WAIT_SECONDS,
			X.describePoll(auto)
		)
	)
	if progress:isCanceled() then
		return
	end

	-- E2c: explicit update.
	if auto.timedOut then
		local available, waited, why = waitUntilAvailable(copy, progress)
		local okUpdate, errUpdate = false, unavailableMessage(waited, why)
		if available then
			okUpdate, errUpdate = updateAI(catalog, copy, "LrGenius experiment E2c")
		end
		local polled = okUpdate and pollCorrection(copy, subject.CorrectionSyncID, AI_POLL_SECONDS, progress) or nil
		addStep(exp, copyLabel, "E2c photo:updateAISettings() then poll", okUpdate, errUpdate, polled)
		local outcome = okUpdate and ("the subject mask is " .. X.describePoll(polled))
			or ("the call failed: " .. tostring(errUpdate))
		table.insert(exp.verdicts, copyLabel .. ": E2c after updateAISettings() " .. outcome)
	end
	if progress:isCanceled() then
		return
	end

	-- E2d: the other AI kinds, including a people-part category that
	-- LrDevelopController.createNewMask cannot express.
	runE2Extras(exp, catalog, copy, copyLabel, progress)
end

local function runE2(catalog, photos, progress, lrVersion, sink)
	local exp = newExperiment(
		sink,
		"E2",
		"New AI masks via applyDevelopSettings",
		"Can applyDevelopSettings add a digest-less AI mask definition, and does Lightroom compute it on its own or only after photo:updateAISettings()?"
	)
	if not X.versionAtLeast(lrVersion, 15, 3) then
		table.insert(
			exp.verdicts,
			"Lightroom is older than 15.3: needsUpdateAISettings/isAvailableForEditing are unavailable, so only the mask state is observed."
		)
	end
	for _, photo in ipairs(photos) do
		if progress:isCanceled() then
			break
		end
		progress:setCaption("E2: " .. photoLabel(photo))
		runE2OnPhoto(exp, catalog, photo, progress)
	end
	table.insert(
		exp.manualChecks,
		"Open an 'LrG Exp E2' copy in Develop > Masks: are the LrGenius masks listed, and does each cover what its name says (subject and hair brighter, sky and background darker)?"
	)
	table.insert(exp.manualChecks, "Did Lightroom show an AI progress dialog during E2c/E2d?")
	table.insert(
		exp.manualChecks,
		"History panel of an 'LrG Exp E2' copy: how many steps did E2a add, and how are they named?"
	)
	return exp
end

---------------------------------------------------------------------------
-- E4: plugin presets
---------------------------------------------------------------------------

local function pluginPresets()
	local ok, presets = LrTasks.pcall(function()
		return LrApplication.getDevelopPresetsForPlugin(_PLUGIN)
	end)
	if ok and type(presets) == "table" then
		return presets
	end
	return {}
end

local function presetCall(preset, method)
	local ok, value = LrTasks.pcall(function()
		return preset[method](preset)
	end)
	if ok then
		return value
	end
	return "error: " .. tostring(value)
end

local function countPresetsNamed(name)
	local total, named = 0, 0
	for _, preset in ipairs(pluginPresets()) do
		total = total + 1
		if presetCall(preset, "getName") == name then
			named = named + 1
		end
	end
	return total, named
end

local function describePreset(preset)
	if preset == nil then
		return nil
	end
	local info = {
		name = presetCall(preset, "getName"),
		uuid = presetCall(preset, "getUuid"),
		file = presetCall(preset, "getFile"),
	}
	local ok, setting = LrTasks.pcall(function()
		return preset:getSetting()
	end)
	if ok and type(setting) == "table" then
		info.settingKeys = X.sortedKeys(setting)
		info.Contrast2012 = setting.Contrast2012
		info.Exposure2012 = setting.Exposure2012
		info.SupportsAmount = setting.SupportsAmount
		info.SupportsAmount2 = setting.SupportsAmount2
		info.maskGroups = X.summarizeCorrections(setting.MaskGroupBasedCorrections)
	else
		info.settingError = tostring(setting)
	end
	if type(info.file) == "string" and LrFileUtils.exists(info.file) == "file" then
		local okRead, content = LrTasks.pcall(function()
			return LrFileUtils.readFile(info.file)
		end)
		if okRead and type(content) == "string" then
			info.fileSize = #content
			info.fileHead = content:sub(1, 3000)
			info.fileMentionsSupportsAmount = content:find("SupportsAmount", 1, true) ~= nil
		end
	end
	return info
end

--- The SDK does not say whether addDevelopPresetForPlugin needs a write gate;
-- try without first and record which way worked.
local function addPluginPreset(catalog, name, value)
	local ok, preset = LrTasks.pcall(function()
		return LrApplication.addDevelopPresetForPlugin(_PLUGIN, name, value)
	end)
	if ok and preset ~= nil then
		return preset, "outside a write gate"
	end
	local outsideError = ok and "returned nil" or tostring(preset)
	local insidePreset
	local okInside, errInside = withWrite(catalog, "LrGenius experiment: add plugin preset", function()
		insidePreset = LrApplication.addDevelopPresetForPlugin(_PLUGIN, name, value)
	end)
	if okInside and insidePreset ~= nil then
		return insidePreset, "inside a write gate (outside: " .. outsideError .. ")"
	end
	return nil, "outside: " .. outsideError .. "; inside: " .. tostring(errInside or "returned nil")
end

local function applyPreset(catalog, photo, preset, amount, withAI, actionName)
	return withWrite(catalog, actionName, function()
		photo:applyDevelopPreset(preset, _PLUGIN, amount, withAI)
	end)
end

--- Remembers every preset file the run created, for the cleanup instructions.
local function recordPresetFile(exp, info)
	if type(info) ~= "table" or type(info.file) ~= "string" or info.file:find("^error:") then
		return
	end
	for _, known in ipairs(exp.presetFiles) do
		if known == info.file then
			return
		end
	end
	table.insert(exp.presetFiles, info.file)
end

-- E4c/E4d on one photo: seed an existing mask, apply v2 with AI update, then
-- apply it again.
local function runE4ApplyOnPhoto(exp, catalog, photo, p2, sky2, subject1, progress)
	local label = photoLabel(photo)
	local copy, copyErr = createVirtualCopy(catalog, photo, "LrG Exp E4")
	if not copy then
		addStep(exp, label, "create virtual copy", false, copyErr, nil)
		table.insert(exp.verdicts, label .. ": skipped - " .. tostring(copyErr))
		return
	end
	local copyLabel = photoLabel(copy)
	noteIfOffline(exp, photo, copyLabel)

	-- A gradient needs no AI computation and third-party plugins already write
	-- it this way, so it is a dependable "mask the photo already has". Whether
	-- the preset keeps it answers merge vs replace.
	local seed = X.gradientCorrection({ name = "LrGenius E4 existing gradient", exposureStops = -0.5 })
	local seedAvailable, seedWaited, seedWhy = waitUntilAvailable(copy, progress)
	local seedOk, seedErr = false, unavailableMessage(seedWaited, seedWhy)
	if seedAvailable then
		seedOk, seedErr =
			applyCorrections(catalog, copy, { seed }, "LrGenius experiment E4 seed", "LrGenius E4 seed gradient")
	end
	local seeded = seedOk and #X.findCorrections(correctionsOf(copy), seed.CorrectionSyncID) > 0

	local countBefore = #correctionsOf(copy)
	local available, waited, why = waitUntilAvailable(copy, progress)
	local ok, err = false, unavailableMessage(waited, why)
	if available then
		ok, err = applyPreset(catalog, copy, p2, 100, true, "LrGenius experiment E4c")
	end
	local settings, settingsErr = readSettings(copy)
	local polled = ok and pollCorrection(copy, sky2.CorrectionSyncID, AI_POLL_SECONDS, progress) or nil
	local seedKept = #X.findCorrections(correctionsOf(copy), seed.CorrectionSyncID) > 0
	local v1Found = #X.findCorrections(correctionsOf(copy), subject1.CorrectionSyncID)
	addStep(exp, copyLabel, "E4c seed a gradient, then applyDevelopPreset(v2, amount 100, updateAI true)", ok, err, {
		module = currentModule(),
		seeded = seeded,
		seedError = seedErr,
		seedKept = seedKept,
		Contrast2012 = settings.Contrast2012,
		correctionsBefore = countBefore,
		correctionsAfter = #correctionsOf(copy),
		v1SubjectFound = v1Found,
		sky = polled,
	})
	if not ok then
		table.insert(exp.verdicts, copyLabel .. ": E4c applyDevelopPreset failed: " .. tostring(err))
		return
	end
	if settingsErr then
		table.insert(exp.verdicts, copyLabel .. ": E4c applied, but the result is unknown - " .. settingsErr)
		return
	end
	local mergeVerdict
	if not seeded then
		mergeVerdict = "merge vs replace inconclusive (the seed gradient did not persist: "
			.. tostring(seedErr or "dropped by applyDevelopSettings")
			.. ")"
	elseif seedKept then
		mergeVerdict = "the existing mask was kept, so preset masks are ADDED"
	else
		mergeVerdict = "the existing mask is gone, so preset masks REPLACE the photo's masks"
	end
	table.insert(
		exp.verdicts,
		string.format(
			"%s: E4c Contrast2012 = %s (v1 30, v2 -30); %s; v1's subject mask came along with v2: %s; v2 sky mask %s",
			copyLabel,
			tostring(settings.Contrast2012),
			mergeVerdict,
			tostring(v1Found > 0),
			polled and X.describePoll(polled) or "?"
		)
	)

	if progress:isCanceled() then
		return
	end
	local availableAgain, waitedAgain, whyAgain = waitUntilAvailable(copy, progress)
	local okAgain, errAgain = false, unavailableMessage(waitedAgain, whyAgain)
	if availableAgain then
		okAgain, errAgain = applyPreset(catalog, copy, p2, 100, true, "LrGenius experiment E4d")
	end
	local groupsAfter, readAgainErr = correctionsOf(copy)
	local skyCount = #X.findCorrections(groupsAfter, sky2.CorrectionSyncID)
	addStep(exp, copyLabel, "E4d apply the same preset again", okAgain and not readAgainErr, errAgain or readAgainErr, {
		skyCorrectionsWithSameSyncId = skyCount,
		correctionsAfter = #groupsAfter,
	})
	local outcome
	if not okAgain then
		outcome = "failed: " .. tostring(errAgain)
	elseif readAgainErr then
		outcome = "applied, but the result is unknown - " .. readAgainErr
	elseif skyCount == 0 then
		outcome = "left no mask with the preset's sync id at all"
	elseif skyCount == 1 then
		outcome = "updated the mask in place"
	else
		outcome = string.format("duplicated the mask (%d corrections with the preset's sync id)", skyCount)
	end
	table.insert(exp.verdicts, copyLabel .. ": E4d re-apply " .. outcome)
end

local function amountVerdict(variantKey, amount, ok, err, data)
	if not ok then
		return string.format("E4e %s amount %d failed: %s", variantKey, amount, tostring(err))
	end
	return string.format(
		"E4e %s amount %d: Contrast2012 %s -> %s (preset 40), Exposure2012 %s -> %s (preset 0.5), mask CorrectionAmount = %s, LocalExposure2012 = %s (preset 0.25)",
		variantKey,
		amount,
		tostring(data.Contrast2012Before),
		tostring(data.Contrast2012),
		tostring(data.Exposure2012Before),
		tostring(data.Exposure2012),
		tostring(data.maskCorrectionAmount),
		tostring(data.maskLocalExposure2012)
	)
end

-- E4e: amount semantics, with and without the SupportsAmount flags, so "plugin
-- presets ignore the amount" cannot be confused with "the preset was not
-- marked as scalable".
local function runE4Amount(exp, catalog, photo, progress)
	local variants = {
		{ key = "plain", name = AMOUNT_PRESET_NAME, flags = {} },
		{
			key = "flagged",
			name = AMOUNT_PRESET_NAME .. " Flagged",
			flags = { SupportsAmount = true, SupportsAmount2 = true },
		},
	}
	local label = photoLabel(photo)
	for _, variant in ipairs(variants) do
		if progress:isCanceled() then
			return
		end
		local subject =
			X.aiMaskCorrection({ name = "LrGenius E4 Amount Subject", mask = X.MASKS.subject, exposureStops = 1 })
		local value = {
			Contrast2012 = 40,
			Exposure2012 = 0.5,
			EnableMaskGroupBasedCorrections = true,
			MaskGroupBasedCorrections = { subject },
		}
		for key, flag in pairs(variant.flags) do
			value[key] = flag
		end
		local preset, how = addPluginPreset(catalog, variant.name, value)
		local info = describePreset(preset)
		recordPresetFile(exp, info)
		local createLabel = "E4e create amount preset (" .. variant.key .. ")"
		addStep(exp, "(global)", createLabel, preset ~= nil, preset == nil and how or nil, { how = how, preset = info })
		if preset == nil then
			table.insert(
				exp.verdicts,
				"E4e " .. variant.key .. " amount preset could not be created: " .. tostring(how)
			)
		else
			table.insert(
				exp.verdicts,
				string.format(
					"E4e %s preset: getSetting SupportsAmount=%s, SupportsAmount2=%s; file mentions SupportsAmount: %s",
					variant.key,
					tostring(info.SupportsAmount),
					tostring(info.SupportsAmount2),
					tostring(info.fileMentionsSupportsAmount)
				)
			)
			for _, amount in ipairs({ 50, 200 }) do
				if progress:isCanceled() then
					return
				end
				local copyName = "LrG Exp E4 amount " .. variant.key .. " " .. tostring(amount)
				local amountCopy, amountCopyErr = createVirtualCopy(catalog, photo, copyName)
				if not amountCopy then
					addStep(exp, label, copyName .. ": create virtual copy", false, amountCopyErr, nil)
					table.insert(exp.verdicts, copyName .. ": skipped - " .. tostring(amountCopyErr))
				else
					-- The amount may blend from the photo's current values rather
					-- than from zero, so the "before" is part of the answer.
					local before, beforeErr = readSettings(amountCopy)
					local actionName = "LrGenius experiment E4e " .. variant.key .. " " .. tostring(amount)
					local applied, applyErr = applyPreset(catalog, amountCopy, preset, amount, false, actionName)
					local settings, afterErr = readSettings(amountCopy)
					local ok = applied and not beforeErr and not afterErr
					local err = applyErr or beforeErr or afterErr
					local mask = X.findCorrections(correctionsOf(amountCopy), subject.CorrectionSyncID)[1]
					local data = {
						variant = variant.key,
						amount = amount,
						Contrast2012Before = before.Contrast2012,
						Exposure2012Before = before.Exposure2012,
						Contrast2012 = settings.Contrast2012,
						Exposure2012 = settings.Exposure2012,
						maskCorrectionAmount = mask and mask.CorrectionAmount,
						maskLocalExposure2012 = mask and mask.LocalExposure2012,
					}
					local stepLabel = "E4e " .. variant.key .. " amount " .. tostring(amount)
					addStep(exp, photoLabel(amountCopy), stepLabel, ok, err, data)
					table.insert(exp.verdicts, amountVerdict(variant.key, amount, ok, err, data))
				end
			end
		end
	end
end

-- `presetFiles` is owned by the caller, so the cleanup list survives even if
-- the experiment throws halfway through.
local function runE4(catalog, photos, progress, lrVersion, presetFiles, sink)
	local exp = newExperiment(
		sink,
		"E4",
		"Plugin preset lifecycle",
		"Where do plugin presets live and what do they hold, does a same-name add overwrite, are preset masks added to or replacing the photo's masks, is a re-apply idempotent, and what does the amount do?"
	)
	exp.presetFiles = presetFiles
	if not X.versionAtLeast(lrVersion, 15, 3) then
		table.insert(
			exp.verdicts,
			"Lightroom is older than 15.3: applyDevelopPreset has no updateAI parameter there, so AI masks from presets are not expected to compute."
		)
	end

	-- E4a/E4b: two presets under the same name, with different content.
	progress:setCaption("E4: creating plugin presets")
	local totalBefore, namedBefore = countPresetsNamed(PRESET_NAME)
	local subject1 =
		X.aiMaskCorrection({ name = "LrGenius E4 v1 Subject", mask = X.MASKS.subject, exposureStops = 0.5 })
	local p1, how1 = addPluginPreset(catalog, PRESET_NAME, {
		Contrast2012 = 30,
		EnableMaskGroupBasedCorrections = true,
		MaskGroupBasedCorrections = { subject1 },
	})
	local p1Info = describePreset(p1)
	recordPresetFile(exp, p1Info)
	addStep(exp, "(global)", "E4a addDevelopPresetForPlugin v1", p1 ~= nil, p1 == nil and how1 or nil, {
		how = how1,
		preset = p1Info,
		pluginPresetsBefore = totalBefore,
		namedBefore = namedBefore,
	})
	if p1 == nil then
		table.insert(exp.verdicts, "E4a addDevelopPresetForPlugin failed: " .. tostring(how1))
		return exp
	end
	table.insert(
		exp.verdicts,
		string.format(
			"E4a preset created %s; file %s (%s bytes); %d preset(s) of that name existed before",
			how1,
			tostring(p1Info.file),
			tostring(p1Info.fileSize),
			namedBefore
		)
	)

	local sky2 = X.aiMaskCorrection({ name = "LrGenius E4 v2 Sky", mask = X.MASKS.sky, exposureStops = -0.5 })
	local p2, how2 = addPluginPreset(catalog, PRESET_NAME, {
		Contrast2012 = -30,
		EnableMaskGroupBasedCorrections = true,
		MaskGroupBasedCorrections = { sky2 },
	})
	local totalAfter, namedAfter = countPresetsNamed(PRESET_NAME)
	local p2Info = describePreset(p2)
	recordPresetFile(exp, p2Info)
	local secondLabel = "E4b addDevelopPresetForPlugin v2 under the same name"
	addStep(exp, "(global)", secondLabel, p2 ~= nil, p2 == nil and how2 or nil, {
		how = how2,
		preset = p2Info,
		p1Now = describePreset(p1),
		pluginPresetsAfter = totalAfter,
		namedAfter = namedAfter,
	})
	if p2 == nil then
		table.insert(exp.verdicts, "E4b second addDevelopPresetForPlugin failed: " .. tostring(how2))
		return exp
	end
	table.insert(
		exp.verdicts,
		string.format(
			"E4b same-name add: presets named %q %d -> %d; same uuid: %s; same file: %s",
			PRESET_NAME,
			namedBefore,
			namedAfter,
			tostring(p1Info.uuid == p2Info.uuid),
			tostring(p1Info.file == p2Info.file)
		)
	)

	for index, photo in ipairs(photos) do
		if progress:isCanceled() then
			break
		end
		progress:setCaption("E4: " .. photoLabel(photo))
		runE4ApplyOnPhoto(exp, catalog, photo, p2, sky2, subject1, progress)
		if index == 1 and not progress:isCanceled() then
			runE4Amount(exp, catalog, photo, progress)
		end
	end

	table.insert(
		exp.manualChecks,
		"History panel of an 'LrG Exp E4' copy: how are the preset steps named, and is it one step per apply?"
	)
	table.insert(exp.manualChecks, "Did an AI progress dialog appear during E4c/E4d, and did it close on its own?")
	table.insert(
		exp.manualChecks,
		"On each 'LrG Exp E4 amount ...' copy in Develop: does the look match the amount (half at 50, double at 200)?"
	)
	table.insert(
		exp.manualChecks,
		"Delete the plugin preset files listed in the report header by hand. They live in Lightroom's preset folder, outside the catalog, and are shared by every catalog; deleting the test catalog does not remove them."
	)
	return exp
end

---------------------------------------------------------------------------
-- Dialog and report
---------------------------------------------------------------------------

local function optionsDialog(ctx, catalogPath, photoCount)
	local f = LrView.osFactory()
	local bind = LrView.bind
	local props = LrBinding.makePropertyTable(ctx)
	props.runE13 = true
	props.runE1 = true
	props.runE2 = true
	props.runE4 = true
	props.testCatalog = false

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:static_text({
			title = "Runs the develop-settings experiments from the AI Edit XMP research\n"
				.. "on the selected photos and writes a report you choose the location of.\n"
				.. "Best set: a portrait-orientation raw with a straightened crop, a raw that\n"
				.. "shows a person and sky, and a JPEG.",
		}),
		f:static_text({ title = "Catalog: " .. tostring(catalogPath) }),
		f:static_text({
			title = string.format("Selected photos: %d (at most %d)", photoCount, MAX_PHOTOS),
		}),
		f:group_box({
			title = "Experiments",
			fill_horizontal = 1,
			f:checkbox({
				value = bind("runE13"),
				title = "E13  Orientation, dimensions and crop at runtime (read-only)",
			}),
			f:checkbox({ value = bind("runE1"), title = "E1  White balance keys: Temp vs Temperature" }),
			f:checkbox({ value = bind("runE2"), title = "E2  Add AI masks via applyDevelopSettings" }),
			f:checkbox({
				value = bind("runE4"),
				title = "E4  Plugin presets: overwrite, merge, re-apply, amount",
			}),
		}),
		f:static_text({
			title = "E1, E2 and E4 write only to new virtual copies named 'LrG Exp ...'.\n"
				.. "They switch Lightroom to the Library module and change the selection.\n"
				.. "E4 leaves hidden plugin presets in Lightroom's preset folder. They are\n"
				.. "shared by all catalogs, are not removed with the test catalog, and the\n"
				.. "SDK cannot delete them; the report lists their files.",
		}),
		f:checkbox({
			value = bind("testCatalog"),
			title = "This is a test catalog - create the virtual copies, and I will delete the plugin presets",
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = "Develop-Settings Experiments",
		contents = contents,
		actionVerb = "Run",
		cancelVerb = "Cancel",
	})
	if result ~= "ok" then
		return nil
	end
	return {
		E13 = props.runE13,
		E1 = props.runE1,
		E2 = props.runE2,
		E4 = props.runE4,
		testCatalog = props.testCatalog,
	}
end

local function writeTextFile(path, text)
	local file, openErr = io.open(path, "wb")
	if not file then
		return false, tostring(openErr)
	end
	local ok, writeErr = file:write(text)
	-- Buffered data is flushed on close, so a full disk may only show here.
	local closed, closeErr = file:close()
	if not ok then
		return false, tostring(writeErr)
	end
	if not closed then
		return false, tostring(closeErr)
	end
	return true, nil
end

local function summaryText(experiments, failedSteps)
	local lines = {}
	for _, experiment in ipairs(experiments) do
		table.insert(lines, experiment.id .. " - " .. experiment.title)
		local shown = 0
		for _, verdict in ipairs(experiment.verdicts) do
			if shown < 4 then
				table.insert(lines, "  " .. verdict)
				shown = shown + 1
			end
		end
		if #experiment.verdicts > shown then
			table.insert(lines, string.format("  ... and %d more in the report", #experiment.verdicts - shown))
		end
	end
	if failedSteps > 0 then
		table.insert(lines, "")
		table.insert(
			lines,
			string.format("%d step(s) raised an error. That is often the answer itself - see the report.", failedSteps)
		)
	end
	return table.concat(lines, "\n")
end

--- Writes both report files and tells the user where they are - or, if a file
-- could not be written, what failed, together with the verdicts.
local function deliverReport(report, reportPath, jsonPath, runOk, runErr, presetFiles)
	local okMd, errMd = writeTextFile(reportPath, X.renderMarkdown(report))
	local okJson, json = LrTasks.pcall(function()
		return JSON:encode_pretty(report)
	end)
	local okJsonWrite, errJson = false, json
	if okJson then
		okJsonWrite, errJson = writeTextFile(jsonPath, json)
	end
	log:info(
		"Develop experiments: report "
			.. tostring(reportPath)
			.. " (written: "
			.. tostring(okMd)
			.. "), JSON "
			.. tostring(jsonPath)
			.. " (written: "
			.. tostring(okJsonWrite)
			.. ")"
	)

	local summary = summaryText(report.experiments, countFailedSteps(report.experiments))
	if #presetFiles > 0 then
		summary = summary
			.. "\n\nPlugin preset files to delete by hand (outside the catalog):\n  "
			.. table.concat(presetFiles, "\n  ")
	end

	if not okMd or not okJsonWrite then
		-- The verdicts are still worth having even when a file failed.
		local markdownLine = okMd and ("Markdown report: " .. tostring(reportPath))
			or ("The Markdown report could not be written: " .. tostring(errMd))
		local jsonLine = okJsonWrite and ("JSON: " .. tostring(jsonPath))
			or ("The JSON copy could not be written: " .. tostring(errJson))
		local runLine = ""
		if not runOk then
			runLine = "\nThe experiments also stopped early: " .. tostring(runErr)
		end
		ErrorHandler.handleError(
			"The experiment report was not saved completely",
			markdownLine .. "\n" .. jsonLine .. runLine .. "\n\n" .. summary
		)
		return
	end

	LrTasks.pcall(function()
		LrShell.revealInShell(reportPath)
	end)
	local locations = "Report: " .. tostring(reportPath) .. "\nJSON: " .. tostring(jsonPath)
	if not runOk then
		ErrorHandler.handleError(
			"The experiments stopped early",
			tostring(runErr) .. "\n\nEverything up to that point is saved.\n" .. locations .. "\n\n" .. summary
		)
		return
	end
	LrDialogs.message(
		report.meta.canceled and "Experiments canceled" or "Experiments finished",
		summary .. "\n\n" .. locations,
		"info"
	)
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskDevelopExperiments", function(ctx)
		local catalog = LrApplication.activeCatalog()
		local photos = catalog:getTargetPhotos() or {}

		local options = optionsDialog(ctx, catalog:getPath(), #photos)
		if not options then
			return
		end

		if #photos == 0 or #photos > MAX_PHOTOS then
			LrDialogs.message(
				"Select 1 to " .. MAX_PHOTOS .. " photos",
				"The experiments are meant for a handful of test photos: a portrait-orientation raw with a straightened crop, a raw that shows a person and sky, and a JPEG.",
				"warning"
			)
			return
		end

		local stills = {}
		for _, photo in ipairs(photos) do
			if not photo:getRawMetadata("isVideo") then
				table.insert(stills, photo)
			end
		end
		if #stills == 0 then
			LrDialogs.message("No photos selected", "Only videos are selected; the experiments need photos.", "warning")
			return
		end

		local writes = options.E1 or options.E2 or options.E4
		if not (writes or options.E13) then
			return
		end
		if writes and not options.testCatalog then
			LrDialogs.message(
				"Confirm the test catalog",
				"E1, E2 and E4 create virtual copies and plugin presets. Run them in a test catalog and tick the confirmation box, or run E13 on its own.",
				"warning"
			)
			return
		end

		local reportPath = LrDialogs.runSavePanel({
			title = "Save the experiment report",
			prompt = "Save Report",
			canCreateDirectories = true,
			requiredFileType = "md",
		})
		if not reportPath or reportPath == "" then
			return
		end
		-- The save panel only asked about the .md file; never clobber a .json
		-- the user did not choose.
		local jsonPath = LrPathUtils.replaceExtension(reportPath, "json")
		if LrFileUtils.exists(jsonPath) then
			jsonPath = LrFileUtils.chooseUniqueFileName(jsonPath)
		end

		local lrVersion = LrApplication.versionTable()
		local moduleAtStart = currentModule()
		local report = {
			meta = {
				lightroom = LrApplication.versionString(),
				lightroomVersion = X.plainValue(lrVersion),
				catalog = catalog:getPath(),
				startedAt = LrDate.timeToW3CDate(LrDate.currentTime()),
				moduleAtStart = moduleAtStart,
				photos = {},
			},
			experiments = {},
		}
		for _, photo in ipairs(stills) do
			table.insert(report.meta.photos, photoLabel(photo))
		end
		math.randomseed(math.floor(LrDate.currentTime() * 1000) % 2147483647)

		local progress = LrProgressScope({
			title = "Running develop-settings experiments",
			functionContext = ctx,
		})

		local presetFiles = {}
		local runOk, runErr = LrTasks.pcall(function()
			if options.E13 then
				progress:setCaption("E13: reading runtime formats")
				runE13(stills, report.experiments)
			end
			if writes then
				-- E2 and E4 ask whether masks can be added *outside* Develop,
				-- so run from the Library module on purpose.
				if moduleAtStart ~= "library" then
					LrApplicationView.switchToModule("library")
					LrTasks.sleep(0.5)
				end
				report.meta.moduleDuringRun = currentModule()
			end
			if options.E1 and not progress:isCanceled() then
				runE1(catalog, stills, progress, report.experiments)
			end
			if options.E2 and not progress:isCanceled() then
				runE2(catalog, stills, progress, lrVersion, report.experiments)
			end
			if options.E4 and not progress:isCanceled() then
				runE4(catalog, stills, progress, lrVersion, presetFiles, report.experiments)
			end
		end)
		report.meta.canceled = progress:isCanceled()
		progress:done()

		report.meta.finishedAt = LrDate.timeToW3CDate(LrDate.currentTime())
		if not runOk then
			report.meta.abortedWith = tostring(runErr)
		end
		if #presetFiles > 0 then
			report.meta.pluginPresetFilesToDelete = presetFiles
		end

		-- Put the selection back on the photos the user chose.
		LrTasks.pcall(function()
			catalog:setSelectedPhotos(stills[1], stills)
		end)

		deliverReport(report, reportPath, jsonPath, runOk, runErr, presetFiles)
	end)
end)
