require("DevelopEditManager")
require("AestheticRefine")
require("EditLoop")

-- AI Edit has two editors.
--
-- The AI retoucher (the default) is a loop with a vision model: the model
-- edits, Lightroom renders the result, the model looks at it next to the
-- original with its measurements and corrects, until it is done. The backend
-- holds the conversation (`/v1/edit/sessions`); EditLoop.lua applies each
-- step, computes its AI masks and renders it. With enough training examples
-- the loop starts from the photographer's own style (the style engine's
-- recipe as round 0) and the model only corrects what does not suit the frame.
--
-- "My style only" is the style engine alone: offline, no model, every recipe
-- built from the user's saved edits (`/v1/edit/style`), with the optional
-- aesthetic-score refinement.

-- Mirrors `MIN_TRAINING_EXAMPLES` in
-- `server-rs/crates/lrg-analysis/src/style_engine.rs`. Below it the style
-- engine declines to answer, so "My style only" stops before it uploads
-- anything, and the retoucher starts from the photo instead of a style.
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

-- The looks the retoucher offers. The ids are the backend's
-- (`lrg_providers::edit_loop::LOOKS`), which says what each means to the model.
local LOOKS = {
	{ title = "Natural — a better version of the photo", value = "natural" },
	{ title = "Bright & airy", value = "bright_airy" },
	{ title = "Moody", value = "moody" },
	{ title = "Cinematic", value = "cinematic" },
	{ title = "Analog film", value = "film" },
	{ title = "Vivid & punchy", value = "punchy" },
	{ title = "Black & white", value = "black_and_white" },
}

local EDIT_ROUNDS = {
	{ title = "1 — edit once, no second look", value = 1 },
	{ title = "2", value = 2 },
	{ title = "3 (recommended)", value = 3 },
	{ title = "4", value = 4 },
	{ title = "5", value = 5 },
}

local function showAiEditDialog(ctx, stats, models)
	log:trace("showAiEditDialog: start")
	local f = LrView.osFactory()
	local bind = LrView.bind
	local share = LrView.share
	local props = LrBinding.makePropertyTable(ctx)

	local readinessText, readinessColor, trainingCount = styleReadinessDisplay(stats)
	local styleAvailable = (trainingCount or 0) >= MIN_TRAINING_EXAMPLES
	local masksSupported = EditLoop.supportsMasks(LrApplication.versionTable())

	props.scope = prefs.aiEditScope or "selected"
	props.engine = prefs.aiEditEngine or "retoucher"
	props.modelKey = AiProviders.initialKey(models.items, models.savedKey)
	props.look = prefs.aiEditLook or "natural"
	props.instructions = prefs.aiEditInstructions or ""
	props.maxEdits = tonumber(prefs.aiEditMaxEdits) or 3
	props.useStyle = styleAvailable and prefs.aiEditUseStyle ~= false
	props.useMasks = masksSupported and prefs.aiEditUseMasks ~= false
	props.reviewBeforeApply = prefs.aiEditReviewBeforeApply ~= false
	props.createVirtualCopy = prefs.aiEditCreateVirtualCopy == true
	props.refineWithAesthetic = prefs.aiEditRefineAesthetic == true

	-- Which controls apply depends on the editor; they stay visible, so the
	-- dialog does not change size, and are switched off when they do not.
	local function updateEnabled()
		local retoucher = props.engine == "retoucher"
		props.isRetoucher = retoucher
		props.styleEnabled = retoucher and styleAvailable
		props.masksEnabled = retoucher and masksSupported
		props.isStyleOnly = not retoucher
	end
	updateEnabled()
	props:addObserver("engine", updateEnabled)

	local labelWidth = share("labelWidth")
	local grey = LrColor(0.5, 0.5, 0.5)

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/common/Scope=Scope"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/common/ApplyTo=Apply to:"),
					width = labelWidth,
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
			title = "Editor",
			fill_horizontal = 1,
			f:row({
				f:static_text({ title = "Edit with:", width = labelWidth }),
				f:popup_menu({
					value = bind("engine"),
					width = 300,
					items = {
						{ title = "AI retoucher (recommended)", value = "retoucher" },
						{ title = "My style only (offline, no AI model)", value = "style" },
					},
				}),
			}),
			f:row({
				f:static_text({
					title = "The AI retoucher edits each photo, looks at what Lightroom renders and corrects\n"
						.. "it, like a person at the Develop panel. My style only blends your saved edits.",
					text_color = grey,
				}),
			}),
		}),
		f:group_box({
			title = "AI retoucher",
			fill_horizontal = 1,
			f:row({
				f:static_text({ title = "AI model:", width = labelWidth, enabled = bind("isRetoucher") }),
				f:column({
					f:popup_menu({
						value = bind("modelKey"),
						items = models.items,
						width = 300,
						enabled = bind("isRetoucher"),
					}),
					models.warnings and f:static_text({
						title = models.warnings,
						text_color = LrColor(0.8, 0, 0),
						size = "small",
						wrap = true,
						width = 300,
					}) or nil,
				}),
			}),
			f:row({
				f:static_text({ title = "", width = labelWidth }),
				f:static_text({
					title = "It has to see images: a vision model. Larger models edit better.",
					text_color = grey,
				}),
			}),
			f:row({
				f:static_text({ title = "Look:", width = labelWidth, enabled = bind("isRetoucher") }),
				f:popup_menu({
					value = bind("look"),
					items = LOOKS,
					width = 300,
					enabled = bind("isRetoucher"),
				}),
			}),
			f:row({
				f:static_text({ title = "Your instructions:", width = labelWidth, enabled = bind("isRetoucher") }),
				f:edit_field({
					value = bind("instructions"),
					width_in_chars = 40,
					height_in_lines = 2,
					enabled = bind("isRetoucher"),
					placeholder_string = "optional, e.g. keep skin warm, no vignette",
				}),
			}),
			f:row({
				f:static_text({ title = "Edits per photo:", width = labelWidth, enabled = bind("isRetoucher") }),
				f:popup_menu({
					value = bind("maxEdits"),
					items = EDIT_ROUNDS,
					width = 300,
					enabled = bind("isRetoucher"),
				}),
			}),
			f:row({
				f:checkbox({ value = bind("useStyle"), enabled = bind("styleEnabled") }),
				f:static_text({
					title = styleAvailable and "Start from my own style (the training examples below)"
						or ("Start from my own style (needs " .. MIN_TRAINING_EXAMPLES .. " training examples)"),
					enabled = bind("styleEnabled"),
				}),
			}),
			f:row({
				f:checkbox({ value = bind("useMasks"), enabled = bind("masksEnabled") }),
				f:static_text({
					title = masksSupported and "Use AI masks (sky, subject, background, skin, eyes, …)"
						or "Use AI masks (needs Lightroom Classic 15.3 or later)",
					enabled = bind("masksEnabled"),
				}),
			}),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/StyleProfile=Style profile"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/TrainingExamples=Training examples:"),
					width = labelWidth,
				}),
				f:static_text({
					title = trainingCount and tostring(trainingCount) or "—",
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/StyleStatus=Status:"),
					width = labelWidth,
				}),
				f:static_text({
					title = readinessText,
					text_color = readinessColor,
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
					title = "Review each edit (the retoucher shows the finished edit to keep or discard)",
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
					enabled = bind("isStyleOnly"),
				}),
				f:static_text({
					title = "Refine my style with the aesthetic score (slower: tries gentler strengths)",
					enabled = bind("isStyleOnly"),
				}),
			}),
			f:row({
				f:static_text({
					title = "Every try and every round is a step in the Edit History; a virtual copy keeps the original's clean.",
					text_color = grey,
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
	prefs.aiEditEngine = props.engine
	prefs.aiEditModelKey = props.modelKey
	prefs.aiEditLook = props.look
	prefs.aiEditInstructions = props.instructions
	prefs.aiEditMaxEdits = props.maxEdits
	prefs.aiEditUseStyle = props.useStyle
	prefs.aiEditUseMasks = props.useMasks
	prefs.aiEditReviewBeforeApply = props.reviewBeforeApply
	prefs.aiEditCreateVirtualCopy = props.createVirtualCopy
	prefs.aiEditRefineAesthetic = props.refineWithAesthetic

	return {
		scope = props.scope,
		engine = props.engine,
		modelKey = props.modelKey,
		look = props.look,
		instructions = props.instructions,
		maxEdits = props.maxEdits,
		useStyle = props.useStyle and styleAvailable,
		useMasks = props.useMasks and masksSupported,
		reviewBeforeApply = props.reviewBeforeApply,
		createVirtualCopy = props.createVirtualCopy,
		refineWithAesthetic = props.engine == "style" and props.refineWithAesthetic,
		trainingCount = trainingCount,
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

---
-- "My style only": the style engine's recipe for each photo, applied as it
-- came or refined with the aesthetic score.
---
local function runStyleEdits(ctx, photos, options, progressScope, run)
	for index, photo in ipairs(photos) do
		if progressScope:isCanceled() then
			run.canceled = true
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
			table.insert(run.errorMessages, fileName .. ": " .. tostring(photoIdErr))
			run.errorCount = run.errorCount + 1
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
				table.insert(run.errorMessages, fileName .. ": export failed")
				run.errorCount = run.errorCount + 1
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
						Util.tallyWarnings(run.warningTally, { "Not refined — " .. reason }, fileName, index)
					end
				end
				LrTasks.pcall(function()
					if exportedPath and LrFileUtils.exists(exportedPath) then
						LrFileUtils.delete(exportedPath)
					end
				end)
				if not ok then
					log:error("AI edit generation threw for " .. fileName .. ": " .. tostring(apiOk))
					table.insert(run.errorMessages, fileName .. ": exception thrown: " .. tostring(apiOk))
					run.errorCount = run.errorCount + 1
					continueProcessing = false
				else
					response = apiResponse
					local responseWarnings = Util.responseWarnings(response)
					if #responseWarnings > 0 then
						log:warn(
							"AI edit backend warnings for " .. fileName .. ": " .. table.concat(responseWarnings, " | ")
						)
						Util.tallyWarnings(run.warningTally, responseWarnings, fileName, index)
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
					table.insert(run.errorMessages, fileName .. ": " .. errMsg)
					run.errorCount = run.errorCount + 1
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
					run.skippedCount = run.skippedCount + 1
					continueProcessing = false
				elseif validated then
					applyOptions = validated
				end
			end

			if continueProcessing and not applyOptions.applyGlobal and not applyOptions.applyMasks then
				run.skippedCount = run.skippedCount + 1
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
						run.errorMessages,
						fileName .. ": could not create virtual copy: " .. tostring(copyErr)
					)
					run.errorCount = run.errorCount + 1
					continueProcessing = false
					run.strayCopyCount = run.strayCopyCount + strayCount
				end
			end

			if continueProcessing then
				log:trace("Persisting generated recipe for " .. fileName)
				local okPersist, recorded, recordErr = LrTasks.pcall(function()
					return DevelopEditManager.persistEditRecipe(target, response, nil, "generated")
				end)
				local persistErr = recorded
				if okPersist and not recorded then
					-- Written to the catalog or not, the edit goes on: the
					-- record is AI Edit's note, not the edit.
					Util.tallyWarnings(
						run.warningTally,
						{ DevelopEditManager.recordFailureWarning(recordErr) },
						fileName,
						index
					)
				end
				if not okPersist then
					log:error("Persist generated recipe threw for " .. fileName .. ": " .. tostring(persistErr))
					table.insert(run.errorMessages, fileName .. ": could not persist recipe: " .. tostring(persistErr))
					run.errorCount = run.errorCount + 1
					continueProcessing = false
					if target ~= photo then
						run.strayCopyCount = run.strayCopyCount + 1
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
						run.refinedCount = run.refinedCount + 1
						if refined.strength < 1 then
							run.reducedCount = run.reducedCount + 1
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
					run.successCount = run.successCount + 1
				else
					run.errorCount = run.errorCount + 1
					table.insert(run.errorMessages, fileName .. ": failed to apply recipe")
					if target ~= photo then
						run.strayCopyCount = run.strayCopyCount + 1
					end
				end
				-- For a failed apply too: its warnings carry the cause
				-- ("Failed to apply global develop settings: ..."), and a
				-- log line is not a report.
				Util.tallyWarnings(run.warningTally, warnings, fileName, index)
				if warnings and #warnings > 0 then
					log:warn("AI edit warnings for " .. fileName .. ": " .. table.concat(warnings, " | "))
				end
			end
		end
	end
end

-- The run's completion dialog, for either editor.
local function showCompletion(run)
	local warningCount = Util.warningTallySize(run.warningTally)
	if run.errorCount > 0 or warningCount > 0 then
		local uniqueErrors = {}
		local errorList = {}
		for _, msg in ipairs(run.errorMessages) do
			if not uniqueErrors[msg] then
				uniqueErrors[msg] = true
				table.insert(errorList, "- " .. msg)
				if #errorList >= 5 then
					break
				end
			end
		end

		local combinedReport =
			LOC("$$$/LrGeniusAI/TaskAiEditPhotos/Summary=Applied edits to ^1 photo(s).", tostring(run.successCount))
		if run.refinedCount > 0 then
			combinedReport = combinedReport .. "\n" .. refinementSummary(run.refinedCount, run.reducedCount)
		end
		for _, line in ipairs(run.summaryLines or {}) do
			combinedReport = combinedReport .. "\n" .. line
		end
		if run.canceled then
			combinedReport = combinedReport .. "\n" .. "Canceled before all photos were processed."
		end
		if run.skippedCount > 0 then
			combinedReport = combinedReport
				.. "\n"
				.. LOC("$$$/LrGeniusAI/common/Skipped=Skipped: ^1", tostring(run.skippedCount))
		end
		if run.errorCount > 0 then
			combinedReport = combinedReport
				.. "\n"
				.. LOC("$$$/LrGeniusAI/common/Errors=Errors: ^1", tostring(run.errorCount))
		end

		if #errorList > 0 then
			combinedReport = combinedReport
				.. "\n\n"
				.. LOC("$$$/LrGeniusAI/common/ErrorDetails=Error details:")
				.. "\n"
				.. table.concat(errorList, "\n")
			if #run.errorMessages > 5 then
				combinedReport = combinedReport
					.. "\n"
					.. LOC("$$$/LrGeniusAI/common/MoreErrors=... and ^1 more errors", tostring(#run.errorMessages - 5))
			end
		end

		if run.strayCopyCount > 0 then
			-- Said once for the run, not in every error line: the user acts
			-- on it once, after the run.
			combinedReport = combinedReport
				.. "\n\n"
				.. string.format(
					'Failed attempts may have left %d virtual %s named "AI Edit" without the edit,\n'
						.. "or with only part of it. Lightroom does not let plugins delete photos: select\n"
						.. "them in the Library grid and remove them with Photo > Remove Photo.",
					run.strayCopyCount,
					run.strayCopyCount == 1 and "copy" or "copies"
				)
		end

		if warningCount > 0 then
			combinedReport = combinedReport
				.. "\n\n"
				.. "Warnings:"
				.. "\n"
				.. table.concat(Util.formatWarningTally(run.warningTally, 5), "\n")
		end

		if run.errorCount > 0 and run.successCount == 0 then
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
			tostring(run.successCount),
			tostring(run.skippedCount)
		)
		if run.refinedCount > 0 then
			summary = summary .. "\n" .. refinementSummary(run.refinedCount, run.reducedCount)
		end
		for _, line in ipairs(run.summaryLines or {}) do
			summary = summary .. "\n" .. line
		end
		if run.canceled then
			summary = summary .. "\n\n" .. "Canceled before all photos were processed."
		end
		LrDialogs.message(LOC("$$$/LrGeniusAI/TaskAiEditPhotos/SuccessTitle=AI Lightroom Edit"), summary, "info")
	end
end

local function removeFile(path)
	LrTasks.pcall(function()
		if path and LrFileUtils.exists(path) then
			LrFileUtils.delete(path)
		end
	end)
end

-- The request fields every session of a run shares: the model and its
-- connection, the look, the user's words, and the local engine's settings.
-- @return table|nil fields, string|nil why the model cannot be used.
local function retoucherFields(options)
	local provider, model = AiProviders.splitModelKey(options.modelKey)
	if Util.nilOrEmpty(provider) or Util.nilOrEmpty(model) then
		return nil, "Choose an AI model for the retoucher in the AI Edit dialog."
	end
	local connection, problem = AiProviders.connectionOptions(provider, Secrets.connectionFields(prefs))
	if not connection then
		return nil, problem
	end
	return {
		provider = provider,
		model = model,
		api_key = connection.api_key,
		server_url = connection.server_url,
		look = options.look,
		instructions = options.instructions,
		language = prefs.generateLanguage or "English",
		max_edits = tostring(options.maxEdits or 3),
		masks = tostring(options.useMasks == true),
		llm_n_ctx = prefs.llmContextSize and tostring(prefs.llmContextSize) or nil,
		llm_n_parallel = prefs.llmParallel and tostring(prefs.llmParallel) or nil,
		llm_gpu_layers = prefs.llmGpuLayers and tostring(prefs.llmGpuLayers) or nil,
	},
		nil
end

-- What the retoucher did with one photo, in words, for the review dialog.
local function retouchReport(result)
	local lines = {}
	local modelRounds = 0
	for _, round in ipairs(result.rounds or {}) do
		if round.source ~= "style" then
			modelRounds = modelRounds + 1
		end
	end
	if result.keptRound == 0 then
		table.insert(lines, "Kept your own style, unchanged by the AI model.")
	elseif result.keptRound ~= nil then
		table.insert(lines, string.format("Kept round %d of %d.", result.keptRound, modelRounds))
	end
	if not Util.nilOrEmpty(result.verdict) then
		table.insert(lines, "The AI model: " .. tostring(result.verdict))
	end
	table.insert(lines, "")
	for _, round in ipairs(result.rounds or {}) do
		if round.source == "style" then
			table.insert(lines, "Round 0: your style from your saved edits.")
		elseif not Util.nilOrEmpty(round.assessment) then
			table.insert(lines, "Round " .. tostring(round.round) .. ": " .. tostring(round.assessment))
		end
	end
	if #(result.guardrailExplanations or {}) > 0 then
		table.insert(lines, "")
		table.insert(lines, "What the frame allowed:")
		local seen = {}
		for _, text in ipairs(result.guardrailExplanations) do
			if not seen[text] then
				seen[text] = true
				table.insert(lines, "- " .. text)
			end
		end
	end
	if #(result.warnings or {}) > 0 then
		table.insert(lines, "")
		table.insert(lines, "Notes:")
		local seen = {}
		for _, text in ipairs(result.warnings) do
			if not seen[text] then
				seen[text] = true
				table.insert(lines, "- " .. text)
			end
		end
	end
	return table.concat(lines, "\n")
end

-- Shows a finished edit, already on the photo, to keep or discard.
-- @return string "ok" keep, "cancel" discard, "other" keep this one and the
--   rest without asking.
local function reviewRetouch(photo, fileName, result)
	local f = LrView.osFactory()
	local report = retouchReport(result)
	-- A static_text clips to its height instead of growing (see
	-- Defaults.copyrightLineCount), so count the lines it will wrap into.
	local lineCount = 0
	for line in (report .. "\n"):gmatch("([^\n]*)\n") do
		lineCount = lineCount + math.max(1, math.ceil(#line / 60))
	end
	local contents = f:row({
		spacing = f:control_spacing(),
		f:catalog_photo({
			photo = photo,
			width = 380,
			height = 380,
		}),
		f:scrolled_view({
			width = 420,
			height = 380,
			horizontal_scroller = false,
			f:static_text({
				title = report,
				width = 390,
				height_in_lines = lineCount,
				wrap = true,
			}),
		}),
	})
	return LrDialogs.presentModalDialog({
		title = "AI Edit: " .. fileName,
		contents = contents,
		actionVerb = "Keep",
		cancelVerb = "Discard",
		otherVerb = "Keep all",
	})
end

---
-- The AI retoucher: one session per photo, see EditLoop.lua.
---
local function runRetoucherEdits(photos, options, progressScope, run)
	local fields, problem = retoucherFields(options)
	if not fields then
		run.errorCount = run.errorCount + 1
		table.insert(run.errorMessages, problem)
		return
	end
	local reviewing = options.reviewBeforeApply
	local modelRounds, inputTokens, outputTokens = 0, 0, 0

	local function editPhoto(index, photo)
		local fileName = photo:getFormattedMetadata("fileName") or "Photo"
		local function caption(text)
			progressScope:setCaption(
				fileName .. " (" .. tostring(index) .. " of " .. tostring(#photos) .. "): " .. tostring(text)
			)
		end
		caption("starting")
		progressScope:setPortionComplete(index - 1, #photos)

		local photoId, photoIdErr = SearchIndexAPI.getPhotoIdForPhoto(photo)
		if not photoId then
			run.errorCount = run.errorCount + 1
			table.insert(run.errorMessages, fileName .. ": " .. tostring(photoIdErr))
			return
		end
		local photoOptions = enrichPhotoOptions(photo, {})

		local styleRecipe = nil
		if options.useStyle then
			caption("matching your style")
			local path = SearchIndexAPI.exportPhotoForIndexing(photo)
			local why
			if path then
				local ok, apiOk, response = LrTasks.pcall(function()
					return SearchIndexAPI.styleEdit(photoId, path, photoOptions)
				end)
				removeFile(path)
				if ok and apiOk and type(response) == "table" and type(response.edit) == "table" then
					styleRecipe = response.edit
					Util.tallyWarnings(run.warningTally, Util.responseWarnings(response), fileName, index)
				elseif ok and apiOk then
					why = "the style engine returned no recipe"
				elseif ok then
					why = Util.userFacingError(tostring(response))
				else
					why = tostring(apiOk)
				end
			else
				why = "the photo could not be rendered"
			end
			if why then
				Util.tallyWarnings(
					run.warningTally,
					{ "Your style could not be matched, so the AI model edited without it: " .. why },
					fileName,
					index
				)
			end
		end

		-- The loop edits for real, so the copy comes first.
		local target = photo
		if options.createVirtualCopy then
			local copy, copyErr, strayCount = createVirtualCopyFor(photo)
			if not copy then
				run.errorCount = run.errorCount + 1
				table.insert(run.errorMessages, fileName .. ": could not create virtual copy: " .. tostring(copyErr))
				run.strayCopyCount = run.strayCopyCount + strayCount
				return
			end
			target = copy
		end

		local sessionFields = copyOptions(fields)
		if photoOptions.is_raw ~= nil then
			sessionFields.is_raw = tostring(photoOptions.is_raw)
		end
		local result = EditLoop.run({
			photo = target,
			photoId = photoId,
			fields = sessionFields,
			styleRecipe = styleRecipe,
			isCanceled = function()
				return progressScope:isCanceled()
			end,
			onProgress = caption,
		})
		inputTokens = inputTokens + (tonumber(result.inputTokens) or 0)
		outputTokens = outputTokens + (tonumber(result.outputTokens) or 0)
		for _, round in ipairs(result.rounds) do
			if round.source ~= "style" then
				modelRounds = modelRounds + 1
			end
		end

		Util.tallyWarnings(run.warningTally, result.warnings, fileName, index)
		if result.canceled then
			-- A photo left halfway is worse than one left alone.
			local reverted, revertErr = EditLoop.revert(target, result)
			if not reverted then
				table.insert(
					run.errorMessages,
					fileName .. ": canceled halfway, and the edit could not be taken back: " .. tostring(revertErr)
				)
				run.errorCount = run.errorCount + 1
			end
			if target ~= photo then
				run.strayCopyCount = run.strayCopyCount + 1
			end
			run.canceled = true
			return
		end
		if not result.applied then
			log:error("AI Edit retoucher failed for " .. fileName .. ": " .. tostring(result.error))
			run.errorCount = run.errorCount + 1
			table.insert(
				run.errorMessages,
				fileName .. ": " .. tostring(result.error or "the edit could not be applied")
			)
			if target ~= photo then
				run.strayCopyCount = run.strayCopyCount + 1
			end
			return
		end

		if reviewing then
			local answer = reviewRetouch(target, fileName, result)
			if answer == "cancel" then
				local reverted, revertErr = EditLoop.revert(target, result)
				if reverted then
					run.skippedCount = run.skippedCount + 1
				else
					run.errorCount = run.errorCount + 1
					table.insert(
						run.errorMessages,
						fileName .. ": the edit could not be discarded: " .. tostring(revertErr)
					)
				end
				if target ~= photo then
					run.strayCopyCount = run.strayCopyCount + 1
				end
				return
			elseif answer == "other" then
				reviewing = false
			end
		end

		local okPersist, recorded, recordErr = LrTasks.pcall(function()
			return DevelopEditManager.persistEditRecipe(
				target,
				{ edit = result.recipe or {}, edit_model = fields.model },
				result.warnings,
				"generated"
			)
		end)
		if not okPersist or not recorded then
			local why = okPersist and recordErr or recorded
			log:error("Recording the retoucher's edit failed for " .. fileName .. ": " .. tostring(why))
			Util.tallyWarnings(run.warningTally, { DevelopEditManager.recordFailureWarning(why) }, fileName, index)
		end
		run.successCount = run.successCount + 1
	end

	for index, photo in ipairs(photos) do
		if progressScope:isCanceled() then
			run.canceled = true
			break
		end
		editPhoto(index, photo)
		if run.canceled then
			break
		end
	end

	run.summaryLines = {
		string.format(
			"AI retoucher: %s, %d edit round(s) in total; %d input and %d output tokens.",
			tostring(fields.model),
			modelRounds,
			inputTokens,
			outputTokens
		),
	}
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("AiEditPhotosTask", function(ctx)
		LrDialogs.attachErrorDialogToFunctionContext(ctx)
		log:info("AI Edit task started")

		if not Util.waitForServerDialog() then
			log:warn("AI Edit task aborted: backend server unavailable")
			return
		end

		-- A backend that could not answer is not a backend reporting zero: the
		-- dialog says "unavailable", and a style-only run falls through to let
		-- the per-photo errors say what actually went wrong.
		local stats, statsErr = SearchIndexAPI.getTrainingStats()
		if not stats then
			log:warn("AI Edit could not read the style profile stats: " .. tostring(statsErr))
		end

		local modelsResp, modelsErr = SearchIndexAPI.getModels({ includeCloud = true })
		local savedKey = AiProviders.resolveSavedKey(modelsResp, prefs.aiEditModelKey or prefs.modelKey)
		local models = {
			items = AiProviders.modelItems(modelsResp, savedKey),
			savedKey = savedKey,
			warnings = SearchIndexAPI.condenseMessages(modelsResp and modelsResp.warnings),
		}
		if not modelsResp then
			models.warnings = "The list of AI models could not be loaded: " .. tostring(modelsErr or "no answer")
		end

		local options = showAiEditDialog(ctx, stats, models)
		if not options then
			log:info("AI Edit task canceled by user in options dialog")
			return
		end
		log:trace(
			"AI Edit options selected: engine="
				.. tostring(options.engine)
				.. " scope="
				.. tostring(options.scope)
				.. " model="
				.. tostring(options.modelKey)
				.. " look="
				.. tostring(options.look)
				.. " edits="
				.. tostring(options.maxEdits)
				.. " style="
				.. tostring(options.useStyle)
				.. " masks="
				.. tostring(options.useMasks)
		)

		if options.engine == "style" then
			local trainingCount = (stats and tonumber(stats.count)) or 0
			if stats and trainingCount < MIN_TRAINING_EXAMPLES then
				log:warn("AI Edit task aborted: only " .. tostring(trainingCount) .. " training example(s) available")
				LrDialogs.message(
					LOC("$$$/LrGeniusAI/TaskAiEditPhotos/NoStyleProfileTitle=Style profile not ready"),
					"My style only builds every recipe from your own saved edits, and it needs at least "
						.. tostring(MIN_TRAINING_EXAMPLES)
						.. " training examples to do that. You currently have "
						.. tostring(trainingCount)
						.. ".\n\nUse the AI retoucher instead, or edit a handful of photos the way you like them, "
						.. "run Help → Save Edits as AI Training Examples and start AI Edit again.",
					"info"
				)
				return
			end
			if options.refineWithAesthetic then
				local answer = aestheticModelPreflight()
				if answer == "cancel" then
					log:info("AI Edit task canceled at the aesthetic model preflight")
					return
				elseif answer == "other" then
					options.refineWithAesthetic = false
				end
			end
		else
			if Util.nilOrEmpty(options.modelKey) then
				LrDialogs.showError("Choose an AI model for the retoucher in the AI Edit dialog.")
				return
			end
			local unavailable = AiProviders.unavailableReason(modelsResp, options.modelKey)
			if unavailable then
				LrDialogs.showError(unavailable)
				return
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

		local run = {
			successCount = 0,
			skippedCount = 0,
			errorCount = 0,
			errorMessages = {},
			-- One entry per distinct text with a photo count: a cause that hits
			-- every photo in the run (white balance on JPEGs, say) is reported
			-- once, not once per photo.
			warningTally = Util.newWarningTally(),
			refinedCount = 0,
			reducedCount = 0,
			-- "AI Edit" virtual copies left in the catalog by an attempt that
			-- then failed or was discarded: the SDK cannot delete photos.
			strayCopyCount = 0,
			canceled = false,
			summaryLines = {},
		}

		if options.engine == "style" then
			runStyleEdits(ctx, photos, options, progressScope, run)
		else
			runRetoucherEdits(photos, options, progressScope, run)
		end

		progressScope:done()
		showCompletion(run)
		log:info(
			"AI Edit task completed. engine="
				.. tostring(options.engine)
				.. " success="
				.. tostring(run.successCount)
				.. " skipped="
				.. tostring(run.skippedCount)
				.. " errors="
				.. tostring(run.errorCount)
		)
	end)
end)
