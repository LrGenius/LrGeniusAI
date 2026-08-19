local function showCullDialog(ctx)
	local f = LrView.osFactory()
	local bind = LrView.bind
	local share = LrView.share

	local props = LrBinding.makePropertyTable(ctx)
	props.scope = prefs.cullScope or "selected"
	props.timeDeltaSeconds = prefs.cullTimeDeltaSeconds or 2
	props.cullingPreset = prefs.cullPreset or "default"
	props.createDuplicatesCollection = prefs.cullCreateDuplicatesCollection ~= false

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/CullTask/ScopeGroup=Scope"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/CullTask/ScopeLabel=Apply to:"),
					width = share("labelWidth"),
				}),
				f:popup_menu({
					value = bind("scope"),
					items = {
						{ title = LOC("$$$/LrGeniusAI/common/ScopeSelected=Selected photos only"), value = "selected" },
						{ title = LOC("$$$/LrGeniusAI/common/ScopeView=Current view"), value = "view" },
					},
					width = 260,
				}),
			}),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/CullTask/OptionsGroup=Options"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/CullTask/TimeDeltaLabel=Burst time window (seconds):"),
					width = share("labelWidth"),
				}),
				f:combo_box({
					value = bind("timeDeltaSeconds"),
					items = {
						{ title = "1", value = 1 },
						{ title = "2", value = 2 },
						{ title = "3", value = 3 },
						{ title = "5", value = 5 },
					},
					width = 120,
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/CullTask/PresetLabel=Culling preset:"),
					width = share("labelWidth"),
				}),
				f:popup_menu({
					value = bind("cullingPreset"),
					items = {
						{ title = LOC("$$$/LrGeniusAI/CullTask/PresetDefault=Default (balanced)"), value = "default" },
						{
							title = LOC("$$$/LrGeniusAI/CullTask/PresetPortrait=Portrait (face-focused)"),
							value = "portrait",
						},
						{
							title = LOC("$$$/LrGeniusAI/CullTask/PresetStreet=Street (technical-focused)"),
							value = "street",
						},
						{
							title = LOC("$$$/LrGeniusAI/CullTask/PresetEvent=Event (people + moments)"),
							value = "event",
						},
						{
							title = LOC("$$$/LrGeniusAI/CullTask/PresetSports=Sports (motion-tolerant)"),
							value = "sports",
						},
					},
					width = 260,
				}),
			}),
			f:checkbox({
				value = bind("createDuplicatesCollection"),
				title = LOC(
					"$$$/LrGeniusAI/CullTask/CreateDuplicates=Create 'Duplicates / Near Duplicates' collection"
				),
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/CullTask/WindowTitle=Cull Similar Photos"),
		contents = contents,
		actionVerb = LOC("$$$/LrGeniusAI/CullTask/Run=Cull"),
		cancelVerb = LOC("$$$/LrGeniusAI/common/Cancel=Cancel"),
	})

	if result ~= "ok" then
		return nil
	end

	prefs.cullScope = props.scope
	prefs.cullTimeDeltaSeconds = props.timeDeltaSeconds
	prefs.cullPreset = props.cullingPreset
	prefs.cullCreateDuplicatesCollection = props.createDuplicatesCollection

	return {
		scope = props.scope,
		timeDeltaSeconds = props.timeDeltaSeconds,
		cullingPreset = props.cullingPreset,
		createDuplicatesCollection = props.createDuplicatesCollection,
	}
end

local function dedupePhotoIds(photoIds)
	local result = {}
	local seen = {}
	for _, photoId in ipairs(photoIds or {}) do
		if photoId and not seen[photoId] then
			table.insert(result, photoId)
			seen[photoId] = true
		end
	end
	return result
end

local function photosFromIds(photoIds, photoById)
	local photos = {}
	for _, photoId in ipairs(dedupePhotoIds(photoIds)) do
		local photo = photoById[photoId]
		if photo then
			table.insert(photos, photo)
		else
			log:warn("Cull task: photo not found in catalog for photo_id " .. tostring(photoId))
		end
	end
	return photos
end

---
-- Runs the fast cull-only ingest for photos the backend has no culling data
-- for. Sends `tasks = {"cull"}`, which computes exactly what culling reads —
-- pHash, image metrics and face quality — and skips the SigLIP2 embedding and
-- the LLM. That is the difference between seconds and tens of minutes on a
-- freshly imported shoot.
--
-- Originals are sent by path when the backend is on this machine; otherwise
-- each photo is exported to a temporary JPEG and uploaded, and the temp file
-- is removed afterwards.
-- @return number|nil processed count, string|nil error, string|nil warnings
local function cullPrepare(missingIds, photoById, progressScope)
	local total = #missingIds
	local processed = 0
	local failures = 0
	local byReference = SearchIndexAPI.isLocalBackend()
	-- A photo that indexes successfully can still have lost a signal culling
	-- reads — a face model that is not downloaded yet is the common one, and
	-- it costs every photo its face-quality score. Dropping these on the floor
	-- meant the run looked clean and the picks were quietly worse.
	local warningsSeen = {}
	local warningsList = {}
	local warningsTotal = 0

	-- `warningsTotal` counts occurrences so the dialog can say "and N more",
	-- while `warningsList` holds each distinct message once.
	--
	-- The backend's request-level `warnings` is the union of the per-photo
	-- lists, so counting an occurrence from both would report roughly twice as
	-- many as happened. Per-photo entries are counted each time; a request-level
	-- one is only counted if nothing else has already reported it, which keeps
	-- a genuinely request-scoped warning from being dropped.
	local function noteWarnings(list, countEach)
		if type(list) ~= "table" then
			return
		end
		for _, w in ipairs(list) do
			local seen = warningsSeen[w]
			if not seen then
				warningsSeen[w] = true
				table.insert(warningsList, w)
			end
			if countEach or not seen then
				warningsTotal = warningsTotal + 1
			end
		end
	end

	-- Rebuilt per photo rather than hoisted: exposure_bias is the one per-photo
	-- field this fast path carries, and it is what lets the backend recognise
	-- an exposure bracket instead of nominating most of it for deletion.
	-- Sharing one table across the loop would give every photo the first one's
	-- compensation.
	local function indexOptionsFor(photo)
		local indexOptions = { tasks = { "cull" }, regenerate_metadata = false }
		local exposureBias = photo:getRawMetadata("exposureBias")
		if type(exposureBias) == "number" then
			indexOptions.exposure_bias = exposureBias
		end
		return indexOptions
	end

	-- One photo on its own. Also the fallback for anything a group did not
	-- come back with, so a grouped request that fails outright costs nothing
	-- beyond the retry.
	local function prepareOne(photoId, photo)
		-- `result` is the decoded response on success and an error string on
		-- failure; both matter here, so it is read either way.
		local ok, result
		if byReference then
			local path = photo:getRawMetadata("path")
			ok, result = SearchIndexAPI.analyzeAndIndexPhotoByReference(photoId, path, indexOptionsFor(photo))
		else
			local exported = SearchIndexAPI.exportPhotoForIndexing(photo)
			if exported then
				ok, result = SearchIndexAPI.analyzeAndIndexPhoto(photoId, exported, indexOptionsFor(photo))
				LrFileUtils.delete(exported)
			else
				ok, result = false, "could not export photo for upload"
			end
		end
		if ok then
			if type(result) == "table" then
				-- A single-photo request: its request-level list is that
				-- photo's own warnings.
				noteWarnings(result.warnings, true)
			end
			return true
		end
		log:warn("Cull prepare failed for " .. tostring(photoId) .. ": " .. tostring(result))
		return false
	end

	-- `tasks = {"cull"}` never reaches an LLM, so the group is sized for the
	-- server's decode throughput rather than a context window. Grouping only
	-- pays off when the backend reads the originals itself; the upload path
	-- still sends one exported JPEG at a time.
	local groupSize = Util.groupedBatchSize(nil, byReference, byReference, prefs and prefs.indexBatchSize, false)

	local index = 1
	while index <= total do
		if progressScope and progressScope:isCanceled() then
			return processed, nil
		end

		-- Photos the catalog no longer has still advance the counter, so the
		-- progress bar stays tied to position in `missingIds`.
		local chunk = {}
		while #chunk < groupSize and index <= total do
			local photoId = missingIds[index]
			local photo = photoById[photoId]
			if photo then
				table.insert(chunk, { photoId = photoId, photo = photo })
			end
			index = index + 1
		end

		local outcomes = {}
		if byReference and #chunk > 1 then
			local entries = {}
			for _, item in ipairs(chunk) do
				local path = item.photo:getRawMetadata("path")
				if path then
					table.insert(entries, {
						photoId = item.photoId,
						filePath = path,
						options = indexOptionsFor(item.photo),
					})
				end
			end
			if #entries > 0 then
				if progressScope then
					progressScope:setCaption(
						LOC(
							"$$$/LrGeniusAI/CullTask/PrepProgress=Preparing ^1/^2...",
							tostring(index - 1),
							tostring(total)
						)
					)
				end
				local ok, response = SearchIndexAPI.analyzeAndIndexPhotosByReference(entries, {
					tasks = { "cull" },
					regenerate_metadata = false,
				})
				-- A transport-level failure has no per-photo detail, so every
				-- photo in the chunk falls through to its own request rather
				-- than being written off on the group's behalf.
				if type(response) == "table" then
					noteWarnings(response.warnings, false)
					outcomes = Util.resultsByPhotoId(response.results)
				else
					log:warn(
						"Grouped cull prepare failed (" .. tostring(response) .. "); falling back to single photos"
					)
				end
				if not ok then
					log:warn("Grouped cull prepare reported failures; affected photos fall back to single sends")
				end
			end
		end

		for _, item in ipairs(chunk) do
			local outcome = outcomes[item.photoId]
			if outcome == nil then
				if prepareOne(item.photoId, item.photo) then
					processed = processed + 1
				else
					failures = failures + 1
				end
			elseif outcome.success then
				processed = processed + 1
				noteWarnings(outcome.warnings, true)
			else
				failures = failures + 1
				log:warn("Cull prepare failed for " .. tostring(item.photoId) .. ": " .. tostring(outcome.error))
			end
		end

		if progressScope then
			progressScope:setPortionComplete(index - 1, total)
			progressScope:setCaption(
				LOC("$$$/LrGeniusAI/CullTask/PrepProgress=Preparing ^1/^2...", tostring(index - 1), tostring(total))
			)
		end
		LrTasks.yield()
	end

	-- Partial failure is not fatal: culling still runs on whatever succeeded,
	-- and the backend reports the rest via summary.unindexed_count.
	if processed == 0 and failures > 0 then
		return nil, LOC("$$$/LrGeniusAI/CullTask/PrepAllFailed=None of the photos could be prepared.")
	end
	-- One missing model warns once per photo, so the dialog shows a handful
	-- and counts the rest: a wall of identical lines is as unreadable as no
	-- message at all.
	local combinedWarnings
	if #warningsList > 0 then
		local shown = {}
		for i = 1, math.min(5, #warningsList) do
			table.insert(shown, warningsList[i])
		end
		combinedWarnings = table.concat(shown, "\n")
		if warningsTotal > #shown then
			combinedWarnings = combinedWarnings
				.. "\n"
				.. LOC("$$$/LrGeniusAI/common/MoreWarnings=... and ^1 more warnings", tostring(warningsTotal - #shown))
		end
	end
	return processed, nil, combinedWarnings
end

local function joinReasonCodes(reasonCodes)
	if type(reasonCodes) ~= "table" or #reasonCodes == 0 then
		return ""
	end
	return table.concat(reasonCodes, ", ")
end

local function formatMetric(value)
	if type(value) ~= "number" then
		return tostring(value or "")
	end
	return string.format("%.4f", value)
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskCullPhotos", function(context)
		if not Util.waitForServerDialog() then
			return
		end

		local options = showCullDialog(context)
		if not options then
			return
		end

		local photosToProcess, status = PhotoSelector.getPhotosInScope(options.scope)
		if not photosToProcess or #photosToProcess == 0 then
			if status == "Invalid view" then
				LrDialogs.message(
					LOC("$$$/LrGeniusAI/common/InvalidViewTitle=Invalid View"),
					LOC(
						"$$$/LrGeniusAI/common/InvalidViewMessage=The 'Current view' scope only works when a folder or collection is selected."
					)
				)
			else
				LrDialogs.message(
					LOC("$$$/LrGeniusAI/common/NoPhotosTitle=No Photos Found"),
					LOC("$$$/LrGeniusAI/common/NoPhotosMessage=No photos found in the selected scope.")
				)
			end
			return
		end

		local photoIds = {}
		local photoById = {}
		for _, photo in ipairs(photosToProcess) do
			local photoId, photoIdErr = SearchIndexAPI.getPhotoIdForPhoto(photo)
			if photoId then
				table.insert(photoIds, photoId)
				photoById[photoId] = photo
			else
				log:error("Cull task: skipping photo due to missing photo_id: " .. tostring(photoIdErr))
			end
		end

		if #photoIds == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/CullTask/NoPhotoIdsTitle=No usable photos"),
				LOC(
					"$$$/LrGeniusAI/CullTask/NoPhotoIdsMessage=No usable photo IDs could be computed for the selected photos."
				)
			)
			return
		end

		-- Pre-flight: the backend drops photos it has no record for, so an
		-- unanalyzed folder used to come back as an empty result with no
		-- explanation. Ask first, and offer the fast cull-only ingest, which
		-- computes just what culling reads (pHash, image metrics, face quality)
		-- and skips the embedding and the LLM entirely.
		local missing, missingErr = SearchIndexAPI.checkUnprocessedPhotoIds(photoIds, { "cull" })
		if missingErr then
			log:warn("Cull pre-flight check failed, continuing anyway: " .. tostring(missingErr))
		elseif missing and #missing > 0 then
			local answer = LrDialogs.confirm(
				LOC("$$$/LrGeniusAI/CullTask/NeedsPrepTitle=Some photos need preparing"),
				LOC(
					"$$$/LrGeniusAI/CullTask/NeedsPrepMessage=^1 of ^2 selected photos have no culling data yet and would be skipped. Prepare them now? This only computes culling signals, so it is much faster than a full Analyze & Index.",
					tostring(#missing),
					tostring(#photoIds)
				),
				LOC("$$$/LrGeniusAI/CullTask/NeedsPrepPrepare=Prepare now"),
				LOC("$$$/LrGeniusAI/CullTask/NeedsPrepSkip=Cull without them")
			)
			if answer == "ok" then
				-- Same gate as Analyze & Index, for the same reason: the prep
				-- pass scores face quality, and without the face model it
				-- produces photos culling can only grade on technical metrics.
				if not SearchIndexAPI.confirmModelsReadyForTasks({ "cull" }) then
					return
				end
				local prepScope = LrProgressScope({
					title = LOC("$$$/LrGeniusAI/CullTask/PrepProgressTitle=Preparing photos for culling..."),
					functionContext = context,
				})
				local prepared, prepErr, prepWarnings = cullPrepare(missing, photoById, prepScope)
				prepScope:done()
				if prepErr then
					ErrorHandler.handleError(LOC("$$$/LrGeniusAI/CullTask/PrepErrorTitle=Preparation failed"), prepErr)
					return
				end
				if prepWarnings then
					log:warn("Cull prepare warnings: " .. prepWarnings)
					LrDialogs.message(
						LOC("$$$/LrGeniusAI/CullTask/PrepWarningTitle=Some culling signals are missing"),
						LOC(
							"$$$/LrGeniusAI/CullTask/PrepWarningMessage=The photos were prepared, but not every signal could be computed. Culling will run without them.\n\n^1",
							prepWarnings
						),
						"warning"
					)
				end
				log:info("Cull prepare completed for " .. tostring(prepared) .. " photo(s)")
			end
		end

		local progressScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/CullTask/ProgressTitle=Culling similar photos..."),
			functionContext = context,
		})
		progressScope:setPortionComplete(0, 1)

		local cullResult, err = SearchIndexAPI.cullPhotos(photoIds, {
			phash_threshold = "auto",
			clip_threshold = "auto",
			time_delta_seconds = options.timeDeltaSeconds,
			culling_preset = options.cullingPreset,
		})
		local groups = cullResult and cullResult.groups or nil
		local summary = (cullResult and cullResult.summary) or {}

		progressScope:setPortionComplete(1, 1)
		progressScope:done()

		if err or type(groups) ~= "table" then
			ErrorHandler.handleError(
				LOC("$$$/LrGeniusAI/CullTask/ErrorTitle=Culling failed"),
				err or LOC("$$$/LrGeniusAI/CullTask/ErrorMessage=Could not create culling groups.")
			)
			return
		end

		if cullResult and cullResult.warning then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/common/BackendWarning=Backend Warning"),
				cullResult.warning,
				"warning"
			)
		end

		if #groups == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/CullTask/NoGroupsTitle=No groups found"),
				LOC("$$$/LrGeniusAI/CullTask/NoGroupsMessage=The selected photos could not be grouped for culling.")
			)
			return
		end

		local picksIds = {}
		local alternateIds = {}
		local rejectIds = {}
		local duplicateIds = {}
		local setIds = {}
		local nearDuplicateGroupCount = 0
		local setGroupCount = 0

		for _, group in ipairs(groups) do
			local winnerPhotoId = group["winner_photo_id"]
			local alternatePhotoIds = group["alternate_photo_ids"] or {}
			local rejectCandidatePhotoIds = group["reject_candidate_photo_ids"] or {}
			local groupType = group["group_type"]
			local groupPhotoIds = group["photo_ids"] or {}
			-- The backend flags brackets, focus stacks and panoramas as
			-- keep_all: every frame is part of one picture. It already returns
			-- an empty reject list for them, so nothing here can nominate one
			-- by accident; the collection exists so the user can see that these
			-- frames were recognised rather than just quietly not culled.
			local keepAll = group["keep_all"] == true

			if winnerPhotoId then
				table.insert(picksIds, winnerPhotoId)
			end
			for _, photoId in ipairs(alternatePhotoIds) do
				table.insert(alternateIds, photoId)
			end
			for _, photoId in ipairs(rejectCandidatePhotoIds) do
				table.insert(rejectIds, photoId)
			end
			if keepAll then
				setGroupCount = setGroupCount + 1
				for _, photoId in ipairs(groupPhotoIds) do
					table.insert(setIds, photoId)
				end
			elseif options.createDuplicatesCollection and groupType == "near_duplicate" then
				nearDuplicateGroupCount = nearDuplicateGroupCount + 1
				for _, photoId in ipairs(groupPhotoIds) do
					if photoId ~= winnerPhotoId then
						table.insert(duplicateIds, photoId)
					end
				end
			elseif groupType == "near_duplicate" then
				nearDuplicateGroupCount = nearDuplicateGroupCount + 1
			end
		end

		local picksPhotos = photosFromIds(picksIds, photoById)
		local alternatePhotos = photosFromIds(alternateIds, photoById)
		local rejectPhotos = photosFromIds(rejectIds, photoById)
		local duplicatePhotos = photosFromIds(duplicateIds, photoById)
		local setPhotos = photosFromIds(setIds, photoById)

		local catalog = LrApplication.activeCatalog()
		local timestamp = LrDate.timeToW3CDate(LrDate.currentTime())
		local resultSet = nil
		local picksCollection = nil

		local cullDataByPhotoId = {}
		for _, group in ipairs(groups) do
			local groupId = tostring(group["group_id"] or "")
			local groupType = tostring(group["group_type"] or "")
			local groupPhotos = group["photos"] or {}
			for _, photoResult in ipairs(groupPhotos) do
				local photoId = photoResult["photo_id"]
				if photoId then
					local metrics = photoResult["metrics"] or {}
					local decision = "alternate"
					if photoResult["winner"] then
						decision = "pick"
					elseif photoResult["reject_candidate"] then
						decision = "reject_candidate"
					end
					cullDataByPhotoId[photoId] = {
						decision = decision,
						groupId = groupId,
						groupType = groupType,
						groupRank = tostring(photoResult["rank"] or ""),
						groupWinner = photoResult["winner"] and "true" or "false",
						score = formatMetric(photoResult["cull_score"]),
						reasonCodes = joinReasonCodes(photoResult["reason_codes"]),
						explanation = tostring(photoResult["explanation"] or ""),
						sharpness = formatMetric(metrics["sharpness"]),
						exposure = formatMetric(metrics["exposure"]),
						noise = formatMetric(metrics["noise"]),
						technicalScore = formatMetric(metrics["technical_score"]),
						aesthetic = formatMetric(metrics["aesthetic"]),
						faceCount = tostring(metrics["face_count"] or ""),
						faceSharpness = formatMetric(metrics["face_sharpness"]),
						faceProminence = formatMetric(metrics["face_prominence"]),
						faceVisibility = formatMetric(metrics["face_visibility"]),
						faceScore = formatMetric(metrics["face_score"]),
						occlusion = formatMetric(metrics["occlusion"]),
						eyeOpenness = formatMetric(metrics["eye_openness"]),
						blinkPenalty = formatMetric(metrics["blink_penalty"]),
					}
				end
			end
		end

		catalog:withPrivateWriteAccessDo(function()
			for photoId, cullData in pairs(cullDataByPhotoId) do
				local photo = photoById[photoId]
				if photo then
					photo:setPropertyForPlugin(_PLUGIN, "cullDecision", cullData.decision)
					photo:setPropertyForPlugin(_PLUGIN, "cullGroupId", cullData.groupId)
					photo:setPropertyForPlugin(_PLUGIN, "cullGroupType", cullData.groupType)
					photo:setPropertyForPlugin(_PLUGIN, "cullGroupRank", cullData.groupRank)
					photo:setPropertyForPlugin(_PLUGIN, "cullGroupWinner", cullData.groupWinner)
					photo:setPropertyForPlugin(_PLUGIN, "cullScore", cullData.score)
					photo:setPropertyForPlugin(_PLUGIN, "cullReasonCodes", cullData.reasonCodes)
					photo:setPropertyForPlugin(_PLUGIN, "cullExplanation", cullData.explanation)
					photo:setPropertyForPlugin(_PLUGIN, "cullSharpness", cullData.sharpness)
					photo:setPropertyForPlugin(_PLUGIN, "cullExposure", cullData.exposure)
					photo:setPropertyForPlugin(_PLUGIN, "cullNoise", cullData.noise)
					photo:setPropertyForPlugin(_PLUGIN, "cullTechnicalScore", cullData.technicalScore)
					photo:setPropertyForPlugin(_PLUGIN, "cullAesthetic", cullData.aesthetic)
					photo:setPropertyForPlugin(_PLUGIN, "cullFaceCount", cullData.faceCount)
					photo:setPropertyForPlugin(_PLUGIN, "cullFaceSharpness", cullData.faceSharpness)
					photo:setPropertyForPlugin(_PLUGIN, "cullFaceProminence", cullData.faceProminence)
					photo:setPropertyForPlugin(_PLUGIN, "cullFaceVisibility", cullData.faceVisibility)
					photo:setPropertyForPlugin(_PLUGIN, "cullFaceScore", cullData.faceScore)
					photo:setPropertyForPlugin(_PLUGIN, "cullOcclusion", cullData.occlusion)
					photo:setPropertyForPlugin(_PLUGIN, "cullEyeOpenness", cullData.eyeOpenness)
					photo:setPropertyForPlugin(_PLUGIN, "cullBlinkPenalty", cullData.blinkPenalty)
				end
			end
		end, Defaults.catalogWriteAccessOptions)

		catalog:withWriteAccessDo("Create culling collections", function()
			resultSet = catalog:createCollectionSet(
				LOC("$$$/LrGeniusAI/CullTask/ResultSet=Culling Results @ ^1", timestamp),
				nil,
				true
			)

			local function createResultCollection(name, photos)
				local collection = catalog:createCollection(name, resultSet, false)
				if photos and #photos > 0 then
					collection:addPhotos(photos)
				end
				return collection
			end

			picksCollection = createResultCollection(LOC("$$$/LrGeniusAI/CullTask/Picks=Picks"), picksPhotos)
			createResultCollection(LOC("$$$/LrGeniusAI/CullTask/Alternates=Alternates"), alternatePhotos)
			createResultCollection(LOC("$$$/LrGeniusAI/CullTask/Rejects=Reject Candidates"), rejectPhotos)
			if options.createDuplicatesCollection then
				createResultCollection(
					LOC("$$$/LrGeniusAI/CullTask/Duplicates=Duplicates / Near Duplicates"),
					duplicatePhotos
				)
			end
			-- Only created when something was actually detected: an empty
			-- collection on every run would train the user to ignore it.
			if #setPhotos > 0 then
				createResultCollection(
					LOC("$$$/LrGeniusAI/CullTask/Sets=Brackets / Stacks / Panoramas (keep all)"),
					setPhotos
				)
			end
		end, Defaults.catalogWriteAccessOptions)

		if picksCollection then
			catalog:setActiveSources({ picksCollection })
			LrApplicationView.gridView()
		end

		-- The set line is appended rather than folded into the main message so
		-- that a run with no brackets or stacks reads exactly as it did before.
		local completionMessage = LOC(
			"$$$/LrGeniusAI/CullTask/CompletionMessage=Created culling collections for ^1 groups. Picks: ^2, Alternates: ^3, Reject candidates: ^4. Near-duplicate groups: ^5. Preset: ^6.",
			tostring(summary.group_count or #groups),
			tostring(summary.pick_count or #picksPhotos),
			tostring(summary.alternate_count or #alternatePhotos),
			tostring(summary.reject_candidate_count or #rejectPhotos),
			tostring(summary.near_duplicate_group_count or nearDuplicateGroupCount),
			tostring(summary.culling_preset or options.cullingPreset or "default")
		)
		local setGroups = summary.intentional_set_group_count or setGroupCount
		if setGroups and setGroups > 0 then
			completionMessage = completionMessage
				.. "\n\n"
				.. LOC(
					"$$$/LrGeniusAI/CullTask/CompletionSets=^1 group(s) covering ^2 photo(s) were recognised as exposure brackets, focus stacks or panoramas. Every frame in those was kept — none was suggested for rejection.",
					tostring(setGroups),
					tostring(summary.intentional_set_photo_count or #setPhotos)
				)
		end

		LrDialogs.message(LOC("$$$/LrGeniusAI/CullTask/CompletionTitle=Culling Complete"), completionMessage)
	end)
end)
