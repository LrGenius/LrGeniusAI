require("DevelopEditManager")
require("AestheticRefine")

-- AI Edit runs on the Style Engine only. The LLM edit path (`/v1/edit/recipe`, provider
-- and model choice, prompts, intent presets, creative controls, per-photo
-- instructions) is gone from this task: none of it reached the style engine,
-- which builds a recipe purely by matching the photo against the training
-- examples the user saved from their own edits. The backend still serves
-- `/v1/edit/recipe`, and `SearchIndexAPI.generateEditRecipePhoto` still speaks to it —
-- nothing here calls either.

-- Mirrors `MIN_TRAINING_EXAMPLES` in
-- `server-rs/crates/lrg-analysis/src/style_engine.rs`. Below it the style
-- engine declines to answer, and with the LLM fallback no longer requested
-- there is nothing behind it — so the run is stopped here rather than
-- exporting and uploading every photo for a guaranteed 422.
local MIN_TRAINING_EXAMPLES = 5

local function copyOptions(source)
	local copied = {}
	for key, value in pairs(source or {}) do
		copied[key] = value
	end
	return copied
end

-- Same wording and thresholds as the Style Profile block in the plug-in
-- manager (`PluginInfoDialogSections.lua`), reusing its `LOC` keys so the two
-- places cannot drift apart.
local function styleReadinessDisplay(stats)
	if not stats then
		return LOC(
			"$$$/LrGeniusAI/TaskAiEditPhotos/StyleStatusUnknown=Unavailable — the backend did not report the style profile"
		),
			LrColor(0.7, 0.7, 0.7),
			nil
	end

	local count = tonumber(stats.count) or 0
	local readiness = stats.readiness or "cold_start"

	if readiness == "active" then
		return LOC("$$$/LrGeniusAI/Training/Status/Active=Active — matching your style precisely"),
			LrColor(0.2, 0.8, 0.2),
			count
	elseif readiness == "limited" then
		return LOC("$$$/LrGeniusAI/Training/Status/Limited=Learning — getting better with more examples"),
			LrColor(0.8, 0.8, 0.2),
			count
	elseif readiness == "warming_up" then
		return LOC("$$$/LrGeniusAI/Training/Status/WarmingUp=Building up — ^1 of 10 examples saved", tostring(count)),
			LrColor(0.8, 0.4, 0.1),
			count
	end
	return LOC("$$$/LrGeniusAI/Training/Status/ColdStart=Not started — save some edited photos to begin"),
		LrColor(0.7, 0.7, 0.7),
		count
end

-- `catalog:createVirtualCopies` copies whatever is *selected* — it takes no
-- photo argument — so the photo has to be selected first, and the selection has
-- to be verified before anything is created. A photo that is not in the current
-- source cannot be selected, and copying whatever was selected instead would be
-- much worse than not copying at all.
--
-- Selecting works without a write gate (`focusPhotoInDevelop` in
-- `DevelopEditManager` does the same); only creating the copy needs one. The
-- switch to All Photographs is the same fallback that function uses, for the
-- same reason: the scope may reach outside the folder or collection on screen.
local function isOnlySelected(catalog, photo)
	local selected = catalog:getTargetPhotos()
	return type(selected) == "table" and #selected == 1 and selected[1] == photo
end

local function selectOnly(catalog, photo)
	local ok = LrTasks.pcall(function()
		catalog:setSelectedPhotos(photo, { photo })
	end)
	if not ok then
		return false
	end
	return isOnlySelected(catalog, photo)
end

local function localIdOf(photo)
	return photo and tostring(photo.localIdentifier) or "nil"
end

-- Returns `copy, nil, 0` on success, or `nil, err, strayCount` on failure.
-- `strayCount` is how many copies Lightroom may already have created before
-- the failure was detected: the SDK cannot delete a photo, so those copies stay
-- in the catalog and the user has to be told to remove them.
local function createVirtualCopyFor(photo)
	local catalog = LrApplication.activeCatalog()

	if not selectOnly(catalog, photo) then
		local switched = LrTasks.pcall(function()
			catalog:setActiveSources({ catalog.kAllPhotos })
			LrTasks.sleep(0.2)
		end)
		if not switched or not selectOnly(catalog, photo) then
			return nil, "the photo could not be selected in Lightroom", 0
		end
	end

	local copies
	local selectionChanged = false
	local ok, err = LrTasks.pcall(function()
		catalog:withWriteAccessDo(
			LOC("$$$/LrGeniusAI/TaskAiEditPhotos/VirtualCopyUndo=Create virtual copy for AI edit"),
			function()
				-- Waiting for write access can take up to the gate's timeout,
				-- and the user may click in the grid meanwhile. The copy is
				-- made from whatever is selected *now*, so check again here.
				if not isOnlySelected(catalog, photo) then
					selectionChanged = true
					return
				end
				copies = catalog:createVirtualCopies(LOC("$$$/LrGeniusAI/TaskAiEditPhotos/VirtualCopyName=AI Edit"))
			end,
			Defaults.catalogWriteAccessOptions
		)
	end)
	if not ok then
		return nil, tostring(err), type(copies) == "table" and #copies or 0
	end
	if selectionChanged then
		return nil, "the selection in Lightroom changed before the copy could be made", 0
	end
	if type(copies) ~= "table" or #copies ~= 1 then
		return nil, "Lightroom did not return exactly one virtual copy", type(copies) == "table" and #copies or 0
	end
	-- A copy of a virtual copy belongs to that copy's master, not to the copy.
	local isVirtualCopy = photo:getRawMetadata("isVirtualCopy")
	local sourceMaster = isVirtualCopy and photo:getRawMetadata("masterPhoto") or nil
	local copyMaster = copies[1]:getRawMetadata("masterPhoto")
	if not Util.isVirtualCopyOf(copyMaster, photo, isVirtualCopy, sourceMaster) then
		-- Catalog-local ids only, so a failure can be diagnosed from the log.
		log:error(
			"Virtual copy check failed: source="
				.. localIdOf(photo)
				.. " isVirtualCopy="
				.. tostring(isVirtualCopy)
				.. " sourceMaster="
				.. localIdOf(sourceMaster)
				.. " copyMaster="
				.. localIdOf(copyMaster)
		)
		return nil, "Lightroom copied a different photo than the one selected", 1
	end
	return copies[1], nil, 0
end

-- Refinement scores every candidate with the aesthetic model, which is a
-- download of its own. Asked once, before the run, rather than failing the same
-- way on every photo. Never blocks on its own failure: a status call that does
-- not answer lets the run go on, and each photo then says what went wrong.
--
-- @return string "ok" to go on as asked, "other" to go on without refining,
--   "cancel" to stop.
local function aestheticModelPreflight()
	local status, err = SearchIndexAPI.getAssetStatus()
	if not status or type(status.families) ~= "table" then
		log:warn("AI Edit aesthetic model preflight skipped: " .. tostring(err or "no families in /v1/models/assets"))
		return "ok"
	end
	if #Util.missingModelFamilies(status.families, { "aesthetic" }) == 0 then
		return "ok"
	end
	local answer = LrDialogs.confirm(
		"The aesthetic model is not downloaded yet",
		'Refining an edit scores it with the aesthetic model, which is not on disk yet. Run "Download AI models" in Plug-in Manager to get it, or carry on with plain edits.',
		"Edit without refinement",
		"Cancel"
	)
	return answer == "ok" and "other" or "cancel"
end

local function refinementSummary(refinedCount, reducedCount)
	return "Refined with the aesthetic score: "
		.. tostring(refinedCount)
		.. " photo(s), "
		.. tostring(reducedCount)
		.. " of them at a gentler strength than the full recipe."
end

local function showAiEditDialog(ctx, stats)
	log:trace("showAiEditDialog: start")
	local f = LrView.osFactory()
	local bind = LrView.bind
	local share = LrView.share
	local props = LrBinding.makePropertyTable(ctx)

	props.scope = prefs.aiEditScope or "selected"
	props.reviewBeforeApply = prefs.aiEditReviewBeforeApply ~= false
	props.createVirtualCopy = prefs.aiEditCreateVirtualCopy == true
	props.refineWithAesthetic = prefs.aiEditRefineAesthetic == true

	local readinessText, readinessColor, trainingCount = styleReadinessDisplay(stats)

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/common/Scope=Scope"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/common/ApplyTo=Apply to:"),
					width = share("labelWidth"),
				}),
				f:popup_menu({
					value = bind("scope"),
					width = 300,
					items = {
						{ title = LOC("$$$/LrGeniusAI/common/ScopeSelected=Selected photos only"), value = "selected" },
						{ title = LOC("$$$/LrGeniusAI/common/ScopeView=Current view"), value = "view" },
						{ title = LOC("$$$/LrGeniusAI/common/ScopeAll=All photos in catalog"), value = "all" },
					},
				}),
			}),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/StyleProfile=Style profile"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/TrainingExamples=Training examples:"),
					width = share("labelWidth"),
				}),
				f:static_text({
					title = trainingCount and tostring(trainingCount) or "—",
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/StyleStatus=Status:"),
					width = share("labelWidth"),
				}),
				f:static_text({
					title = readinessText,
					text_color = readinessColor,
				}),
			}),
			f:row({
				f:static_text({
					title = LOC(
						"$$$/LrGeniusAI/TaskAiEditPhotos/StyleProfileHint=Edits are built from your own saved edits — the more examples, the closer the match."
					),
				}),
			}),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/common/Options=Options"),
			fill_horizontal = 1,
			f:row({
				f:checkbox({
					value = bind("reviewBeforeApply"),
				}),
				f:static_text({
					title = LOC(
						"$$$/LrGeniusAI/TaskAiEditPhotos/ReviewProposed=Review each proposed edit before applying it"
					),
				}),
			}),
			f:row({
				f:checkbox({
					value = bind("createVirtualCopy"),
				}),
				f:static_text({
					title = LOC(
						"$$$/LrGeniusAI/TaskAiEditPhotos/CreateVirtualCopy=Apply the edit to a new virtual copy (leaves the original untouched)"
					),
				}),
			}),
			f:row({
				f:checkbox({
					value = bind("refineWithAesthetic"),
				}),
				f:static_text({
					title = "Refine each edit with the aesthetic score (slower: tries gentler strengths)",
				}),
			}),
			f:row({
				f:static_text({
					title = "Every try is a step in the Edit History; a virtual copy keeps the original's clean.",
					text_color = LrColor(0.5, 0.5, 0.5),
				}),
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/DialogTitle=AI Edit Photos in Lightroom"),
		contents = contents,
		actionVerb = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/GenerateEdits=Generate edits"),
	})
	log:trace("showAiEditDialog: dialog result=" .. tostring(result))

	if result ~= "ok" then
		return nil
	end

	prefs.aiEditScope = props.scope
	prefs.aiEditReviewBeforeApply = props.reviewBeforeApply
	prefs.aiEditCreateVirtualCopy = props.createVirtualCopy
	prefs.aiEditRefineAesthetic = props.refineWithAesthetic

	return {
		scope = props.scope,
		reviewBeforeApply = props.reviewBeforeApply,
		createVirtualCopy = props.createVirtualCopy,
		refineWithAesthetic = props.refineWithAesthetic,
	}
end

local function enrichPhotoOptions(photo, baseOptions)
	log:trace("enrichPhotoOptions: start for " .. tostring(photo and photo:getFormattedMetadata("fileName") or "nil"))
	local photoOptions = copyOptions(baseOptions)

	local datetime = photo:getRawMetadata("dateTime")
	if datetime ~= nil and type(datetime) == "number" then
		photoOptions.date_time = LrDate.timeToW3CDate(datetime)
		photoOptions.capture_time = datetime -- feeds the style engine's time-of-day bucket
	end

	-- EXIF context for style matching (focal length, capture time, camera, ISO).
	local exif = Util.getPhotoExif(photo)
	for k, v in pairs(exif) do
		photoOptions[k] = v
	end

	-- The backend receives a normalised JPEG, so the original encoding is no
	-- longer visible in the bytes it gets. Lightroom knows, and the edit
	-- guardrails need it: blown highlights are recoverable on a raw file and
	-- gone on a rendered one, which changes how far the white point may go. It
	-- also decides which training examples' white balance may be carried over.
	--
	-- Taken from the photo's develop settings, not only its file format, so it
	-- matches the white-balance keys DevelopEditManager will write: a DNG
	-- converted from a JPEG is a DNG, yet has a rendered file's white balance.
	-- Left absent when neither source knows.
	local wbFamily = DevelopEditManager.photoWhiteBalanceFamily(photo)
	if wbFamily ~= nil then
		photoOptions.is_raw = wbFamily == "raw"
	end

	return photoOptions
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("AiEditPhotosTask", function(ctx)
		LrDialogs.attachErrorDialogToFunctionContext(ctx)
		log:info("AI Edit task started")

		-- No `requireProviders` any more: the style engine needs no LLM
		-- provider, so a missing API key must not block this task.
		if not Util.waitForServerDialog() then
			log:warn("AI Edit task aborted: backend server unavailable")
			return
		end

		-- A backend that could not answer is not a backend reporting zero: fall
		-- through and let the per-photo errors say what actually went wrong,
		-- rather than telling the user their style profile is empty when it may
		-- be full.
		local stats, statsErr = SearchIndexAPI.getTrainingStats()
		if not stats then
			log:warn("AI Edit could not read the style profile stats: " .. tostring(statsErr))
		end
		local trainingCount = (stats and tonumber(stats.count)) or 0
		if stats and trainingCount < MIN_TRAINING_EXAMPLES then
			log:warn("AI Edit task aborted: only " .. tostring(trainingCount) .. " training example(s) available")
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/TaskAiEditPhotos/NoStyleProfileTitle=Style profile not ready"),
				LOC(
					"$$$/LrGeniusAI/TaskAiEditPhotos/NoStyleProfile=AI Edit builds every recipe from your own saved edits, and it needs at least ^1 training examples to do that. You currently have ^2.\n\nEdit a handful of photos the way you like them, then run Library → Plug-in Extras → Save Edits as AI Training Examples and start AI Edit again.",
					tostring(MIN_TRAINING_EXAMPLES),
					tostring(trainingCount)
				),
				"info"
			)
			return
		end

		local options = showAiEditDialog(ctx, stats)
		if not options then
			log:info("AI Edit task canceled by user in options dialog")
			return
		end
		log:trace(
			"AI Edit options selected: scope="
				.. tostring(options.scope)
				.. " review="
				.. tostring(options.reviewBeforeApply)
				.. " virtualCopy="
				.. tostring(options.createVirtualCopy)
				.. " refine="
				.. tostring(options.refineWithAesthetic)
		)

		if options.refineWithAesthetic then
			local answer = aestheticModelPreflight()
			if answer == "cancel" then
				log:info("AI Edit task canceled at the aesthetic model preflight")
				return
			elseif answer == "other" then
				options.refineWithAesthetic = false
			end
		end

		local photos = PhotoSelector.getPhotosInScope(options.scope)
		if not photos or #photos == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/common/NoPhotosTitle=No Photos"),
				LOC("$$$/LrGeniusAI/common/NoPhotosInScope=No photos found in the selected scope."),
				"info"
			)
			log:warn("AI Edit task found no photos in scope: " .. tostring(options.scope))
			return
		end

		local progressScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/ProgressTitle=Generating AI Lightroom edits..."),
			functionContext = ctx,
		})
		progressScope:setPortionComplete(0, #photos)

		local successCount = 0
		local skippedCount = 0
		local errorCount = 0
		local errorMessages = {}
		-- One entry per distinct text with a photo count: a cause that hits
		-- every photo in the run (white balance on JPEGs, say) is reported
		-- once, not once per photo.
		local warningTally = Util.newWarningTally()
		local refinedCount = 0
		local reducedCount = 0
		-- "AI Edit" virtual copies left in the catalog by an attempt that then
		-- failed (the check, persisting the recipe, or applying it).
		local strayCopyCount = 0

		local canceled = false

		for index, photo in ipairs(photos) do
			if progressScope:isCanceled() then
				canceled = true
				break
			end

			local fileName = photo:getFormattedMetadata("fileName") or "Photo"
			progressScope:setCaption(
				"Processing " .. fileName .. " (" .. tostring(index) .. " of " .. tostring(#photos) .. ")"
			)
			progressScope:setPortionComplete(index - 1, #photos)
			local continueProcessing = true

			log:trace("AI Edit photo loop start: index=" .. tostring(index) .. " photo=" .. tostring(fileName))

			local photoId, photoIdErr = SearchIndexAPI.getPhotoIdForPhoto(photo)
			if not photoId then
				log:error("Failed to resolve photo ID for " .. fileName .. ": " .. tostring(photoIdErr))
				table.insert(errorMessages, fileName .. ": " .. tostring(photoIdErr))
				errorCount = errorCount + 1
				continueProcessing = false
			else
				log:trace("Resolved photo ID for " .. fileName .. ": " .. tostring(photoId))
			end

			local response = nil
			local baselineScore = nil
			if continueProcessing then
				local photoOptions = enrichPhotoOptions(photo, options)
				local exportedPath = SearchIndexAPI.exportPhotoForIndexing(photo)
				if not exportedPath then
					log:error("Failed to export photo for AI edit generation: " .. fileName)
					table.insert(errorMessages, fileName .. ": export failed")
					errorCount = errorCount + 1
					continueProcessing = false
				end

				if continueProcessing then
					log:trace("AI Edit calling API for " .. fileName .. " exportedPath=" .. tostring(exportedPath))
					local ok, apiOk, apiResponse = LrTasks.pcall(function()
						return SearchIndexAPI.styleEdit(photoId, exportedPath, photoOptions)
					end)
					-- The export is the photo as it is before the edit, so its score is what the
					-- refinement has to beat. A photo that cannot be scored is still edited, just
					-- not refined, and says so.
					if ok and apiOk and options.refineWithAesthetic then
						local okScore, score, scoreErr = LrTasks.pcall(function()
							return SearchIndexAPI.scoreEditedPhoto(exportedPath)
						end)
						if okScore and type(score) == "number" then
							baselineScore = score
						else
							local reason = okScore and Util.userFacingError(tostring(scoreErr)) or tostring(score)
							log:warn("AI Edit could not score " .. fileName .. " before editing: " .. reason)
							Util.tallyWarnings(warningTally, { "Not refined — " .. reason }, fileName, index)
						end
					end
					LrTasks.pcall(function()
						if exportedPath and LrFileUtils.exists(exportedPath) then
							LrFileUtils.delete(exportedPath)
						end
					end)
					if not ok then
						log:error("AI edit generation threw for " .. fileName .. ": " .. tostring(apiOk))
						table.insert(errorMessages, fileName .. ": exception thrown: " .. tostring(apiOk))
						errorCount = errorCount + 1
						continueProcessing = false
					else
						response = apiResponse
						local responseWarnings = Util.responseWarnings(response)
						if #responseWarnings > 0 then
							log:warn(
								"AI edit backend warnings for "
									.. fileName
									.. ": "
									.. table.concat(responseWarnings, " | ")
							)
							Util.tallyWarnings(warningTally, responseWarnings, fileName, index)
						end
					end
					if
						continueProcessing
						and (not apiOk or not response or type(response) ~= "table" or response.status ~= "success")
					then
						local errMsg = "Unknown error"
						if not apiOk then
							errMsg = Util.userFacingError(tostring(apiResponse))
						elseif type(response) == "string" then
							errMsg = response
						elseif response and response.error then
							errMsg = response.error
						end
						log:error(
							"AI edit generation failed for "
								.. fileName
								.. ": apiOk="
								.. tostring(apiOk)
								.. " responseType="
								.. tostring(type(response))
								.. " response="
								.. tostring(response)
						)
						table.insert(errorMessages, fileName .. ": " .. errMsg)
						errorCount = errorCount + 1
						continueProcessing = false
					else
						log:trace(
							"AI edit generation succeeded for "
								.. fileName
								.. " responseStatus="
								.. tostring(response and response.status)
						)
					end
				end
			end

			if continueProcessing and response then
				local applyOptions = {
					applyGlobal = true,
					applyMasks = true,
				}

				if options.reviewBeforeApply then
					log:trace("Showing review dialog for " .. fileName)
					local result, validated = DevelopEditManager.showValidationDialog(ctx, photo, response, options)
					log:trace("Review dialog result for " .. fileName .. ": " .. tostring(result))
					if result == "cancel" then
						skippedCount = skippedCount + 1
						continueProcessing = false
					elseif validated then
						applyOptions = validated
					end
				end

				if continueProcessing and not applyOptions.applyGlobal and not applyOptions.applyMasks then
					skippedCount = skippedCount + 1
					continueProcessing = false
				end

				-- Only now, once the edit is actually going to be applied: a
				-- photo the user skipped in the review dialog must not leave a
				-- virtual copy behind.
				local target = photo
				if continueProcessing and options.createVirtualCopy then
					local copy, copyErr, strayCount = createVirtualCopyFor(photo)
					if copy then
						target = copy
						log:trace("Created virtual copy for " .. fileName)
					else
						-- Deliberately not falling back to the original: the
						-- user asked for it to stay untouched.
						log:error("Failed to create virtual copy for " .. fileName .. ": " .. tostring(copyErr))
						table.insert(
							errorMessages,
							fileName .. ": could not create virtual copy: " .. tostring(copyErr)
						)
						errorCount = errorCount + 1
						continueProcessing = false
						strayCopyCount = strayCopyCount + strayCount
					end
				end

				if continueProcessing then
					log:trace("Persisting generated recipe for " .. fileName)
					local okPersist, persistErr = LrTasks.pcall(function()
						DevelopEditManager.persistEditRecipe(target, response, nil, "generated")
					end)
					if not okPersist then
						log:error("Persist generated recipe threw for " .. fileName .. ": " .. tostring(persistErr))
						table.insert(errorMessages, fileName .. ": could not persist recipe: " .. tostring(persistErr))
						errorCount = errorCount + 1
						continueProcessing = false
						if target ~= photo then
							strayCopyCount = strayCopyCount + 1
						end
					end
				end

				if continueProcessing then
					log:trace(
						"Applying recipe for "
							.. fileName
							.. " applyGlobal="
							.. tostring(applyOptions.applyGlobal)
							.. " applyMasks="
							.. tostring(applyOptions.applyMasks)
					)
					local applied, warnings, refined
					local refineWarnings = nil
					if baselineScore and applyOptions.applyGlobal then
						applied, warnings, refined = AestheticRefine.run({
							photo = target,
							response = response,
							baselineScore = baselineScore,
							applyOptions = applyOptions,
							isCanceled = function()
								return progressScope:isCanceled()
							end,
							onTrial = function(strength)
								local percent = math.floor(strength * 100 + 0.5)
								progressScope:setCaption("Refining " .. fileName .. " (trying " .. percent .. "%)")
							end,
						})
						if applied == nil then
							-- Nothing was touched; the plain recipe still goes on.
							refineWarnings = warnings
						elseif refined then
							refinedCount = refinedCount + 1
							if refined.strength < 1 then
								reducedCount = reducedCount + 1
							end
						end
					end
					if applied == nil then
						applied, warnings = DevelopEditManager.applyRecipe(target, response, applyOptions)
						for _, warning in ipairs(refineWarnings or {}) do
							table.insert(warnings, warning)
						end
					end
					log:trace(
						"Apply result for "
							.. fileName
							.. ": applied="
							.. tostring(applied)
							.. " warningsCount="
							.. tostring(type(warnings) == "table" and #warnings or 0)
					)
					if applied then
						successCount = successCount + 1
					else
						errorCount = errorCount + 1
						table.insert(errorMessages, fileName .. ": failed to apply recipe")
						if target ~= photo then
							strayCopyCount = strayCopyCount + 1
						end
					end
					-- For a failed apply too: its warnings carry the cause
					-- ("Failed to apply global develop settings: ..."), and a
					-- log line is not a report.
					Util.tallyWarnings(warningTally, warnings, fileName, index)
					if warnings and #warnings > 0 then
						log:warn("AI edit warnings for " .. fileName .. ": " .. table.concat(warnings, " | "))
					end
				end
			end
		end

		progressScope:done()

		local warningCount = Util.warningTallySize(warningTally)
		if errorCount > 0 or warningCount > 0 then
			local uniqueErrors = {}
			local errorList = {}
			for _, msg in ipairs(errorMessages) do
				if not uniqueErrors[msg] then
					uniqueErrors[msg] = true
					table.insert(errorList, "- " .. msg)
					if #errorList >= 5 then
						break
					end
				end
			end

			local combinedReport =
				LOC("$$$/LrGeniusAI/TaskAiEditPhotos/Summary=Applied edits to ^1 photo(s).", tostring(successCount))
			if refinedCount > 0 then
				combinedReport = combinedReport .. "\n" .. refinementSummary(refinedCount, reducedCount)
			end
			if skippedCount > 0 then
				combinedReport = combinedReport
					.. "\n"
					.. LOC("$$$/LrGeniusAI/common/Skipped=Skipped: ^1", tostring(skippedCount))
			end
			if errorCount > 0 then
				combinedReport = combinedReport
					.. "\n"
					.. LOC("$$$/LrGeniusAI/common/Errors=Errors: ^1", tostring(errorCount))
			end

			if #errorList > 0 then
				combinedReport = combinedReport
					.. "\n\n"
					.. LOC("$$$/LrGeniusAI/common/ErrorDetails=Error details:")
					.. "\n"
					.. table.concat(errorList, "\n")
				if #errorMessages > 5 then
					combinedReport = combinedReport
						.. "\n"
						.. LOC("$$$/LrGeniusAI/common/MoreErrors=... and ^1 more errors", tostring(#errorMessages - 5))
				end
			end

			if strayCopyCount > 0 then
				-- Said once for the run, not in every error line: the user acts
				-- on it once, after the run.
				combinedReport = combinedReport
					.. "\n\n"
					.. string.format(
						'Failed attempts may have left %d virtual %s named "AI Edit" without the edit,\n'
							.. "or with only part of it. Lightroom does not let plugins delete photos: select\n"
							.. "them in the Library grid and remove them with Photo > Remove Photo.",
						strayCopyCount,
						strayCopyCount == 1 and "copy" or "copies"
					)
			end

			if warningCount > 0 then
				combinedReport = combinedReport
					.. "\n\n"
					.. "Warnings:"
					.. "\n"
					.. table.concat(Util.formatWarningTally(warningTally, 5), "\n")
			end

			if errorCount > 0 and successCount == 0 then
				ErrorHandler.handleError(
					LOC("$$$/LrGeniusAI/TaskAiEditPhotos/CompletionTitle=AI Edit Completed"),
					combinedReport
				)
			else
				LrDialogs.message(
					LOC("$$$/LrGeniusAI/TaskAiEditPhotos/CompletionTitle=AI Edit Completed"),
					combinedReport,
					"warning"
				)
			end
		else
			local summary = LOC(
				"$$$/LrGeniusAI/TaskAiEditPhotos/SuccessSummary=Applied edits to ^1 photo(s).\nSkipped: ^2",
				tostring(successCount),
				tostring(skippedCount)
			)
			if refinedCount > 0 then
				summary = summary .. "\n" .. refinementSummary(refinedCount, reducedCount)
			end
			if canceled then
				summary = summary .. "\n\n" .. "Canceled before all photos were processed."
			end
			LrDialogs.message(LOC("$$$/LrGeniusAI/TaskAiEditPhotos/SuccessTitle=AI Lightroom Edit"), summary, "info")
		end
		log:info(
			"AI Edit task completed. success="
				.. tostring(successCount)
				.. " skipped="
				.. tostring(skippedCount)
				.. " errors="
				.. tostring(errorCount)
		)
	end)
end)
