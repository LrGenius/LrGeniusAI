--- Developer task: controlled develop-settings experiments.
--
-- The AI Edit XMP research (docs/wiki/Dev-AI-Edit-XMP-Findings.md) settled most
-- questions from files on disk, but these decide the architecture and can only
-- be answered by Lightroom itself:
--
--   E13  In which orientation does the SDK report `dimensions` and
--        `croppedDimensions`, and what does `orientation` look like at runtime?
--        Also records every top-level develop key of each photo and whether
--        the photo has develop edits; on an unedited raw and JPEG that is
--        Lightroom's raw and non-raw default table. (read-only)
--   E1   Does `applyDevelopSettings` accept the `Temp` key AI Edit writes today,
--        or only `Temperature` / `IncrementalTemperature`? And does writing only
--        a mode (`WhiteBalance="Daylight"` / `"Auto"`) make Lightroom recompute
--        the temperature and tint, and how soon?
--   E11  Does an Adobe Raw profile `Look` transfer through
--        `applyDevelopSettings`: as a stub in the form presets carry it
--        (`Stubbed = true`), as a bare stub, and as the full table with
--        `Parameters` taken from a photo? Raw photos only.
--   E2   Can `applyDevelopSettings` add a new AI mask (subject, sky, background,
--        people part), and does Lightroom compute it - on its own or after
--        `updateAISettings()`? How long does the update call take, and how long
--        until the mask is ready?
--   E4   How do plugin presets behave: same-name re-adds, where the file lives
--        and what it holds, merging with existing masks, re-applying, amount.
--        E4f: can a preset be applied and its file deleted right after, and
--        what does Lightroom still report about it until a restart?
--
-- Everything that writes goes to virtual copies named "LrG Exp ...", never to
-- the selected photos themselves. E4 additionally leaves hidden plugin presets
-- behind: they are files in Lightroom's preset folder, shared by every catalog,
-- and the SDK has no call to delete a preset. E4f deletes its own preset files
-- with LrFileUtils.delete - only paths that pass
-- `DevelopExperiments.presetFileDeletable` - and the final dialog lists every
-- preset file that still exists.
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
local DELETE_PRESET_NAME = "LrGenius Experiment E4f"
local DELETE_PRESET_CONTRAST = 25
local READD_PRESET_CONTRAST = 15
-- A white-balance mode may be resolved after `applyDevelopSettings` returns,
-- and Auto on a raw needs the pixels decoded first. E1's mode-only variants
-- read back right away, then every WB_POLL_INTERVAL seconds until the
-- temperature or tint changes or WB_POLL_SECONDS pass.
local WB_POLL_SECONDS = 15
local WB_POLL_INTERVAL = 0.5
-- E11b's preset fallback needs one full Look whose name differs from the
-- photo's; two distinct names always leave one that does.
local FULL_LOOKS_WANTED = 2

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
-- wait is visible as evidence of an automatic computation. `endedAt` is the
-- absolute time of the last reading, for timings anchored elsewhere.
local function pollCorrection(photo, syncId, timeoutSeconds, progress)
	local started = LrDate.currentTime()
	local timeline = {}
	local last = {}
	local state, matches, elapsed, readErr, endedAt
	while true do
		local groups
		groups, readErr = correctionsOf(photo)
		matches = X.findCorrections(groups, syncId)
		-- An unreadable state is terminal: polling on would report "still
		-- missing" for a mask that may well be there.
		state = readErr and "unreadable" or X.correctionState(matches[1])
		endedAt = LrDate.currentTime()
		elapsed = endedAt - started
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
		endedAt = endedAt,
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

--- Whether the photo has develop edits, per Lightroom's own "Has
-- Adjustments" search. The search is narrowed to the file name so it stays
-- cheap in a large catalog, and is only trusted when the name search alone
-- finds the photo.
-- @return boolean|string true, false, or "unknown: ..."
local function hasDevelopAdjustments(catalog, photo)
	local ok, result = LrTasks.pcall(function()
		local name = photo:getFormattedMetadata("fileName")
		local byName = { criteria = "filename", operation = "all", value = name }
		local function contains(found)
			for _, candidate in ipairs(found or {}) do
				if candidate == photo then
					return true
				end
			end
			return false
		end
		if not contains(catalog:findPhotos({ searchDesc = byName })) then
			return "unknown: a file-name search does not find the photo"
		end
		return contains(catalog:findPhotos({
			searchDesc = {
				{ criteria = "hasAdjustments", operation = "isTrue", value = true },
				byName,
				combine = "intersect",
			},
		}))
	end)
	if ok then
		return result
	end
	return "unknown: " .. tostring(result)
end

local function describeAdjustments(hasAdjustments)
	if hasAdjustments == true then
		return "has develop edits - not a default table"
	end
	if hasAdjustments == false then
		return "no develop edits"
	end
	return "could not tell whether it has develop edits (" .. tostring(hasAdjustments) .. ")"
end

local function runE13(catalog, photos, sink)
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
			"editCount",
			"lastEditTime",
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

		-- The complete top-level key set, for the raw and non-raw default
		-- tables. It goes into the step data only: the JSON has it in full,
		-- the verdict just says where to look.
		if readErr == nil then
			local full = X.summarizeSettings(settings)
			local family = X.settingsFamily(settings)
			local look = X.summarizeLook(settings.Look)
			local keyCount = #X.sortedKeys(full)
			local hasAdjustments = hasDevelopAdjustments(catalog, photo)
			addStep(exp, label, "full develop-settings readback", true, nil, {
				fileFormat = raw.fileFormat,
				whiteBalanceFamily = family,
				hasAdjustments = hasAdjustments,
				editCount = raw.editCount,
				lastEditTime = raw.lastEditTime,
				keyCount = keyCount,
				settings = full,
			})
			table.insert(
				exp.verdicts,
				string.format(
					"%s: %s; full readback has %d top-level keys, white-balance family %s, %s (every key in the JSON, step 'full develop-settings readback')",
					label,
					describeAdjustments(hasAdjustments),
					keyCount,
					tostring(family or "unknown"),
					look
							and string.format(
								"Look %s (Parameters: %s)",
								tostring(look.Name),
								look.hasParameters and "yes" or "no"
							)
						or "no Look"
				)
			)
		end
	end
	table.insert(
		exp.manualChecks,
		"The frame verdict needs a portrait-orientation photo (rotated in camera) whose crop is straightened or changes the aspect ratio; otherwise it says 'ambiguous'."
	)
	table.insert(
		exp.manualChecks,
		"The full readback shows Lightroom's defaults only for a photo without develop edits (the verdict says which ones have edits) and only if Preferences > Presets > Raw Defaults is 'Adobe Default': use a freshly imported raw and JPEG for the default tables."
	)
	return exp
end

---------------------------------------------------------------------------
-- E1: white balance keys
---------------------------------------------------------------------------

--- Raw or not, decided by the photo's white-balance key family where it has
-- one (a DNG converted from a JPEG is non-raw to Lightroom), by the file
-- format otherwise.
-- @return boolean, string|nil, string|nil isRaw, file format, family
local function isRawFile(photo)
	local ok, format = LrTasks.pcall(function()
		return photo:getRawMetadata("fileFormat")
	end)
	format = ok and format or nil
	local settings = readSettings(photo)
	local family = X.settingsFamily(settings)
	if family then
		return family == "raw", format, family
	end
	return format == "RAW" or format == "DNG", format, nil
end

--- Every variant gets a fresh virtual copy, so each starts from the master's
-- white balance instead of whatever the previous variant left behind.
-- `modeOnly` variants write a mode without values and ask whether Lightroom
-- recomputes the temperature and tint; they first put the copy on a
-- distinctive Custom white balance (`X.wbPrecondition`) so a recomputation
-- cannot hide behind values that happen to match. `flattenAuto` passes
-- optFlattenAutoNow = true, which the SDK documents as resolving Auto
-- settings synchronously.
local function e1Variants(isRaw)
	if isRaw then
		return {
			{ id = "E1a", settings = { Temp = 7000 } },
			{ id = "E1b", settings = { Temperature = 7100 } },
			{ id = "E1c", settings = { WhiteBalance = "Custom", Temperature = 7200, Tint = 12 } },
			-- A no-op under "As Shot" must not be mistaken for the key being
			-- rejected: try `Temp` again with the mode set in the same call.
			{ id = "E1d", settings = { WhiteBalance = "Custom", Temp = 6500 }, probe = "Temperature", expect = 6500 },
			{ id = "E1e", settings = { WhiteBalance = "Daylight" }, modeOnly = true },
			{ id = "E1f", settings = { WhiteBalance = "Auto" }, modeOnly = true },
			{ id = "E1g", settings = { WhiteBalance = "Auto" }, modeOnly = true, flattenAuto = true },
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
		-- Non-raw files have no named light sources, only As Shot, Auto and
		-- Custom; E1e (Daylight) is raw-only.
		{ id = "E1f", settings = { WhiteBalance = "Auto" }, modeOnly = true },
		{ id = "E1g", settings = { WhiteBalance = "Auto" }, modeOnly = true, flattenAuto = true },
	}
end

local function e1Outcome(variant, ok, err, readErr, after, changed, context)
	if not ok then
		return "raised an error: " .. tostring(err)
	end
	if readErr then
		return "applied, but the result is unknown - " .. tostring(readErr)
	end
	if variant.modeOnly then
		local text = X.describeWbModeOutcome(variant.settings.WhiteBalance, context.family, context.before, after, {
			immediate = context.immediate,
			settle = context.settle,
			flatten = variant.flattenAuto == true,
		})
		if not context.preconditionHeld then
			text = text
				.. string.format(
					" (the Custom precondition did not take%s, so the copy started from the master's white balance)",
					context.preconditionError and (": " .. tostring(context.preconditionError)) or ""
				)
		end
		return text
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

--- Reads a mode-only copy back right away, then polls until its
-- temperature or tint differs from `before` or WB_POLL_SECONDS pass.
-- @return table, string|nil, table, table after, read error, the immediate
--   readback, and `{ seconds, changed, canceled }` - `seconds` is when the
--   pair changed (0 = already in the immediate readback) or how long it did not.
local function waitForWbChange(copy, family, before, progress)
	local started = LrDate.currentTime()
	local settings, err = readSettings(copy)
	local immediate = X.pick(settings, WB_KEYS)
	if err then
		return immediate, err, nil, nil
	end
	if X.wbPairChanged(family, before, immediate) then
		return immediate, nil, immediate, { seconds = 0, changed = true }
	end
	local after = immediate
	while true do
		local elapsed = LrDate.currentTime() - started
		local canceled = progress:isCanceled()
		if elapsed >= WB_POLL_SECONDS or canceled then
			return after, nil, immediate, { seconds = round(elapsed), changed = false, canceled = canceled or nil }
		end
		LrTasks.sleep(WB_POLL_INTERVAL)
		settings, err = readSettings(copy)
		if err then
			return after, err, immediate, nil
		end
		after = X.pick(settings, WB_KEYS)
		if X.wbPairChanged(family, before, after) then
			return after, nil, immediate, { seconds = round(LrDate.currentTime() - started), changed = true }
		end
	end
end

local function runE1(catalog, photos, progress, sink)
	local exp = newExperiment(
		sink,
		"E1",
		"White balance keys via applyDevelopSettings",
		"Does applyDevelopSettings accept the `Temp` key AI Edit writes today, or only `Temperature` (raw) / `IncrementalTemperature` (non-raw)? And does writing only a mode (Daylight, Auto) make Lightroom recompute the temperature and tint?"
	)
	for _, photo in ipairs(photos) do
		local label = photoLabel(photo)
		local isRaw, format, family = isRawFile(photo)
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
				local startFamily = family or (isRaw and "raw" or "non-raw")
				local precondition, preconditionOk, preconditionErr
				if variant.modeOnly then
					precondition = X.wbPrecondition(startFamily)
					preconditionOk, preconditionErr = withWrite(
						catalog,
						"LrGenius experiment " .. variant.id .. " precondition",
						function()
							copy:applyDevelopSettings(precondition, "LrGenius " .. variant.id .. " precondition", false)
						end
					)
				end
				local beforeSettings, beforeErr = readSettings(copy)
				local before = X.pick(beforeSettings, WB_KEYS)
				local held = precondition ~= nil and preconditionOk and X.preconditionHeld(precondition, before)
				if precondition ~= nil and preconditionOk and not held and not beforeErr then
					preconditionErr = "it reads back as " .. X.inlineValue(before)
				end
				-- A mode-only write judges the family of the copy itself, not
				-- of the file format.
				local copyFamily = X.settingsFamily(beforeSettings) or startFamily
				local flatten = variant.flattenAuto == true
				local ok, err = withWrite(catalog, "LrGenius experiment " .. variant.id, function()
					copy:applyDevelopSettings(variant.settings, "LrGenius " .. variant.id, flatten)
				end)
				local after, afterErr, immediate, settle
				if variant.modeOnly and ok then
					after, afterErr, immediate, settle = waitForWbChange(copy, copyFamily, before, progress)
				else
					local afterSettings
					afterSettings, afterErr = readSettings(copy)
					after = X.pick(afterSettings, WB_KEYS)
				end
				local readErr = beforeErr or afterErr
				local changed = X.diff(before, after, WB_KEYS)
				local applied = X.inlineValue(variant.settings)
				if flatten then
					applied = applied .. " with optFlattenAutoNow=true"
				end
				-- A precondition that could not be written is a failed step; one
				-- that wrote but read back differently is only noted.
				local preconditionFailed = precondition ~= nil and not preconditionOk
				local stepErr = err
					or readErr
					or (preconditionFailed and ("precondition: " .. tostring(preconditionErr)))
					or nil
				addStep(
					exp,
					copyLabel,
					variant.id .. " apply " .. applied,
					ok and not readErr and not preconditionFailed,
					stepErr,
					{
						fileFormat = format,
						whiteBalanceFamily = copyFamily,
						precondition = precondition,
						preconditionHeld = precondition ~= nil and held or nil,
						preconditionError = preconditionErr,
						applied = variant.settings,
						flattenAutoNow = flatten,
						before = before,
						afterImmediately = immediate,
						settle = settle,
						after = after,
						changed = changed,
					}
				)
				table.insert(
					exp.verdicts,
					string.format(
						"%s (%s) %s %s from WhiteBalance=%s: %s",
						label,
						tostring(format),
						variant.id,
						applied,
						tostring(before.WhiteBalance),
						e1Outcome(variant, ok, err, readErr, after, changed, {
							family = copyFamily,
							before = before,
							immediate = immediate,
							settle = settle,
							preconditionHeld = held,
							preconditionError = preconditionErr,
						})
					)
				)
			end
		end
	end
	table.insert(
		exp.manualChecks,
		"History panel of each 'LrG Exp E1a' ... 'E1g' copy: is the variant's step named 'LrGenius E1x' (optHistoryName honoured) or a generic 'Multiple Settings'?"
	)
	table.insert(exp.manualChecks, "Basic panel of each 'LrG Exp E1x' copy: does it show that variant's white balance?")
	table.insert(
		exp.manualChecks,
		"Basic panel of the 'LrG Exp E1e/E1f/E1g' copies: does the WB menu show Daylight/Auto, and do the Temp and Tint sliders show the values the report read back? Their history starts with an 'LrGenius E1x precondition' step (a Custom white balance) before the mode itself."
	)
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

--- Calls `updateAISettings()` in a write gate and times it.
-- @return boolean, string|nil, table ok, error, and `{ updateSeconds,
--   updateCallSeconds, gateWaitSeconds, callStartedAt }`: the time around the
--   write gate, the call itself, the wait for write access before it, and the
--   absolute time the call started (the last three nil when the gate never
--   ran the function).
local function updateAI(catalog, photo, actionName)
	local started = LrDate.currentTime()
	local callStartedAt, callSeconds
	local ok, err = withWrite(catalog, actionName, function()
		callStartedAt = LrDate.currentTime()
		photo:updateAISettings()
		callSeconds = LrDate.currentTime() - callStartedAt
	end)
	return ok,
		err,
		{
			updateSeconds = round(LrDate.currentTime() - started, 2),
			updateCallSeconds = callSeconds and round(callSeconds, 2),
			gateWaitSeconds = callStartedAt and round(callStartedAt - started, 2),
			callStartedAt = callStartedAt,
		}
end

--- True when any reading of an unasked-wait timeline found the photo locked:
-- Lightroom was computing on its own.
local function lockedDuringWait(poll)
	for _, entry in ipairs(type(poll) == "table" and poll.timeline or {}) do
		if entry.isAvailableForEditing == false then
			return true
		end
	end
	return false
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
	local okUpdate, errUpdate, updateTiming = false, errExtra, nil
	if okExtra then
		okUpdate, errUpdate, updateTiming = updateAI(catalog, copy, "LrGenius experiment E2d update")
	end
	addStep(exp, copyLabel, "E2d apply sky, background, hair and updateAISettings()", okExtra and okUpdate, errUpdate, {
		applied = okExtra,
		applyError = errExtra,
		updated = okUpdate,
		updateError = errUpdate,
		updateSeconds = updateTiming and updateTiming.updateSeconds,
		updateCallSeconds = updateTiming and updateTiming.updateCallSeconds,
		gateWaitSeconds = updateTiming and updateTiming.gateWaitSeconds,
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

--- Runs E2a-E2d on one photo.
-- @return table The photo's timing entry for `X.describeTimings`: how long
--   the subject mask took to be ready, and how long the update call took.
local function runE2OnPhoto(exp, catalog, photo, progress)
	local label = photoLabel(photo)
	local copy, copyErr = createVirtualCopy(catalog, photo, "LrG Exp E2")
	if not copy then
		addStep(exp, label, "create virtual copy", false, copyErr, nil)
		table.insert(exp.verdicts, label .. ": skipped - " .. tostring(copyErr))
		return { photo = label, source = "none", reason = "no virtual copy" }
	end
	local copyLabel = photoLabel(copy)
	local function notMeasured(reason)
		return { photo = copyLabel, source = "none", reason = reason }
	end
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
		return notMeasured(err == CANCELED_MESSAGE and "canceled" or "the subject mask was not applied")
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
		return notMeasured("Lightroom dropped the subject mask")
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
	local timing
	if auto.state == "canceled" then
		return notMeasured("canceled")
	elseif auto.state == "unreadable" then
		timing = notMeasured("the mask state could not be read back")
	elseif not auto.timedOut then
		-- Computed without being asked: the time from the apply to that.
		timing = { photo = copyLabel, source = "auto", state = auto.state, readySeconds = auto.seconds }
	end
	if progress:isCanceled() then
		return timing or notMeasured("canceled")
	end

	-- E2c: explicit update.
	if auto.timedOut then
		local available, waited, why = waitUntilAvailable(copy, progress)
		local okUpdate, errUpdate, updateTiming = false, unavailableMessage(waited, why), nil
		if available then
			okUpdate, errUpdate, updateTiming = updateAI(catalog, copy, "LrGenius experiment E2c")
		end
		local polled = okUpdate and pollCorrection(copy, subject.CorrectionSyncID, AI_POLL_SECONDS, progress) or nil
		if updateTiming and updateTiming.callStartedAt then
			-- Anchored at the start of the call itself, so waiting for write
			-- access is not counted as compute time.
			timing = {
				photo = copyLabel,
				source = "update",
				updateOk = okUpdate,
				updateSeconds = updateTiming.updateSeconds,
				updateCallSeconds = updateTiming.updateCallSeconds,
				gateWaitSeconds = updateTiming.gateWaitSeconds,
				waitedBeforeUpdate = waited,
				lockedDuringAutoWait = lockedDuringWait(auto),
				state = polled and polled.state,
				readySeconds = polled and polled.endedAt and round(polled.endedAt - updateTiming.callStartedAt),
			}
		elseif updateTiming then
			timing = notMeasured("write access was not granted: " .. tostring(errUpdate))
		else
			timing = notMeasured(errUpdate == CANCELED_MESSAGE and "canceled" or tostring(errUpdate))
		end
		addStep(exp, copyLabel, "E2c photo:updateAISettings() then poll", okUpdate, errUpdate, {
			updateSeconds = updateTiming and updateTiming.updateSeconds,
			updateCallSeconds = updateTiming and updateTiming.updateCallSeconds,
			gateWaitSeconds = updateTiming and updateTiming.gateWaitSeconds,
			waitedBeforeUpdate = waited,
			lockedDuringAutoWait = lockedDuringWait(auto),
			maskReadySeconds = X.timingIsTerminal(timing) and timing.readySeconds or nil,
			timing = X.describeTiming(timing),
			poll = polled,
		})
		local outcome = okUpdate and ("the subject mask is " .. X.describePoll(polled))
			or ("the call failed: " .. tostring(errUpdate))
		table.insert(
			exp.verdicts,
			copyLabel .. ": E2c after updateAISettings() " .. outcome .. " (" .. X.describeTiming(timing) .. ")"
		)
	end
	if progress:isCanceled() then
		return timing
	end

	-- E2d: the other AI kinds, including a people-part category that
	-- LrDevelopController.createNewMask cannot express.
	runE2Extras(exp, catalog, copy, copyLabel, progress)
	return timing
end

local function runE2(catalog, photos, progress, lrVersion, sink)
	local exp = newExperiment(
		sink,
		"E2",
		"New AI masks via applyDevelopSettings",
		"Can applyDevelopSettings add a digest-less AI mask definition, and does Lightroom compute it on its own or only after photo:updateAISettings()? How long do the update call and the mask take?"
	)
	if not X.versionAtLeast(lrVersion, 15, 3) then
		table.insert(
			exp.verdicts,
			"Lightroom is older than 15.3: needsUpdateAISettings/isAvailableForEditing are unavailable, so only the mask state is observed."
		)
	end
	local timings = {}
	for _, photo in ipairs(photos) do
		if progress:isCanceled() then
			break
		end
		progress:setCaption("E2: " .. photoLabel(photo))
		table.insert(timings, runE2OnPhoto(exp, catalog, photo, progress))
	end
	-- One line with every photo's numbers: the basis for any runtime estimate
	-- of AI masks in a batch.
	table.insert(exp.verdicts, X.describeTimings(timings))
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
			info.fileContrast2012 = X.presetFileSetting(content, "Contrast2012")
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

local function pathExists(path)
	if type(path) ~= "string" then
		return false
	end
	local kind = LrFileUtils.exists(path)
	return kind == "file" or kind == "directory"
end

--- Takes `file` off the cleanup list once it no longer exists on disk, so the
-- final dialog only names files the user still has to delete.
local function forgetDeletedPresetFile(exp, file)
	if type(file) ~= "string" or pathExists(file) then
		return
	end
	for index = #exp.presetFiles, 1, -1 do
		if exp.presetFiles[index] == file then
			table.remove(exp.presetFiles, index)
		end
	end
end

--- Deletes one E4f preset file - only a path `X.presetFileDeletable` accepts,
-- and only when it is a file: `LrFileUtils.delete` would remove a directory
-- with everything in it.
-- @param knownFiles table The preset files of the other E4 steps.
-- @return table `{ skipped, ok, error, existsAfter, returned }` for `X.describeE4f`.
local function deletePresetFile(file, knownFiles)
	local allowed, why = X.presetFileDeletable(file, DELETE_PRESET_NAME, knownFiles)
	if not allowed then
		return { skipped = why }
	end
	local kind = LrFileUtils.exists(file)
	if kind ~= "file" then
		return { skipped = "there is no file at " .. file .. " (LrFileUtils.exists: " .. tostring(kind) .. ")" }
	end
	local deleted, reason
	local ok, err = LrTasks.pcall(function()
		deleted, reason = LrFileUtils.delete(file)
	end)
	local result = { existsAfter = pathExists(file), returned = tostring(deleted) }
	if not ok then
		result.ok = false
		result.error = tostring(err)
	elseif deleted ~= true and result.existsAfter then
		result.ok = false
		result.error = tostring(reason or ("LrFileUtils.delete returned " .. tostring(deleted)))
	else
		-- The file is gone even if the return value was not `true`, or the
		-- call claimed success; `existsAfter` tells the two cases apart.
		result.ok = true
	end
	return result
end

local function deleteStepError(result)
	return result.skipped or result.error or (result.existsAfter and "the file is still there" or nil)
end

-- E4f, steps; `r` collects what `X.describeE4f` turns into verdicts and is
-- filled step by step, so a stop halfway leaves the later parts nil.
local function runE4DeleteSteps(exp, catalog, photo, progress, r)
	progress:setCaption("E4f: apply a preset, then delete its file")
	-- A snapshot taken before E4f records its own file: the path check
	-- compares against the files the other E4 steps created.
	local knownFiles = {}
	for _, known in ipairs(exp.presetFiles) do
		table.insert(knownFiles, known)
	end
	-- The presets of this name that are already listed, each with whether its
	-- file exists: the only programmatic trace of an earlier run after a
	-- restart, recorded before this run's add can overwrite that file.
	r.before = {}
	for _, listed in ipairs(pluginPresets()) do
		if presetCall(listed, "getName") == DELETE_PRESET_NAME then
			local listedFile = presetCall(listed, "getFile")
			table.insert(r.before, {
				uuid = presetCall(listed, "getUuid"),
				file = listedFile,
				fileExists = pathExists(listedFile),
			})
		end
	end

	local subject = X.aiMaskCorrection({ name = "LrGenius E4f Subject", mask = X.MASKS.subject, exposureStops = 0.5 })
	local preset, how = addPluginPreset(catalog, DELETE_PRESET_NAME, {
		Contrast2012 = DELETE_PRESET_CONTRAST,
		EnableMaskGroupBasedCorrections = true,
		MaskGroupBasedCorrections = { subject },
	})
	local info = describePreset(preset)
	-- Listed right away, so a run that stops before the delete still names
	-- the file in the cleanup list.
	recordPresetFile(exp, info)
	r.create = { ok = preset ~= nil, error = preset == nil and how or nil }
	addStep(exp, "(global)", "E4f addDevelopPresetForPlugin", preset ~= nil, preset == nil and how or nil, {
		how = how,
		preset = info,
		before = r.before,
	})
	if preset == nil then
		return
	end
	local file = info.file

	-- Plugin presets never appear in Develop > Presets (SDK reference,
	-- addDevelopPresetForPlugin), so the check looks at the folder itself.
	local presetFolder = type(file) == "string" and not file:find("^error:") and X.splitPath(file) or nil
	table.insert(
		exp.manualChecks,
		"Quit and restart Lightroom, then open the 'Plugin Develop Presets' folder "
			.. (presetFolder and ("(" .. presetFolder .. ")") or "(the folder of the E4f preset path in the report)")
			.. " in Finder or Explorer: is a file named '"
			.. DELETE_PRESET_NAME
			.. "' back? Plugin presets never show in Develop > Presets, so that panel tells nothing either way."
	)
	table.insert(
		exp.manualChecks,
		"After that restart, run E4 again: its first E4f verdict then says whether presets named '"
			.. DELETE_PRESET_NAME
			.. "' are still listed, and for each whether its file is missing (kept somewhere else) or present (left over or written back at quit)."
	)
	table.insert(
		exp.manualChecks,
		"After the restart, on the 'LrG Exp E4f' copy in Develop: does it still show the preset's contrast and subject mask, and how is the preset step named in the History panel?"
	)

	local function stopIfCanceled(label)
		if progress:isCanceled() then
			addStep(exp, "(global)", label, false, CANCELED_MESSAGE, { file = file })
			r.canceled = true
			return true
		end
		return false
	end

	-- Apply to a fresh copy and read it back.
	local label = photoLabel(photo)
	local copy, copyErr = createVirtualCopy(catalog, photo, "LrG Exp E4f")
	if not copy then
		addStep(exp, label, "E4f create virtual copy", false, copyErr, nil)
		r.apply = { ok = false, error = "no virtual copy: " .. tostring(copyErr) }
		return
	end
	local copyLabel = photoLabel(copy)
	noteIfOffline(exp, photo, copyLabel)
	local applyLabel = "E4f applyDevelopPreset(amount 100, updateAI true)"
	local available, waited, why = waitUntilAvailable(copy, progress)
	if not available and why == "canceled" then
		addStep(exp, copyLabel, applyLabel, false, CANCELED_MESSAGE, { file = file })
		r.canceled = true
		return
	end
	local ok, err = false, unavailableMessage(waited, why)
	if available then
		ok, err = applyPreset(catalog, copy, preset, 100, true, "LrGenius experiment E4f")
	end
	local polled = ok and pollCorrection(copy, subject.CorrectionSyncID, AI_POLL_SECONDS, progress) or nil
	local settings, readErr = readSettings(copy)
	r.apply = {
		ok = ok,
		error = err,
		readError = ok and readErr or nil,
		contrast = settings.Contrast2012,
		maskFound = #X.findCorrections(settings.MaskGroupBasedCorrections, subject.CorrectionSyncID) > 0,
		maskState = polled and X.describePoll(polled) or nil,
		maskPollState = polled and polled.state or nil,
	}
	addStep(exp, copyLabel, applyLabel, ok and not readErr, err or readErr, {
		Contrast2012 = settings.Contrast2012,
		presetContrast2012 = DELETE_PRESET_CONTRAST,
		subjectMaskFound = r.apply.maskFound,
		subject = polled,
		file = file,
	})
	if not ok or readErr then
		return
	end

	-- Delete the preset's file.
	if stopIfCanceled("E4f delete the preset file") then
		return
	end
	r.delete = deletePresetFile(file, knownFiles)
	forgetDeletedPresetFile(exp, file)
	local deleteErr = deleteStepError(r.delete)
	addStep(exp, "(global)", "E4f delete the preset file with LrFileUtils.delete", deleteErr == nil, deleteErr, {
		file = file,
		result = r.delete,
	})
	if deleteErr ~= nil then
		return
	end

	-- What Lightroom still reports about the deleted preset.
	local namedAfter, listedByUuid = 0, false
	for _, listed in ipairs(pluginPresets()) do
		if presetCall(listed, "getName") == DELETE_PRESET_NAME then
			namedAfter = namedAfter + 1
		end
		if info.uuid ~= nil and presetCall(listed, "getUuid") == info.uuid then
			listedByUuid = true
		end
	end
	local okLookup, found = LrTasks.pcall(function()
		return LrApplication.getDevelopPresetsForPlugin(_PLUGIN, info.uuid)
	end)
	local lookup
	if not okLookup then
		lookup = "error: " .. tostring(found)
	elseif found == nil then
		lookup = "nothing"
	else
		lookup = "found " .. tostring(presetCall(found, "getName"))
	end
	local okSetting, setting = LrTasks.pcall(function()
		return preset:getSetting()
	end)
	local settingResult = { ok = okSetting and type(setting) == "table" }
	if settingResult.ok then
		settingResult.contrast = setting.Contrast2012
	else
		settingResult.error = okSetting and ("returned " .. type(setting)) or tostring(setting)
	end
	-- Deleting a preset must not undo an edit it made.
	local firstSettings, firstErr = readSettings(copy)
	r.after = {
		namedCount = namedAfter,
		listedByUuid = listedByUuid,
		lookup = lookup,
		setting = settingResult,
		first = {
			readError = firstErr,
			contrast = firstSettings.Contrast2012,
			maskFound = #X.findCorrections(firstSettings.MaskGroupBasedCorrections, subject.CorrectionSyncID) > 0,
			-- Found only proves the definition is there; the state says
			-- whether the computed mask is too.
			maskState = X.correctionState(
				X.findCorrections(firstSettings.MaskGroupBasedCorrections, subject.CorrectionSyncID)[1]
			),
		},
	}
	addStep(exp, copyLabel, "E4f what Lightroom reports after the delete", firstErr == nil, firstErr, r.after)

	-- Apply the preset object whose file is gone to a second fresh copy.
	if stopIfCanceled("E4f apply the deleted preset to a second copy") then
		return
	end
	local secondLabel = "E4f apply the deleted preset to a second copy"
	local copy2, copy2Err = createVirtualCopy(catalog, photo, "LrG Exp E4f after delete")
	if not copy2 then
		addStep(exp, label, secondLabel, false, copy2Err, nil)
		r.second = { copyError = copy2Err }
	else
		local copy2Label = photoLabel(copy2)
		local available2, waited2, why2 = waitUntilAvailable(copy2, progress)
		if not available2 and why2 == "canceled" then
			addStep(exp, copy2Label, secondLabel, false, CANCELED_MESSAGE, { file = file })
			r.canceled = true
			return
		end
		local ok2, err2 = false, unavailableMessage(waited2, why2)
		if available2 then
			ok2, err2 = applyPreset(catalog, copy2, preset, 100, true, "LrGenius experiment E4f after delete")
		end
		local settings2, readErr2 = readSettings(copy2)
		r.second = {
			ok = ok2,
			error = err2,
			readError = ok2 and readErr2 or nil,
			contrast = settings2.Contrast2012,
			maskFound = #X.findCorrections(settings2.MaskGroupBasedCorrections, subject.CorrectionSyncID) > 0,
			fileBack = pathExists(file),
		}
		if r.second.fileBack then
			-- Applying brought the file back: it is a file to clean up again.
			recordPresetFile(exp, info)
		end
		-- Failing is a possible answer here, not a broken step: the step is
		-- only failed when the copy could not be read back.
		addStep(exp, copy2Label, secondLabel, readErr2 == nil, readErr2, r.second)
	end

	-- Re-add the same name: does Lightroom write a file again, and where?
	if stopIfCanceled("E4f re-add the same name") then
		return
	end
	local readdPreset, readdHow = addPluginPreset(catalog, DELETE_PRESET_NAME, {
		Contrast2012 = READD_PRESET_CONTRAST,
	})
	local readdInfo = describePreset(readdPreset)
	local readd = {
		ok = readdPreset ~= nil,
		error = readdPreset == nil and readdHow or nil,
		expectedContrast = READD_PRESET_CONTRAST,
	}
	if readdInfo then
		readd.file = readdInfo.file
		readd.fileExists = pathExists(readdInfo.file)
		readd.samePath = readdInfo.file == file
		readd.sameUuid = readdInfo.uuid == info.uuid
		-- New settings, or stale ones from the deleted preset of that name?
		readd.contrast = readdInfo.Contrast2012
		if readd.fileExists then
			readd.fileContrast = readdInfo.fileContrast2012
			if readd.fileContrast ~= nil then
				readd.fileHasNewContrast = readd.fileContrast == READD_PRESET_CONTRAST
			end
		end
		recordPresetFile(exp, readdInfo)
		forgetDeletedPresetFile(exp, readdInfo.file)
	end
	-- Does every add-then-delete cycle grow the list until a restart? AI Edit
	-- would run one per photo.
	if readd.ok then
		readd.namedCount = select(2, countPresetsNamed(DELETE_PRESET_NAME))
	end
	r.readd = readd
	addStep(exp, "(global)", "E4f re-add the same name", readd.ok, readd.error, {
		how = readdHow,
		preset = readdInfo,
		result = readd,
	})
	-- Clean up the re-added file too, so the restart check above looks at a
	-- preset with no file left at all.
	if readd.fileExists and not stopIfCanceled("E4f delete the re-added preset file") then
		readd.cleanup = deletePresetFile(readdInfo.file, knownFiles)
		forgetDeletedPresetFile(exp, readdInfo.file)
		local cleanupErr = deleteStepError(readd.cleanup)
		addStep(exp, "(global)", "E4f delete the re-added preset file", cleanupErr == nil, cleanupErr, {
			file = readdInfo.file,
			result = readd.cleanup,
		})
	end
end

-- E4f: the SDK has no call to delete a preset, so AI Edit could only clean up
-- a per-edit preset by deleting its file. Applies a preset, deletes the file,
-- and records what Lightroom still reports until it restarts.
local function runE4Delete(exp, catalog, photo, progress)
	local r = { name = DELETE_PRESET_NAME, expectedContrast = DELETE_PRESET_CONTRAST }
	runE4DeleteSteps(exp, catalog, photo, progress, r)
	for _, line in ipairs(X.describeE4f(r)) do
		table.insert(exp.verdicts, line)
	end
end

-- `presetFiles` is owned by the caller, so the cleanup list survives even if
-- the experiment throws halfway through.
local function runE4(catalog, photos, progress, lrVersion, presetFiles, sink)
	local exp = newExperiment(
		sink,
		"E4",
		"Plugin preset lifecycle",
		"Where do plugin presets live and what do they hold, does a same-name add overwrite, are preset masks added to or replacing the photo's masks, is a re-apply idempotent, what does the amount do, and can a preset's file be deleted right after applying it?"
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
	-- Last, so the other steps have recorded the preset files whose folder the
	-- delete is checked against.
	if photos[1] and not progress:isCanceled() then
		runE4Delete(exp, catalog, photos[1], progress)
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
		"Delete the plugin preset files listed in the report header by hand (E4f deletes its own; any it could not are listed too). They live in Lightroom's preset folder, outside the catalog, and are shared by every catalog; deleting the test catalog does not remove them."
	)
	return exp
end

---------------------------------------------------------------------------
-- E11: Look transfer
---------------------------------------------------------------------------

--- Last-resort source for E11b: complete Looks (with `Parameters`) among the
-- installed develop presets. Lightroom's bundled presets carry Looks only as
-- stubs, so this normally finds nothing; the photos are the real source.
-- @return table `{ found, scanned, failedReads, firstError, stubKeys,
--   canceled, error }`: up to FULL_LOOKS_WANTED entries `{ look, preset,
--   folder }` with distinct names, how many presets were read and how many of
--   those reads failed (with the first error), the key list of the first
--   stubbed Look seen, whether the user canceled, and an error that stopped
--   the scan.
local function findFullLooks(progress)
	local scan = { found = {}, scanned = 0, failedReads = 0, canceled = false }
	local seen = {}
	local ok, err = LrTasks.pcall(function()
		for _, folder in ipairs(LrApplication.developPresetFolders() or {}) do
			local folderName = presetCall(folder, "getName")
			for _, preset in ipairs(folder:getDevelopPresets() or {}) do
				if progress:isCanceled() then
					scan.canceled = true
					return
				end
				scan.scanned = scan.scanned + 1
				local okSetting, setting = LrTasks.pcall(function()
					return preset:getSetting()
				end)
				if not okSetting or type(setting) ~= "table" then
					scan.failedReads = scan.failedReads + 1
					scan.firstError = scan.firstError
						or string.format("%s: %s", tostring(presetCall(preset, "getName")), tostring(setting))
				else
					local look = setting.Look
					if scan.stubKeys == nil and type(look) == "table" and look.Stubbed ~= nil then
						scan.stubKeys = X.summarizeLook(look).keys
					end
					if X.isFullLook(look) and not look.isAdobeAdaptive and not seen[look.Name] then
						seen[look.Name] = true
						table.insert(
							scan.found,
							{ look = look, preset = presetCall(preset, "getName"), folder = folderName }
						)
						if #scan.found >= FULL_LOOKS_WANTED then
							return
						end
					end
				end
			end
			LrTasks.yield()
		end
	end)
	if not ok then
		scan.error = tostring(err)
	end
	return scan
end

local function lookState(settings)
	return { CameraProfile = settings.CameraProfile, look = X.summarizeLook(settings.Look) }
end

local function cameraModel(photo)
	local ok, model = LrTasks.pcall(function()
		return photo:getFormattedMetadata("cameraModel")
	end)
	return ok and model or nil
end

--- The E11 variants. E11a and E11c are stubs of the first Adobe Raw profile
-- whose name differs from the photo's Look - in the preset form and bare;
-- E11b is a complete Look, chosen at run time by `e11FullLook`.
local E11_VARIANTS = {
	{ id = "E11a", kind = "stubbed (preset form, Stubbed=true)" },
	{ id = "E11b", kind = "full" },
	{ id = "E11c", kind = "bare stub (no Stubbed flag)", bare = true },
}

--- Picks E11b's full Look, from the most to the least direct source:
--   1. the Look of another selected photo (a complete Look as Lightroom
--      stores it on a photo);
--   2. the Look the E11a copy read back, if Lightroom filled in the stub;
--   3. the photo's own Look, written onto a copy that was first switched to a
--      stubbed different Look (see `prepareOwnLook`);
--   4. a develop preset whose Look has `Parameters` (normally none).
-- @param ctx table `{ photo, currentName, masterLook, camera, donors,
--   e11aLook, getPresetScan }`.
-- @return table|nil, table source description, string|nil skip reason
local function e11FullLook(ctx)
	local donor, rejected = X.pickDonorLook(ctx.donors, ctx.currentName, ctx.camera)
	if donor then
		return donor.look, { kind = "another selected photo", photo = donor.photo, passedOver = rejected }, nil
	end
	local fromE11a = ctx.e11aLook
	if fromE11a and X.lookRejection(fromE11a, ctx.currentName, ctx.camera, ctx.camera) == nil then
		return fromE11a,
			{ kind = "the E11a copy's readback (Lightroom filled in the stub)", passedOver = rejected },
			nil
	end
	local ownReason = X.lookRejection(ctx.masterLook, nil, ctx.camera, ctx.camera)
	if ownReason == nil then
		return ctx.masterLook,
			{
				kind = "the photo's own Look, after switching the copy away from it",
				ownLook = true,
				passedOver = rejected,
			},
			nil
	end
	local scan = ctx.getPresetScan()
	if scan.canceled then
		return nil, nil, CANCELED_MESSAGE
	end
	if scan.error then
		return nil, nil, "no photo offers a full Look, and the develop-preset scan failed: " .. scan.error
	end
	local entry = X.pickLook(scan.found, ctx.currentName, function(candidate)
		return candidate.look.Name
	end)
	if entry then
		return entry.look, { kind = "develop preset", preset = entry.preset, folder = entry.folder }, nil
	end
	local reasons = { "the photo's own Look: " .. tostring(ownReason) }
	for _, item in ipairs(rejected) do
		table.insert(reasons, tostring(item.photo) .. ": " .. tostring(item.reason))
	end
	return nil,
		nil,
		"no source for a full Look (" .. table.concat(reasons, "; ") .. "), and " .. X.describePresetScan(scan)
end

--- Switches a fresh copy away from the photo's own Look, in history steps of
-- their own, so writing that Look back afterwards has something to change.
-- Tries the preset-form stub first, then the bare one, since either may be
-- the form Lightroom ignores.
-- @return boolean, table true once the copy's Look differs from the
--   photo's, and the step data (every attempt).
local function prepareOwnLook(catalog, copy, currentName, cameraProfile)
	local candidate = X.pickLook(X.STUB_LOOKS, currentName)
	if not candidate then
		return false, { error = "every stub candidate is already the photo's Look", attempts = {} }
	end
	local data = { attempts = {} }
	for _, bare in ipairs({ false, true }) do
		local stub = X.stubLook(candidate, bare)
		local ok, err = withWrite(catalog, "LrGenius experiment E11b prepare", function()
			copy:applyDevelopSettings({ CameraProfile = cameraProfile, Look = stub }, "LrGenius E11b prepare", false)
		end)
		local settings, readErr = readSettings(copy)
		local state = lookState(settings)
		table.insert(data.attempts, { applied = X.summarizeLook(stub), ok = ok, error = err or readErr, after = state })
		data.after, data.error = state, err or readErr
		if ok and not readErr and state.look ~= nil and state.look.Name ~= currentName then
			return true, data
		end
	end
	return false, data
end

local function runE11Variant(exp, catalog, photo, variant, ctx, progress)
	local label = photoLabel(photo)
	progress:setCaption("E11: " .. label .. " " .. variant.id)
	local look, source, skip
	if variant.id == "E11b" then
		look, source, skip = e11FullLook(ctx)
	else
		local candidate = X.pickLook(X.STUB_LOOKS, ctx.currentName)
		if candidate then
			look, source = X.stubLook(candidate, variant.bare), { kind = "stub", profile = candidate.Name }
		else
			skip = "every candidate profile is already the photo's Look"
		end
	end
	if skip == CANCELED_MESSAGE or progress:isCanceled() then
		-- A scan cut short proves nothing; say nothing.
		return nil
	end
	if skip then
		table.insert(exp.verdicts, string.format("%s %s: skipped - %s", label, variant.id, skip))
		return nil
	end

	local copy, copyErr = createVirtualCopy(catalog, photo, "LrG Exp " .. variant.id)
	if not copy then
		addStep(exp, label, variant.id .. " create virtual copy", false, copyErr, nil)
		table.insert(exp.verdicts, string.format("%s %s: skipped - %s", label, variant.id, tostring(copyErr)))
		return nil
	end
	local copyLabel = photoLabel(copy)
	if source.ownLook then
		local switched, prepared = prepareOwnLook(catalog, copy, ctx.currentName, ctx.cameraProfile)
		source.prepare = prepared
		if not switched then
			addStep(
				exp,
				copyLabel,
				variant.id .. " prepare: switch away from the photo's Look",
				false,
				prepared.error,
				prepared
			)
			table.insert(
				exp.verdicts,
				string.format(
					"%s %s: inconclusive - the copy could not be switched away from %q first (%s), so writing that Look back cannot show an effect",
					label,
					variant.id,
					tostring(ctx.currentName),
					prepared.error and tostring(prepared.error) or X.describeLookState(prepared.after)
				)
			)
			return nil
		end
	end

	local beforeSettings, beforeErr = readSettings(copy)
	local ok, err = withWrite(catalog, "LrGenius experiment " .. variant.id, function()
		copy:applyDevelopSettings({ CameraProfile = ctx.cameraProfile, Look = look }, "LrGenius " .. variant.id, false)
	end)
	local afterSettings, afterErr = readSettings(copy)
	local readErr = beforeErr or afterErr
	-- Summaries only: `Parameters` is Adobe's profile definition and stays out
	-- of the report.
	local applied = { CameraProfile = ctx.cameraProfile, look = X.summarizeLook(look) }
	local before, after = lookState(beforeSettings), lookState(afterSettings)
	addStep(
		exp,
		copyLabel,
		string.format("%s apply %s Look %q", variant.id, variant.kind, tostring(look.Name)),
		ok and not readErr,
		err or readErr,
		{ applied = applied, source = source, before = before, after = after }
	)
	table.insert(
		exp.verdicts,
		string.format(
			"%s %s %s Look %q (source: %s) on CameraProfile=%s, from %s: %s",
			label,
			variant.id,
			variant.kind,
			tostring(look.Name),
			tostring(source.kind == "stub" and ("stub of " .. tostring(source.profile)) or source.kind),
			tostring(ctx.cameraProfile),
			X.describeLookState(before),
			X.describeLookOutcome(applied, before, after, ok, err, readErr)
		)
	)
	return ok and not readErr and afterSettings.Look or nil
end

local function runE11OnPhoto(exp, catalog, photo, masters, getPresetScan, progress)
	local label = photoLabel(photo)
	local isRaw, format, family = isRawFile(photo)
	if not isRaw then
		table.insert(
			exp.verdicts,
			string.format(
				"%s (%s, white-balance family %s): E11 skipped - it tests Adobe Raw Looks only, which need a raw file; creative Looks on non-raw files are not tested",
				label,
				tostring(format),
				tostring(family or "unknown")
			)
		)
		return
	end
	-- The master decides which Look to write: every copy starts from it, and
	-- a Look the photo already has would make "honoured" and "ignored" look
	-- the same.
	local masterSettings, masterErr = readSettings(photo)
	if masterErr then
		table.insert(exp.verdicts, label .. ": E11 skipped - " .. masterErr)
		return
	end
	local ctx = {
		currentName = type(masterSettings.Look) == "table" and masterSettings.Look.Name or nil,
		masterLook = masterSettings.Look,
		camera = cameraModel(photo),
		cameraProfile = X.e11CameraProfile(masterSettings.CameraProfile),
		donors = {},
		getPresetScan = getPresetScan,
	}
	for _, other in ipairs(masters) do
		if other.photo ~= photo then
			table.insert(ctx.donors, other.donor)
		end
	end

	for _, variant in ipairs(E11_VARIANTS) do
		if progress:isCanceled() then
			return
		end
		local readBack = runE11Variant(exp, catalog, photo, variant, ctx, progress)
		if variant.id == "E11a" then
			ctx.e11aLook = readBack
		end
	end
end

local function runE11(catalog, photos, progress, sink)
	local exp = newExperiment(
		sink,
		"E11",
		"Look transfer via applyDevelopSettings",
		"Does applyDevelopSettings set an Adobe Raw profile Look - as a stub in the form presets carry it (E11a, Stubbed=true), as a complete Look with Parameters taken from a photo (E11b), and as a bare stub without the Stubbed flag (E11c) - and does Lightroom fill in a stub's Parameters?"
	)
	-- Every selected photo's Look, read once: the donors for E11b.
	local masters = {}
	for _, photo in ipairs(photos) do
		local settings, err = readSettings(photo)
		if not err then
			table.insert(masters, {
				photo = photo,
				donor = { photo = photoLabel(photo), camera = cameraModel(photo), look = settings.Look },
			})
		end
	end
	-- The preset scan is a last resort: one scan for the whole run, and only
	-- when no photo offers a full Look.
	local scan
	local function getPresetScan()
		if scan == nil then
			progress:setCaption("E11: reading develop presets for a full Look")
			scan = findFullLooks(progress)
			if not scan.canceled then
				local found = {}
				for _, entry in ipairs(scan.found) do
					table.insert(
						found,
						{ preset = entry.preset, folder = entry.folder, look = X.summarizeLook(entry.look) }
					)
				end
				-- Every read failing is a broken scan, not a finding.
				local allFailed = scan.scanned > 0 and scan.failedReads == scan.scanned
				addStep(
					exp,
					"(global)",
					"E11b scan develop presets for a Look with Parameters",
					scan.error == nil and not allFailed,
					scan.error or (allFailed and ("every preset read failed: " .. tostring(scan.firstError))) or nil,
					{
						presetsRead = scan.scanned,
						failedReads = scan.failedReads,
						firstError = scan.firstError,
						stubbedLookKeys = scan.stubKeys,
						found = found,
					}
				)
			end
		end
		return scan
	end

	for _, photo in ipairs(photos) do
		if progress:isCanceled() then
			break
		end
		runE11OnPhoto(exp, catalog, photo, masters, getPresetScan, progress)
	end
	table.insert(
		exp.manualChecks,
		"Profile browser of each 'LrG Exp E11a/E11b/E11c' copy: which profile is active, and does the photo look like it rather than plain Adobe Standard?"
	)
	table.insert(
		exp.manualChecks,
		"History panel of an 'LrG Exp E11' copy: is it one step named 'LrGenius E11a' / 'E11b' / 'E11c'? (An E11b copy that used the photo's own Look has an 'LrGenius E11b prepare' step before it.)"
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
	props.runE11 = true
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
			f:checkbox({
				value = bind("runE1"),
				title = "E1  White balance: Temp vs Temperature, mode without values",
			}),
			f:checkbox({
				value = bind("runE11"),
				title = "E11  Look transfer: stubs and a full Look via applyDevelopSettings (raw only)",
			}),
			f:checkbox({ value = bind("runE2"), title = "E2  Add AI masks via applyDevelopSettings" }),
			f:checkbox({
				value = bind("runE4"),
				title = "E4  Plugin presets: overwrite, merge, re-apply, amount, delete after apply",
			}),
		}),
		f:static_text({
			title = "E1, E11, E2 and E4 write only to new virtual copies named 'LrG Exp ...'.\n"
				.. "They switch Lightroom to the Library module and change the selection.\n"
				.. "E4 leaves hidden plugin presets in Lightroom's preset folder. They are\n"
				.. "shared by all catalogs, are not removed with the test catalog, and the\n"
				.. "SDK cannot delete them. E4f deletes the files of its own preset to test\n"
				.. "exactly that; the report lists every preset file that is left.",
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
		E11 = props.runE11,
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
		-- The first few, plus a headline (the E4f answer) wherever it is.
		local shown, hidden = X.summaryVerdicts(experiment.verdicts, 4)
		for _, verdict in ipairs(shown) do
			table.insert(lines, "  " .. verdict)
		end
		if hidden > 0 then
			table.insert(lines, string.format("  ... and %d more in the report", hidden))
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

		local writes = options.E1 or options.E11 or options.E2 or options.E4
		if not (writes or options.E13) then
			return
		end
		if writes and not options.testCatalog then
			LrDialogs.message(
				"Confirm the test catalog",
				"E1, E11, E2 and E4 create virtual copies, and E4 plugin presets. Run them in a test catalog and tick the confirmation box, or run E13 on its own.",
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
				runE13(catalog, stills, report.experiments)
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
			if options.E11 and not progress:isCanceled() then
				runE11(catalog, stills, progress, report.experiments)
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
