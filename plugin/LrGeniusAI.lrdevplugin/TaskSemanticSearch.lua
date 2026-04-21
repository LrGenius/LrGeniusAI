--[[
    Provides an advanced search dialog that combines semantic search with optional quality filtering.
    If the search term is empty, it performs a quality-only search.
]]

local function showAdvancedSearchDialog(ctx)
    local props = LrBinding.makePropertyTable(ctx)
    props.searchTerm = ""
    props.useQualityFilter = false
    props.qualitySort = 'prettiest'
    -- Scope and search-in options from prefs (persisted)
    props.searchScope = prefs.searchScope or 'all'
    props.searchInSemanticSiglip = prefs.searchInSemanticSiglip ~= false
    props.searchInSemanticVertex = prefs.searchInSemanticVertex ~= false
    props.searchInMetadata = prefs.searchInMetadata ~= false
    props.searchInMetadataKeywords = prefs.searchInMetadataKeywords ~= false
    props.searchInMetadataCaption = prefs.searchInMetadataCaption ~= false
    props.searchInMetadataTitle = prefs.searchInMetadataTitle ~= false
    props.searchInMetadataAltText = prefs.searchInMetadataAltText ~= false

    local f = LrView.osFactory()
    local bind = LrView.bind
    local share = LrView.share

    local contents = f:view {
        bind_to_object = props,
        spacing = f:control_spacing(),
        f:column {
            spacing = f:control_spacing(),
            f:row {
                f:static_text { title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchTerm=Search Term", width = share 'labelWidth', alignment = 'right' },
                f:edit_field { value = bind('searchTerm'), width_in_chars = 40 },
            },
            f:row {
                f:static_text { title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchScope=Search Scope:", width = share 'labelWidth', alignment = 'right' },
                f:popup_menu {
                    value = bind('searchScope'),
                    items = {
                        { title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/ScopeAllPhotos=All photos", value = 'all' },
                        { title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/ScopeCurrentView=Current view", value = 'view' },
                        { title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/ScopeSelectedPhotos=Selected photos", value = 'selected' },
                    },
                },
            },
            f:group_box {
                title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchIn=Search in",
                f:column {
                    spacing = f:control_spacing(),
                    f:checkbox {
                        value = bind('searchInSemanticSiglip'),
                        title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchInSemanticSiglip=Semantic (SigLIP / local AI)",
                    },
                    f:checkbox {
                        value = bind('searchInSemanticVertex'),
                        title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchInSemanticVertex=Semantic (Vertex AI)",
                    },
                    f:checkbox {
                        value = bind('searchInMetadata'),
                        title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchInMetadata=Metadata",
                    },
                    f:column {
                        spacing = 2,
                        fill_horizontal = 1,
                        f:row {
                            fill_horizontal = 1,
                            f:static_text { width = 20 },
                            f:checkbox {
                                value = bind('searchInMetadataKeywords'),
                                enabled = bind('searchInMetadata'),
                                title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchInMetadataKeywords=Keywords",
                            },
                        },
                        f:row {
                            f:static_text { width = 20 },
                            f:checkbox {
                                value = bind('searchInMetadataCaption'),
                                enabled = bind('searchInMetadata'),
                                title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchInMetadataCaption=Caption",
                            },
                        },
                        f:row {
                            f:static_text { width = 20 },
                            f:checkbox {
                                value = bind('searchInMetadataTitle'),
                                enabled = bind('searchInMetadata'),
                                title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchInMetadataTitle=Title",
                            },
                        },
                        f:row {
                            f:static_text { width = 20 },
                            f:checkbox {
                                value = bind('searchInMetadataAltText'),
                                enabled = bind('searchInMetadata'),
                                title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchInMetadataAltText=Alt text",
                            },
                        },
                    },
                },
            },
        }
    }

    local result = LrDialogs.presentModalDialog {
        title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/WindowTitle=Advanced Search",
        contents = contents,
        actionVerb = LOC "$$$/LrGeniusAI/common/Search=Search",
        cancelVerb = LOC "$$$/LrGeniusAI/common/Cancel=Cancel",
        resizable = false,
    }

    if result == 'ok' then
        -- Persist dialog options to prefs for next time
        prefs.searchScope = props.searchScope
        prefs.searchInSemanticSiglip = props.searchInSemanticSiglip
        prefs.searchInSemanticVertex = props.searchInSemanticVertex
        prefs.searchInMetadata = props.searchInMetadata
        prefs.searchInMetadataKeywords = props.searchInMetadataKeywords
        prefs.searchInMetadataCaption = props.searchInMetadataCaption
        prefs.searchInMetadataTitle = props.searchInMetadataTitle
        prefs.searchInMetadataAltText = props.searchInMetadataAltText
        return props
    else
        return nil
    end
end

LrTasks.startAsyncTask(function()
    LrFunctionContext.callWithContext("showAdvancedSearchDialog", function(context)
        -- Check server connection and health (ensure CLIP is ready for semantic search)
        if not Util.waitForServerDialog({ requireClip = true }) then return end

        local props = showAdvancedSearchDialog(context)
        if props == nil then return end

        local results, err
        local collectionName
        local catalog = LrApplication.activeCatalog()

        -- Determine photos to search based on scope
        local photosToSearch
        -- 'selected' matches the popup_menu value; keep 'view' as-is
        if props.searchScope == 'selected' or props.searchScope == 'view' then
            local status
            photosToSearch, status = PhotoSelector.getPhotosInScope(props.searchScope)
            if not photosToSearch or #photosToSearch == 0 then
                if status == "Invalid view" then
                    LrDialogs.message(LOC "$$$/LrGeniusAI/common/InvalidViewTitle=Invalid View", LOC "$$$/LrGeniusAI/common/InvalidViewMessage=The 'Current view' scope only works when a folder or collection or collection set is selected.")
                else
                    LrDialogs.message(LOC "$$$/LrGeniusAI/common/NoPhotosTitle=No Photos Found", LOC "$$$/LrGeniusAI/common/NoPhotosMessage=No photos were found in the selected scope.")
                end
                return
            end
        end -- 'all' means photosToSearch is nil, so we search everything

        local qualitySort = props.useQualityFilter and props.qualitySort or nil

        -- Semantic search (with optional quality filter)
        local searchStartedAt = LrDate.currentTime()
        if props.searchTerm ~= "" then
            log:trace("Performing semantic search for: " .. props.searchTerm)
            local searchOptions = {
                semanticSiglip = props.searchInSemanticSiglip,
                semanticVertex = props.searchInSemanticVertex,
                metadata = props.searchInMetadata,
                metadataFields = {},
            }
            if props.searchInMetadata then
                if props.searchInMetadataKeywords then table.insert(searchOptions.metadataFields, "flattened_keywords") end
                if props.searchInMetadataCaption then table.insert(searchOptions.metadataFields, "caption") end
                if props.searchInMetadataTitle then table.insert(searchOptions.metadataFields, "title") end
                if props.searchInMetadataAltText then table.insert(searchOptions.metadataFields, "alt_text") end
            end
            if #searchOptions.metadataFields == 0 and props.searchInMetadata then
                searchOptions.metadataFields = { "flattened_keywords", "alt_text", "caption", "title" }
            end
            results, err = SearchIndexAPI.searchIndex(props.searchTerm, qualitySort, photosToSearch, searchOptions)
            local elapsedMs = math.floor((LrDate.currentTime() - searchStartedAt) * 1000)
            local resCount = 0
            if type(results) == "table" then
                if results.results then 
                    resCount = #results.results
                else
                    resCount = #results
                end
            end
            log:trace("Semantic search completed. term=" .. tostring(props.searchTerm) ..
                " results=" .. tostring(resCount) ..
                " elapsedMs=" .. tostring(elapsedMs))
            collectionName = string.format("'%s' @ %s", props.searchTerm, LrDate.timeToW3CDate(LrDate.currentTime()))

        -- Quality-only search
        elseif props.useQualityFilter then
            local apiCall, apiCallInSelection, collectionNamePrefix
            if props.qualitySort == 'prettiest' then
                apiCall = SearchIndexAPI.getPrettiest
                apiCallInSelection = SearchIndexAPI.getPrettiestInSelection
                collectionNamePrefix = LOC "$$$/LrGeniusAI/AdvancedSearchTask/Prettiest=Prettiest"
            else -- ugliest
                apiCall = SearchIndexAPI.getUgliest
                apiCallInSelection = SearchIndexAPI.getUgliestInSelection
                collectionNamePrefix = LOC "$$$/LrGeniusAI/AdvancedSearchTask/Ugliest=Ugliest"
            end

            if props.searchScope == 'all' then
                results = apiCall()
                collectionNamePrefix = collectionNamePrefix .. LOC "$$$/LrGeniusAI/AdvancedSearchTask/inCatalog= in Catalog"
            else
                if #photosToSearch == 0 then
                    LrDialogs.message(LOC "$$$/LrGeniusAI/common/NoPhotosTitle=No Photos Found", LOC "$$$/LrGeniusAI/common/NoPhotosMessage=No photos were found in the selected scope.")
                    return
                end
                results = apiCallInSelection(photosToSearch)
                collectionNamePrefix = collectionNamePrefix .. (props.searchScope == 'selection' and LOC "$$$/LrGeniusAI/AdvancedSearchTask/inSelection= in Selection" or LOC "$$$/LrGeniusAI/AdvancedSearchTask/inView= in View")
            end
            collectionName = string.format("%s @ %s", collectionNamePrefix, LrDate.timeToW3CDate(LrDate.currentTime()))
        else
            LrDialogs.message(LOC "$$$/LrGeniusAI/AdvancedSearchTask/noSearchCriteria=No Search Criteria", LOC "$$$/LrGeniusAI/AdvancedSearchTask/noSearchCriteriaMessage=Please enter a search term or select a quality filter.")
            return
        end

        if err then
            ErrorHandler.handleError(
                LOC "$$$/LrGeniusAI/AdvancedSearchTask/SearchError=Search failed",
                err
            )
            return
        end

        if results and results.warning then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/common/BackendWarning=Backend Warning",
                results.warning,
                "warning"
            )
        end

        local finalResults = {}
        if type(results) == "table" then
            if results.results and type(results.results) == "table" then
                finalResults = results.results
            else
                finalResults = results
            end
        end

        if #finalResults == 0 then
            LrDialogs.message(LOC "$$$/LrGeniusAI/AdvancedSearchTask/noResults=No Results", LOC "$$$/LrGeniusAI/AdvancedSearchTask/noResultsMessage=No photos found matching the criteria.")
            return
        end

        -- Build a list of photo IDs once and resolve them in batch for better performance.
        local resolveStartedAt = LrDate.currentTime()
        local photoIds = {}
        for _, result in ipairs(finalResults) do
            if type(result) == "table" then
                local resultPhotoId = result.photo_id or result.uuid
                if resultPhotoId then
                    table.insert(photoIds, resultPhotoId)
                else
                    log:warn(LOC("$$$/LrGeniusAI/AdvancedSearchTask/photoNotFound=Photo with ID ^1 not found in catalog.", "nil"))
                end
            end
        end

        if #photoIds == 0 then
            LrDialogs.message(LOC "$$$/LrGeniusAI/AdvancedSearchTask/noResults=No Results", LOC "$$$/LrGeniusAI/AdvancedSearchTask/noResultsMessage=No photos found matching the criteria.")
            return
        end

        local photos = SearchIndexAPI.findPhotosByPhotoIds(photoIds)
        local resolveElapsedMs = math.floor((LrDate.currentTime() - resolveStartedAt) * 1000)
        log:trace("Semantic search: resolved photos from IDs. ids=" .. tostring(#photoIds) ..
            " resolved=" .. tostring(photos and #photos or 0) ..
            " elapsedMs=" .. tostring(resolveElapsedMs))

        if photos and #photos > 0 then
            local collectionSet = nil
            local collection = nil
            local collectionStartedAt = LrDate.currentTime()

            catalog:withWriteAccessDo("Create Collection Set", function()
                collectionSet = catalog:createCollectionSet(LOC "$$$/LrGeniusAI/AdvancedSearchTask/collectionSetName=Search Results", nil, true)
            end, Defaults.catalogWriteAccessOptions)

            if collectionSet == nil then
                ErrorHandler.handleError(LOC "$$$/LrGeniusAI/AdvancedSearchTask/collectionSetErrorTitle=Collection Set Error", LOC "$$$/LrGeniusAI/AdvancedSearchTask/collectionSetErrorMessage=Failed to create or find collection set for search results.")
                return
            end

            catalog:withWriteAccessDo("Create Collection", function()
                collection = catalog:createCollection(collectionName, collectionSet, false)
            end, Defaults.catalogWriteAccessOptions)

            if collection == nil then
                ErrorHandler.handleError(LOC "$$$/LrGeniusAI/AdvancedSearchTask/collectionErrorTitle=Collection Error", LOC "$$$/LrGeniusAI/AdvancedSearchTask/collectionErrorMessage=Failed to create collection for search results.")
                return
            end

            catalog:withWriteAccessDo("Add Photos to Collection", function()
                collection:addPhotos(photos)
                catalog:setActiveSources({collection})
                LrApplicationView.gridView()
            end, Defaults.catalogWriteAccessOptions)

            local collectionElapsedMs = math.floor((LrDate.currentTime() - collectionStartedAt) * 1000)
            log:trace("Semantic search: collection created and photos added. count=" .. tostring(#photos) ..
                " elapsedMs=" .. tostring(collectionElapsedMs))

            if collection == nil then
                ErrorHandler.handleError(LOC "$$$/LrGeniusAI/AdvancedSearchTask/collectionErrorTitle=Collection Error", LOC "$$$/LrGeniusAI/AdvancedSearchTask/collectionErrorMessage=Failed to create collection for search results.")
            elseif #collection:getPhotos() > 0 then
                LrDialogs.messageWithDoNotShow(
                {
                    message = LOC "$$$/LrGeniusAI/AdvancedSearchTask/successTitle=Search Completed", 
                    info = LOC "$$$/LrGeniusAI/AdvancedSearchTask/sortOrder=Please set the sort order to 'Custom Order' to see the results in the correct order.",
                    actionPrefKey = "LrGeniusAI_AdvancedSearch_SortOrder" 
                })
            end
        else
            LrDialogs.message(LOC "$$$/LrGeniusAI/AdvancedSearchTask/noResults=No Results", LOC "$$$/LrGeniusAI/AdvancedSearchTask/noResultsMessage=No photos found matching the criteria.")
        end
    end)
end)
