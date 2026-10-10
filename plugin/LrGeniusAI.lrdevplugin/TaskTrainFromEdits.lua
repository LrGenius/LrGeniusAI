-- TaskTrainFromEdits.lua
-- Allows the user to save their current Lightroom develop settings for selected
-- photos as AI style training examples.  These are stored on the backend and
-- injected as few-shot context the next time AI Edit Photos runs.

require("DevelopEditManager")

local function showTrainDialog(ctx)
	local f = LrView.osFactory()
	local bind = LrView.bind
	local props = LrBinding.makePropertyTable(ctx)

	props.label = prefs.trainingLabel or ""
	props.summary = prefs.trainingSummary or ""
	props.scope = prefs.trainingScope or "selected"

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/AnalyzeAndIndex/Scope=Scope"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/AnalyzeAndIndex/Scope=Scope"),
					width = 150,
				}),
				f:popup_menu({
					value = bind("scope"),
					width = 300,
					items = {
						{ title = LOC("$$$/LrGeniusAI/common/ScopeSelected=Selected photos only"), value = "selected" },
						{ title = LOC("$$$/LrGeniusAI/common/ScopeView=Current view"), value = "view" },
						{ title = LOC("$$$/LrGeniusAI/common/ScopeAll=Entire Catalog"), value = "all" },
					},
				}),
			}),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/Training/StyleGroup=Edit Style"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/Training/LabelLabel=Style label (optional):"),
					width = 180,
				}),
				f:edit_field({
					value = bind("label"),
					width_in_chars = 30,
					placeholder_string = LOC("$$$/LrGeniusAI/Training/LabelPlaceholder=e.g. Wedding, Portrait, Street"),
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/Training/SummaryLabel=Description (optional):"),
					width = 180,
				}),
				f:edit_field({
					value = bind("summary"),
					width_in_chars = 30,
					height_in_lines = 2,
				}),
			}),
		}),
		f:row({
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/Training/DialogHint=Hint: Only select photos that you have manually edited. The AI will learn your style from these examples."
				),
				font = "italic",
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/Training/DialogTitle=Save Edits as AI Training Examples"),
		contents = contents,
		actionVerb = LOC("$$$/LrGeniusAI/Training/SaveButton=Save Examples"),
	})

	if result ~= "ok" then
		return nil
	end

	prefs.trainingLabel = props.label
	prefs.trainingSummary = props.summary
	prefs.trainingScope = props.scope

	return {
		label = props.label,
		summary = props.summary,
		scope = props.scope,
	}
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TrainFromEditsTask", function(ctx)
		LrDialogs.attachErrorDialogToFunctionContext(ctx)
		log:info("Save Training Examples task started")

		if not Util.waitForServerDialog() then
			log:warn("Train task aborted: backend server unavailable")
			return
		end

		local options = showTrainDialog(ctx)
		if not options then
			log:info("Train task cancelled by user")
			return
		end

		local photosToProcess = PhotoSelector.getPhotosInScope(options.scope)
		if not photosToProcess or #photosToProcess == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/Training/NoPhotosTitle=No Photos"),
				LOC("$$$/LrGeniusAI/Training/NoPhotosMsg=No photos found in the selected scope."),
				"info"
			)
			return
		end

		-- Filter photos: only RAW or DNG formats.
		local photos = {}
		for _, photo in ipairs(photosToProcess) do
			local fmt = photo:getRawMetadata("fileFormat")

			-- Only include RAW or DNG.
			if fmt == "RAW" or fmt == "DNG" then
				table.insert(photos, photo)
			end
		end

		if #photos == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/Training/NoValidPhotosTitle=No Valid Training Photos"),
				LOC(
					"$$$/LrGeniusAI/Training/NoValidPhotosMsg=None of the photos in the selected scope match the training criteria (must be RAW or DNG format). JPEGs, TIFFs, and other formats are excluded."
				),
				"info"
			)
			return
		end

		local progressScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/Training/Progress=Saving training examples..."),
			functionContext = ctx,
		})
		progressScope:setPortionComplete(0, #photos)

		local successCount = 0
		local errorCount = 0
		local errorMessages = {}
		-- One line per distinct warning with its photo count, so a cause that
		-- hits every example (an old process version) does not push the
		-- others out of the summary.
		local warningTally = Util.newWarningTally()

		local canceled = false

		for index, photo in ipairs(photos) do
			if progressScope:isCanceled() then
				canceled = true
				break
			end

			local fileName = photo:getFormattedMetadata("fileName") or "Photo"
			progressScope:setCaption(
				string.format(
					LOC("$$$/LrGeniusAI/Training/ProgressCaption=Processing %s (%d of %d)"),
					fileName,
					index,
					#photos
				)
			)
			progressScope:setPortionComplete(index - 1, #photos)

			-- Read current develop settings.
			local developSettings
			local developSettingsFailed = false
			local okGet, devOrErr = LrTasks.pcall(function()
				return photo:getDevelopSettings()
			end)
			if okGet and type(devOrErr) == "table" then
				developSettings = devOrErr
			else
				log:warn("Could not read develop settings for " .. fileName .. ": " .. tostring(devOrErr))
				developSettings = {}
				developSettingsFailed = true
			end

			-- Get a stable photo ID.
			local photoId, photoIdErr = SearchIndexAPI.getPhotoIdForPhoto(photo)
			if not photoId then
				log:error("Failed to resolve photo ID for " .. fileName .. ": " .. tostring(photoIdErr))
				table.insert(errorMessages, fileName .. ": " .. tostring(photoIdErr))
				errorCount = errorCount + 1
			else
				-- Collect EXIF metadata for richer style matching using standardized utility.
				local exifOptions = Util.getPhotoExif(photo)
				exifOptions.label = options.label
				exifOptions.summary = options.summary
				-- Which white-balance family this example's develop settings
				-- use: Kelvin `Temperature` (raw) or an `IncrementalTemperature`
				-- offset (rendered, including a DNG converted from a JPEG).
				-- Taken from the settings themselves, as the backend and
				-- TaskAiEditPhotos do, with the file format as the fallback.
				-- Left absent when neither knows; the backend then reads the
				-- family from the settings' own white-balance keys, and an
				-- example without them does not vote on white balance.
				local wbFamily = DevelopEditManager.whiteBalanceFamily(
					not developSettingsFailed and developSettings or nil,
					Util.isRawPhoto(photo)
				)
				if wbFamily ~= nil then
					exifOptions.is_raw = wbFamily == "raw"
				end

				-- Export a JPEG thumbnail for CLIP embedding + exposure analysis.
				local exportedPath = SearchIndexAPI.exportPhotoForIndexing(photo)

				local ok, resp = SearchIndexAPI.addTrainingExample(
					photoId,
					exportedPath, -- may be nil; server will still store settings
					developSettings,
					exifOptions
				)

				-- Clean up temp file.
				if exportedPath then
					LrTasks.pcall(function()
						if LrFileUtils.exists(exportedPath) then
							LrFileUtils.delete(exportedPath)
						end
					end)
				end

				if ok then
					successCount = successCount + 1
					log:info("Saved training example for " .. fileName)
					local exampleWarnings = {}
					if developSettingsFailed then
						table.insert(
							exampleWarnings,
							"Develop settings could not be read, so this example was saved without them."
						)
					end
					-- `warnings`, or an older backend's joined `warning` string.
					for _, warning in ipairs(Util.responseWarnings(resp)) do
						table.insert(exampleWarnings, warning)
					end
					if #exampleWarnings > 0 then
						log:warn(
							"Training example warnings for " .. fileName .. ": " .. table.concat(exampleWarnings, " | ")
						)
						Util.tallyWarnings(warningTally, exampleWarnings, fileName, index)
					end
				else
					errorCount = errorCount + 1
					table.insert(errorMessages, fileName .. ": " .. tostring(resp))
					log:error("Failed to save training example for " .. fileName .. ": " .. tostring(resp))
				end
			end

			progressScope:setPortionComplete(index, #photos)
		end

		progressScope:done()

		-- Summary dialog.
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
				LOC("$$$/LrGeniusAI/Training/Summary=Saved ^1 training example(s).", tostring(successCount))
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

			if warningCount > 0 then
				combinedReport = combinedReport
					.. "\n\n"
					.. "Warnings:"
					.. "\n"
					.. table.concat(Util.formatWarningTally(warningTally, 5), "\n")
			end

			if canceled then
				combinedReport = combinedReport .. "\n\n" .. "Canceled before all photos were processed."
			end

			local completionTitle = LOC("$$$/LrGeniusAI/Training/CompletionTitle=Training Examples Saved")
			if errorCount > 0 then
				ErrorHandler.handleError(completionTitle, combinedReport)
			else
				-- Warnings only: the examples were saved. The error modal is
				-- titled "Error" and offers "Generate report", which is the wrong
				-- thing to hand someone whose run worked (#375).
				LrDialogs.message(completionTitle, combinedReport, "warning")
			end
		else
			local summary = LOC(
				"$$$/LrGeniusAI/Training/SuccessSummary=Successfully saved ^1 training example(s).\nAI Edit Photos will use your style when editing visually similar photos.",
				tostring(successCount)
			)
			if canceled then
				summary = summary .. "\n\n" .. "Canceled before all photos were processed."
			end
			LrDialogs.message(LOC("$$$/LrGeniusAI/Training/SuccessTitle=Training Examples Saved"), summary, "info")
		end
	end)
end)
