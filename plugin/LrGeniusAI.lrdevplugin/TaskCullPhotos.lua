local function showCullDialog(ctx)
    local f = LrView.osFactory()
    local bind = LrView.bind
    local share = LrView.share

    local props = LrBinding.makePropertyTable(ctx)
    props.scope = prefs.cullScope or "selected"
    props.timeDeltaSeconds = prefs.cullTimeDeltaSeconds or 2
    props.cullingPreset = prefs.cullPreset or "default"
    props.createDuplicatesCollection = prefs.cullCreateDuplicatesCollection ~= false

    local contents = f:column {
        bind_to_object = props,
        spacing = f:control_spacing(),
        f:group_box {
            title = LOC "$$$/LrGeniusAI/CullTask/ScopeGroup=Scope",
            fill_horizontal = 1,
            f:row {
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/CullTask/ScopeLabel=Apply to:",
                    width = share 'labelWidth',
                },
                f:popup_menu {
                    value = bind 'scope',
                    items = {
                        { title = LOC "$$$/LrGeniusAI/common/ScopeSelected=Selected photos only", value = 'selected' },
                        { title = LOC "$$$/LrGeniusAI/common/ScopeView=Current view", value = 'view' },
                    },
                    width = 260,
                },
            },
        },
        f:group_box {
            title = LOC "$$$/LrGeniusAI/CullTask/OptionsGroup=Options",
            fill_horizontal = 1,
            f:row {
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/CullTask/TimeDeltaLabel=Burst time window (seconds):",
                    width = share 'labelWidth',
                },
                f:combo_box {
                    value = bind 'timeDeltaSeconds',
                    items = {
                        { title = "1", value = 1 },
                        { title = "2", value = 2 },
                        { title = "3", value = 3 },
                        { title = "5", value = 5 },
                    },
                    width = 120,
                },
            },
            f:row {
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/CullTask/PresetLabel=Culling preset:",
                    width = share 'labelWidth',
                },
                f:popup_menu {
                    value = bind 'cullingPreset',
                    items = {
                        { title = LOC "$$$/LrGeniusAI/CullTask/PresetDefault=Default (balanced)", value = 'default' },
                        { title = LOC "$$$/LrGeniusAI/CullTask/PresetPortrait=Portrait (face-focused)", value = 'portrait' },
                        { title = LOC "$$$/LrGeniusAI/CullTask/PresetStreet=Street (technical-focused)", value = 'street' },
                        { title = LOC "$$$/LrGeniusAI/CullTask/PresetEvent=Event (people + moments)", value = 'event' },
                        { title = LOC "$$$/LrGeniusAI/CullTask/PresetSports=Sports (motion-tolerant)", value = 'sports' },
                    },
                    width = 260,
                },
            },
            f:row {
                f:checkbox {
                    value = bind 'createDuplicatesCollection',
                },
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/CullTask/CreateDuplicates=Create 'Duplicates / Near Duplicates' collection",
                },
            },
        },
    }

    local result = LrDialogs.presentModalDialog {
        title = LOC "$$$/LrGeniusAI/CullTask/WindowTitle=Cull Similar Photos",
        contents = contents,
        actionVerb = LOC "$$$/LrGeniusAI/CullTask/Run=Cull",
        cancelVerb = LOC "$$$/LrGeniusAI/common/Cancel=Cancel",
    }

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
        if not Util.waitForServerDialog() then return end

        local options = showCullDialog(context)
        if not options then return end

        local photosToProcess, status = PhotoSelector.getPhotosInScope(options.scope)
        if not photosToProcess or #photosToProcess == 0 then
            if status == "Invalid view" then
                LrDialogs.message(
                    LOC "$$$/LrGeniusAI/common/InvalidViewTitle=Invalid View",
                    LOC "$$$/LrGeniusAI/common/InvalidViewMessage=The 'Current view' scope only works when a folder or collection is selected."
                )
            else
                LrDialogs.message(
                    LOC "$$$/LrGeniusAI/common/NoPhotosTitle=No Photos Found",
                    LOC "$$$/LrGeniusAI/common/NoPhotosMessage=No photos found in the selected scope."
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
                LOC "$$$/LrGeniusAI/CullTask/NoPhotoIdsTitle=No usable photos",
                LOC "$$$/LrGeniusAI/CullTask/NoPhotoIdsMessage=No usable photo IDs could be computed for the selected photos."
            )
            return
        end

        local progressScope = LrProgressScope({
            title = LOC "$$$/LrGeniusAI/CullTask/ProgressTitle=Culling similar photos...",
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
                LOC "$$$/LrGeniusAI/CullTask/ErrorTitle=Culling failed",
                err or LOC "$$$/LrGeniusAI/CullTask/ErrorMessage=Could not create culling groups."
            )
            return
        end

        if #groups == 0 then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/CullTask/NoGroupsTitle=No groups found",
                LOC "$$$/LrGeniusAI/CullTask/NoGroupsMessage=The selected photos could not be grouped for culling."
            )
            return
        end

        local picksIds = {}
        local alternateIds = {}
        local rejectIds = {}
        local duplicateIds = {}
        local nearDuplicateGroupCount = 0

        for _, group in ipairs(groups) do
            local winnerPhotoId = group["winner_photo_id"]
            local alternatePhotoIds = group["alternate_photo_ids"] or {}
            local rejectCandidatePhotoIds = group["reject_candidate_photo_ids"] or {}
            local groupType = group["group_type"]
            local groupPhotoIds = group["photo_ids"] or {}

            if winnerPhotoId then
                table.insert(picksIds, winnerPhotoId)
            end
            for _, photoId in ipairs(alternatePhotoIds) do
                table.insert(alternateIds, photoId)
            end
            for _, photoId in ipairs(rejectCandidatePhotoIds) do
                table.insert(rejectIds, photoId)
            end
            if options.createDuplicatesCollection and groupType == "near_duplicate" then
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

            picksCollection = createResultCollection(
                LOC "$$$/LrGeniusAI/CullTask/Picks=Picks",
                picksPhotos
            )
            createResultCollection(
                LOC "$$$/LrGeniusAI/CullTask/Alternates=Alternates",
                alternatePhotos
            )
            createResultCollection(
                LOC "$$$/LrGeniusAI/CullTask/Rejects=Reject Candidates",
                rejectPhotos
            )
            if options.createDuplicatesCollection then
                createResultCollection(
                    LOC "$$$/LrGeniusAI/CullTask/Duplicates=Duplicates / Near Duplicates",
                    duplicatePhotos
                )
            end
        end, Defaults.catalogWriteAccessOptions)

        if picksCollection then
            catalog:setActiveSources({ picksCollection })
            LrApplicationView.gridView()
        end

        LrDialogs.message(
            LOC "$$$/LrGeniusAI/CullTask/CompletionTitle=Culling Complete",
            LOC(
                "$$$/LrGeniusAI/CullTask/CompletionMessage=Created culling collections for ^1 groups. Picks: ^2, Alternates: ^3, Reject candidates: ^4. Near-duplicate groups: ^5. Preset: ^6.",
                tostring(summary.group_count or #groups),
                tostring(summary.pick_count or #picksPhotos),
                tostring(summary.alternate_count or #alternatePhotos),
                tostring(summary.reject_candidate_count or #rejectPhotos),
                tostring(summary.near_duplicate_group_count or nearDuplicateGroupCount),
                tostring(summary.culling_preset or options.cullingPreset or "default")
            )
        )
    end)
end)
