--[[
    Find Similar Images: Select one photo, find others similar by perceptual hash (and CLIP).
    1. User selects a single photo
    2. Optional dialog: search scope (current view / all indexed), max results, similarity strictness
    3. Call server find_similar API
    4. Create collection and show in Library
]]

local function showFindSimilarDialog(ctx)
    local f = LrView.osFactory()
    local bind = LrView.bind

    local props = LrBinding.makePropertyTable(ctx)
    props.searchScope = prefs.findSimilarScope or "all"
    props.maxResults = prefs.findSimilarMaxResults or 100
    props.phashStrictness = prefs.findSimilarPhashStrictness or "normal"
    props.similarityMode = prefs.findSimilarMode or "clip"

    local contents = f:column {
        bind_to_object = props,
        spacing = f:control_spacing(),
        f:static_text {
            title = LOC "$$$/LrGeniusAI/FindSimilarImages/SelectOptions=Search options:",
            font = "<system/bold>",
        },
        f:row {
            f:static_text { title = LOC "$$$/LrGeniusAI/FindSimilarImages/FindBy=Find by:", width = 120 },
            f:popup_menu {
                value = bind "similarityMode",
                items = {
                    { title = LOC "$$$/LrGeniusAI/FindSimilarImages/ModePhash=Near duplicates (phash)", value = "phash" },
                    { title = LOC "$$$/LrGeniusAI/FindSimilarImages/ModeClip=Similar content (CLIP)", value = "clip" },
                },
                width = 260,
            },
        },
        f:row {
            f:static_text { title = LOC "$$$/LrGeniusAI/FindSimilarImages/SearchIn=Search in:", width = 120 },
            f:popup_menu {
                value = bind "searchScope",
                items = {
                    { title = LOC "$$$/LrGeniusAI/FindSimilarImages/ScopeAll=All indexed photos", value = "all" },
                    { title = LOC "$$$/LrGeniusAI/FindSimilarImages/ScopeView=Current view", value = "view" },
                },
                width = 260,
            },
        },
        f:row {
            f:static_text { title = LOC "$$$/LrGeniusAI/FindSimilarImages/MaxResults=Max results:", width = 120 },
            f:popup_menu {
                value = bind "maxResults",
                items = {
                    { title = "50", value = 50 },
                    { title = "100", value = 100 },
                    { title = "200", value = 200 },
                    { title = "500", value = 500 },
                },
                width = 120,
            },
        },
        f:row {
            f:static_text { title = LOC "$$$/LrGeniusAI/FindSimilarImages/Similarity=Similarity:", width = 120 },
            f:popup_menu {
                value = bind "phashStrictness",
                items = {
                    { title = LOC "$$$/LrGeniusAI/FindSimilarImages/Strict=Strict (near duplicates)", value = "strict" },
                    { title = LOC "$$$/LrGeniusAI/FindSimilarImages/Normal=Normal", value = "normal" },
                    { title = LOC "$$$/LrGeniusAI/FindSimilarImages/Loose=Loose (more variety)", value = "loose" },
                },
                width = 260,
            },
        },
    }

    local result = LrDialogs.presentModalDialog {
        title = LOC "$$$/LrGeniusAI/FindSimilarImages/WindowTitle=Find Similar Images",
        contents = contents,
        actionVerb = LOC "$$$/LrGeniusAI/FindSimilarImages/Search=Find Similar",
        cancelVerb = LOC "$$$/LrGeniusAI/common/Cancel=Cancel",
    }
    if result ~= "ok" then
        return nil
    end

    prefs.findSimilarScope = props.searchScope
    prefs.findSimilarMaxResults = props.maxResults
    prefs.findSimilarPhashStrictness = props.phashStrictness
    prefs.findSimilarMode = props.similarityMode

    return {
        searchScope = props.searchScope,
        maxResults = props.maxResults,
        phashStrictness = props.phashStrictness,
        similarityMode = props.similarityMode,
    }
end


local function phashMaxHammingFromStrictness(strictness)
    if strictness == "strict" then return 5 end
    if strictness == "loose" then return 18 end
    return 10
end


local function createCollectionFromPhotoIds(photoIds, collectionName)
    local catalog = LrApplication.activeCatalog()
    local photos = SearchIndexAPI.findPhotosByPhotoIds(photoIds)
    if #photos == 0 then
        LrDialogs.message(
            LOC "$$$/LrGeniusAI/FindSimilarImages/NoPhotosInCatalog=Not in catalog",
            LOC "$$$/LrGeniusAI/FindSimilarImages/NoPhotosInCatalogMessage=Similar photos were found in the index but are not in the current catalog."
        )
        return
    end

    local collectionSet, collection
    catalog:withWriteAccessDo("Create Collection Set", function()
        collectionSet = catalog:createCollectionSet(LOC "$$$/LrGeniusAI/FindSimilarImages/CollectionSetName=Similar Images", nil, true)
    end, Defaults.catalogWriteAccessOptions)
    if not collectionSet then
        ErrorHandler.handleError(
            LOC "$$$/LrGeniusAI/FindSimilarImages/CollectionSetError=Collection set error",
            LOC "$$$/LrGeniusAI/FindSimilarImages/CollectionSetErrorMessage=Could not create collection set for similar images."
        )
        return
    end

    catalog:withWriteAccessDo("Create Collection", function()
        collection = catalog:createCollection(collectionName, collectionSet, false)
    end, Defaults.catalogWriteAccessOptions)
    if not collection then
        ErrorHandler.handleError(
            LOC "$$$/LrGeniusAI/FindSimilarImages/CollectionError=Collection error",
            LOC "$$$/LrGeniusAI/FindSimilarImages/CollectionErrorMessage=Could not create collection."
        )
        return
    end

    catalog:withWriteAccessDo("Add Photos to Collection", function()
        collection:addPhotos(photos)
    end, Defaults.catalogWriteAccessOptions)

    catalog:setActiveSources({ collection })
    LrApplicationView.gridView()
    LrDialogs.message(
        LOC "$$$/LrGeniusAI/FindSimilarImages/Done=Done",
        LOC("$$$/LrGeniusAI/People/CollectionCreated=^1 photo(s) added to collection \"^2\".", tostring(#photos), collectionName)
    )
end


LrTasks.startAsyncTask(function()
    LrFunctionContext.callWithContext("TaskFindSimilarImages", function(context)
        if not Util.waitForServerDialog() then return end

        local catalog = LrApplication.activeCatalog()
        local targetPhotos = catalog:getTargetPhotos()
        if not targetPhotos or #targetPhotos == 0 then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/FindSimilarImages/NoPhotoTitle=No photo selected",
                LOC "$$$/LrGeniusAI/FindSimilarImages/NoPhotoMessage=Please select a single photo in the Library."
            )
            return
        end
        if #targetPhotos > 1 then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/FindSimilarImages/SinglePhotoTitle=Select one photo",
                LOC "$$$/LrGeniusAI/FindSimilarImages/SinglePhotoMessage=Please select exactly one photo to find similar images."
            )
            return
        end

        local options = showFindSimilarDialog(context)
        if not options then return end

        local photo = targetPhotos[1]
        local photoId, photoIdErr = SearchIndexAPI.getPhotoIdForPhoto(photo)
        if not photoId or photoId == "" then
            ErrorHandler.handleError(
                LOC "$$$/LrGeniusAI/FindSimilarImages/PhotoIdError=Could not get photo ID",
                photoIdErr or "Photo has no UUID. Run Analyze & Index first."
            )
            return
        end

        local scopePhotoIds = nil
        if options.searchScope == "view" then
            local scopePhotos, status = PhotoSelector.getPhotosInScope("view")
            if not scopePhotos or #scopePhotos == 0 then
                LrDialogs.message(
                    LOC "$$$/LrGeniusAI/common/InvalidViewTitle=Invalid View",
                    LOC "$$$/LrGeniusAI/common/InvalidViewMessage=The 'Current view' scope only works when a folder or collection is selected."
                )
                return
            end
            scopePhotoIds = {}
            for _, p in ipairs(scopePhotos) do
                local id = SearchIndexAPI.getPhotoIdForPhoto(p)
                if id and id ~= "" and id ~= photoId then
                    scopePhotoIds[#scopePhotoIds + 1] = id
                end
            end
        end

        local progressScope = LrProgressScope({
            title = LOC "$$$/LrGeniusAI/FindSimilarImages/ProgressTitle=Finding similar images...",
            functionContext = context,
        })
        progressScope:setPortionComplete(0, 1)

        local apiOptions = {
            max_results = options.maxResults,
            phash_max_hamming = phashMaxHammingFromStrictness(options.phashStrictness),
            use_clip = true,
            similarity_mode = options.similarityMode or "phash",
        }
        if scopePhotoIds and #scopePhotoIds > 0 then
            apiOptions.scope_photo_ids = scopePhotoIds
        end

        local result, err = SearchIndexAPI.findSimilarImages(photoId, apiOptions)
        progressScope:setPortionComplete(1, 1)
        progressScope:done()

        if err then
            ErrorHandler.handleError(
                LOC "$$$/LrGeniusAI/FindSimilarImages/SearchError=Find similar failed",
                err
            )
            return
        end

        -- If the server returned a warning (e.g. reference photo not indexed), show it and stop.
        if result and result.warning then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/FindSimilarImages/NotIndexedTitle=Photo not indexed",
                result.warning
            )
            return
        end

        local results = (result and result.results) and result.results or {}
        if #results == 0 then
            log:warn("Find similar images: 0 results for photo_id=%s scope=%s phash_max_hamming=%s", photoId, options.searchScope, phashMaxHammingFromStrictness(options.phashStrictness))
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/FindSimilarImages/NoResultsTitle=No similar images",
                LOC "$$$/LrGeniusAI/FindSimilarImages/NoResultsMessage=No similar images found. The photo may not be indexed yet, or no other photos are similar enough. Run 'Analyze & Index' to ensure perceptual hashes are computed."
            )
            return
        end

        local photoIds = {}
        for _, r in ipairs(results) do
            local pid = r.photo_id or r.photo_uuid
            if pid and pid ~= photoId then
                photoIds[#photoIds + 1] = pid
            end
        end

        local collectionName = LOC("$$$/LrGeniusAI/FindSimilarImages/CollectionName=Similar images @ ^1", LrDate.timeToW3CDate(LrDate.currentTime()))
        createCollectionFromPhotoIds(photoIds, collectionName)
    end)
end)
