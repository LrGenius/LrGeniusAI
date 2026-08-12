--- Exports a hand-culled selection as an evaluation fixture.
--
-- Culling quality has no ground truth in this repository. Every signal shipped
-- so far was argued from first principles, and twice that reasoning turned out
-- to be wrong in ways only real photographs revealed. The fixtures under
-- `server-rs/testdata/cull_eval/` are synthetic and were written by the same
-- person who wrote the code they score, which makes them regression tests for
-- known bugs and nothing more.
--
-- This task closes that loop. The photographer culls a series by hand in
-- Lightroom as they normally would; this reads those decisions back out, pairs
-- them with the metrics the backend already stored, and writes a fixture the
-- scoring harness can run against.
--
-- **Labels come from flags and ratings**, because those are what a real cull
-- produces:
--   * rejected flag  -> `reject: true`
--   * flagged pick   -> the keeper, rank 1
--   * star rating    -> orders everything else, highest first
--
-- **No photographs leave the machine.** A fixture holds metric values, capture
-- times, perceptual hashes and opaque photo ids. It can be shared, committed and
-- diffed without shipping any imagery.

local function exportOptionsDialog(ctx)
	local f = LrView.osFactory()
	local bind = LrView.bind
	local share = LrView.share

	local props = LrBinding.makePropertyTable(ctx)
	props.scope = prefs.cullFixtureScope or "selected"
	props.cullingPreset = prefs.cullPreset or "default"
	props.fixtureName = prefs.cullFixtureName or ""
	props.notes = prefs.cullFixtureNotes or ""
	props.groupsAreAuthoritative = prefs.cullFixtureAuthoritative == true

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:static_text({
			title = LOC(
				"$$$/LrGeniusAI/CullFixture/Intro=Turns photos you have already culled by hand into a scoring fixture.\nUse the reject flag for frames you would delete, the pick flag for each keeper,\nand star ratings to order the rest. No images are exported — only measurements."
			),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/CullFixture/SourceGroup=Source"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/common/ScopeLabel=Apply to:"),
					width = share("fixtureLabelWidth"),
				}),
				f:popup_menu({
					value = bind("scope"),
					items = {
						{
							title = LOC("$$$/LrGeniusAI/common/ScopeSelected=Selected photos only"),
							value = "selected",
						},
						{ title = LOC("$$$/LrGeniusAI/common/ScopeView=Current view"), value = "view" },
					},
					width = 260,
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/CullFixture/PresetLabel=Culled with preset:"),
					width = share("fixtureLabelWidth"),
				}),
				f:popup_menu({
					value = bind("cullingPreset"),
					items = {
						{ title = "default", value = "default" },
						{ title = "portrait", value = "portrait" },
						{ title = "street", value = "street" },
						{ title = "event", value = "event" },
						{ title = "sports", value = "sports" },
					},
					width = 260,
				}),
			}),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/CullFixture/DescribeGroup=Describe this fixture"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/CullFixture/NameLabel=Name:"),
					width = share("fixtureLabelWidth"),
				}),
				f:edit_field({ value = bind("fixtureName"), width = 260, immediate = true }),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/CullFixture/NotesLabel=Notes:"),
					width = share("fixtureLabelWidth"),
				}),
				f:edit_field({ value = bind("notes"), width = 260, height_in_lines = 3, immediate = true }),
			}),
			f:checkbox({
				value = bind("groupsAreAuthoritative"),
				title = LOC(
					"$$$/LrGeniusAI/CullFixture/Authoritative=I stacked these photos myself — score the grouping too"
				),
			}),
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/CullFixture/AuthoritativeHint=Leave unticked unless you used Lightroom stacks to mark which frames\nbelong together. Without that, groups are taken from the backend's own\noutput and scoring them would just compare it against itself."
				),
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/CullFixture/WindowTitle=Export Culling Fixture"),
		contents = contents,
		actionVerb = LOC("$$$/LrGeniusAI/CullFixture/Run=Export"),
		cancelVerb = LOC("$$$/LrGeniusAI/common/Cancel=Cancel"),
	})
	if result ~= "ok" then
		return nil
	end

	prefs.cullFixtureScope = props.scope
	prefs.cullFixtureName = props.fixtureName
	prefs.cullFixtureNotes = props.notes
	prefs.cullFixtureAuthoritative = props.groupsAreAuthoritative

	return {
		scope = props.scope,
		cullingPreset = props.cullingPreset,
		fixtureName = props.fixtureName,
		notes = props.notes,
		groupsAreAuthoritative = props.groupsAreAuthoritative,
	}
end

---
-- Reads one photo's hand-cull decision.
--
-- `pickStatus` is 1 (flagged), 0 (unflagged) or -1 (rejected). A rejected frame
-- is a reject whatever its rating says; a flagged frame is the keeper. Rating
-- carries everything in between, and is the only ordering signal most people
-- actually produce.
-- @return number|nil pickStatus, number rating
local function readLabel(photo)
	local pickStatus = photo:getRawMetadata("pickStatus")
	local rating = photo:getRawMetadata("rating")
	return (type(pickStatus) == "number") and pickStatus or 0, (type(rating) == "number") and rating or 0
end

---
-- Turns the photos of one backend group into fixture entries carrying the
-- photographer's ranking.
--
-- Ordering is by (flagged first, then rating descending). Ties keep the
-- backend's own order, which is arbitrary but stable — and a tie in the labels
-- genuinely means the photographer expressed no preference, so any order is as
-- correct as another.
local function labelledPhotos(groupPhotos, photoById, metricsById)
	local entries = {}
	for _, photoResult in ipairs(groupPhotos) do
		local photoId = photoResult["photo_id"]
		local photo = photoId and photoById[photoId]
		if photo then
			local pickStatus, rating = readLabel(photo)
			table.insert(entries, {
				photoId = photoId,
				photo = photo,
				pickStatus = pickStatus,
				rating = rating,
				metrics = metricsById[photoId] or {},
			})
		end
	end

	table.sort(entries, function(a, b)
		if (a.pickStatus == 1) ~= (b.pickStatus == 1) then
			return a.pickStatus == 1
		end
		if a.rating ~= b.rating then
			return a.rating > b.rating
		end
		return false
	end)

	-- A group where nobody expressed a preference carries no ranking at all,
	-- rather than a fabricated one: the harness skips unranked groups, which is
	-- honest, where inventing rank 1 for whichever frame sorted first would
	-- quietly poison every accuracy number computed from this fixture.
	local anyLabel = false
	for _, e in ipairs(entries) do
		if e.pickStatus ~= 0 or e.rating > 0 then
			anyLabel = true
			break
		end
	end

	local out = {}
	for index, e in ipairs(entries) do
		local entry = {
			photo_id = e.photoId,
			filename = e.photo:getFormattedMetadata("fileName") or "",
			metadata = e.metrics,
		}
		local captureTime = e.photo:getRawMetadata("dateTime")
		if type(captureTime) == "number" then
			entry.capture_time = LrDate.timeToPosixDate(captureTime)
		end
		if anyLabel then
			entry.rank = index
			if e.pickStatus == -1 then
				entry.reject = true
			end
		end
		table.insert(out, entry)
	end
	return out, anyLabel
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskExportCullFixture", function(context)
		if not Util.waitForServerDialog() then
			return
		end

		local options = exportOptionsDialog(context)
		if not options then
			return
		end

		local photosToProcess, status = PhotoSelector.getPhotosInScope(options.scope)
		if not photosToProcess or #photosToProcess == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/common/NoPhotosTitle=No Photos Found"),
				status == "Invalid view"
						and LOC(
							"$$$/LrGeniusAI/common/InvalidViewMessage=The 'Current view' scope only works when a folder or collection is selected."
						)
					or LOC("$$$/LrGeniusAI/common/NoPhotosMessage=No photos found in the selected scope.")
			)
			return
		end

		local photoIds = {}
		local photoById = {}
		for _, photo in ipairs(photosToProcess) do
			local photoId = SearchIndexAPI.getPhotoIdForPhoto(photo)
			if photoId then
				table.insert(photoIds, photoId)
				photoById[photoId] = photo
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

		-- The grouping and the stored metrics both come from a real /cull call,
		-- so the fixture describes exactly what the backend saw. The labels
		-- layered on top are the only human input.
		local progressScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/CullFixture/ProgressTitle=Reading culling data..."),
			functionContext = context,
		})
		-- `include_stored_metadata` is what makes the fixture reproducible. The
		-- ranked `metrics` block is what ranking *concluded* — short names,
		-- derived values, preset weights already folded in — and replaying a run
		-- needs what it *read*, under the store's own keys.
		local cullResult, err = SearchIndexAPI.cullPhotos(photoIds, {
			culling_preset = options.cullingPreset,
			include_stored_metadata = true,
		})
		progressScope:done()

		local groups = cullResult and cullResult.groups
		if err or type(groups) ~= "table" or #groups == 0 then
			ErrorHandler.handleError(
				LOC("$$$/LrGeniusAI/CullFixture/ErrorTitle=Could not build fixture"),
				err
					or LOC(
						"$$$/LrGeniusAI/CullFixture/NoDataMessage=The backend returned no culling data for these photos. Run 'Cull Similar Photos' on them first so their metrics are computed."
					)
			)
			return
		end

		local metricsById = {}
		local missingStored = 0
		for _, group in ipairs(groups) do
			for _, photoResult in ipairs(group["photos"] or {}) do
				local photoId = photoResult["photo_id"]
				if photoId then
					local stored = photoResult["stored_metadata"]
					if type(stored) == "table" then
						metricsById[photoId] = stored
					else
						-- An older backend that does not know the flag. Fall
						-- back rather than fail, but count it: the fixture will
						-- be missing the peak-sharpness and set-detection inputs
						-- and cannot fully reproduce a run.
						missingStored = missingStored + 1
						metricsById[photoId] = {}
					end
				end
			end
		end
		if missingStored > 0 then
			log:warn(
				"Cull fixture: backend returned no stored_metadata for "
					.. tostring(missingStored)
					.. " photo(s); it is probably older than this plugin"
			)
		end

		local fixtureGroups = {}
		local labelledGroupCount = 0
		for _, group in ipairs(groups) do
			local photos, anyLabel = labelledPhotos(group["photos"] or {}, photoById, metricsById)
			if #photos > 0 then
				local entry = {
					group_id = tostring(group["group_id"] or ""),
					photos = photos,
				}
				-- Carry the backend's own set classification through. A bracket
				-- is a bracket whoever noticed it, and the harness needs to know
				-- not to expect reject labels inside one.
				if group["keep_all"] == true and group["intentional_set"] then
					entry.intentional_set = tostring(group["intentional_set"])
				end
				table.insert(fixtureGroups, entry)
				if anyLabel then
					labelledGroupCount = labelledGroupCount + 1
				end
			end
		end

		if labelledGroupCount == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/CullFixture/NoLabelsTitle=Nothing is labelled"),
				LOC(
					"$$$/LrGeniusAI/CullFixture/NoLabelsMessage=None of these photos carry a pick flag, a reject flag or a star rating, so there is nothing for the fixture to compare against. Cull them by hand first, then export."
				),
				"warning"
			)
			return
		end

		local outputPath = LrDialogs.runSavePanel({
			title = LOC("$$$/LrGeniusAI/CullFixture/SaveTitle=Save culling fixture"),
			prompt = LOC("$$$/LrGeniusAI/CullFixture/SavePrompt=Save Fixture"),
			canCreateDirectories = true,
			requiredFileType = "json",
		})
		if not outputPath or outputPath == "" then
			return
		end

		local fixtureName = options.fixtureName
		if not fixtureName or fixtureName == "" then
			fixtureName = LrPathUtils.removeExtension(LrPathUtils.leafName(outputPath))
		end

		local fixture = {
			name = fixtureName,
			notes = options.notes or "",
			culling_preset = options.cullingPreset or "default",
			groups_are_authoritative = options.groupsAreAuthoritative and true or false,
			groups = fixtureGroups,
		}

		local ok, encoded = LrTasks.pcall(function()
			return JSON:encode_pretty(fixture)
		end)
		if not ok or not encoded then
			ErrorHandler.handleError(
				LOC("$$$/LrGeniusAI/CullFixture/ErrorTitle=Could not build fixture"),
				tostring(encoded)
			)
			return
		end

		local written, writeErr = LrTasks.pcall(function()
			return LrFileUtils.writeFile(outputPath, encoded)
		end)
		if not written then
			ErrorHandler.handleError(
				LOC("$$$/LrGeniusAI/CullFixture/ErrorTitle=Could not build fixture"),
				tostring(writeErr)
			)
			return
		end

		log:info("Culling fixture written to " .. tostring(outputPath))
		LrDialogs.message(
			LOC("$$$/LrGeniusAI/CullFixture/DoneTitle=Fixture exported"),
			LOC(
				'$$$/LrGeniusAI/CullFixture/DoneMessage=Wrote ^1 group(s), ^2 of them labelled, covering ^3 photo(s).\n\nScore it with:\ncargo run --release -p lrg-analysis --example cull_eval -- "^4"',
				tostring(#fixtureGroups),
				tostring(labelledGroupCount),
				tostring(#photoIds),
				tostring(outputPath)
			)
		)
	end)
end)
