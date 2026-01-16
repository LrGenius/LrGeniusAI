--[[
    Provides an advanced search dialog that combines semantic search with optional quality filtering.
    If the search term is empty, it performs a quality-only search.
]]

local function showAdvancedSearchDialog(ctx)
    local props = LrBinding.makePropertyTable(ctx)
    props.searchTerm = ""
    props.useQualityFilter = false
    props.qualitySort = 'prettiest'
    props.searchScope = 'all' -- 'view', 'selection', 'all'

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
            -- f:group_box {
            --     title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/QualityFilter=Quality Filter",
            --     f:column {
            --         spacing = f:control_spacing(),
            --         f:row {
            --             f:checkbox {
            --                 value = bind('useQualityFilter'),
            --             },
            --             f:static_text { title = "Enable Quality Sorting" },
            --         },
            --         f:row {
            --             f:static_text { title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/SortBy=Sort by:", width = share 'labelWidth', enabled = bind('useQualityFilter'), alignment = 'right' },
            --             f:radio_button {
            --                 title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/PrettiestFirst=Prettiest first",
            --                 value = bind "qualitySort",
            --                 bind_to_property = "qualitySort",
            --                 enabled = bind('useQualityFilter'),
            --                 checked_value = 'prettiest',
            --             },
            --             f:radio_button {
            --                 title = LOC "$$$/LrGeniusAI/AdvancedSearchTask/UgliestFirst=Ugliest first",
            --                 value = bind "qualitySort",
            --                 bind_to_property = "qualitySort",
            --                 enabled = bind('useQualityFilter'),
            --                 checked_value = 'ugliest',
            --             },
            --         },
            --     }
            -- },
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
        return props
    else
        return nil
    end
end

LrTasks.startAsyncTask(function()
    LrFunctionContext.callWithContext("showAdvancedSearchDialog", function(context)
        -- Check server connection
        if not Util.waitForServerDialog() then return end

        local props = showAdvancedSearchDialog(context)
        if props == nil then return end

        local results
        local collectionName
        local catalog = LrApplication.activeCatalog()

        -- Determine photos to search based on scope
        local photosToSearch
        if props.searchScope == 'selection' or props.searchScope == 'view' then
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
        if props.searchTerm ~= "" then
            log:trace("Performing semantic search for: " .. props.searchTerm)
            results = SearchIndexAPI.searchIndex(props.searchTerm, qualitySort, photosToSearch)
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

        if results == nil or #results == 0 then
            LrDialogs.message(LOC "$$$/LrGeniusAI/AdvancedSearchTask/noResults=No Results", LOC "$$$/LrGeniusAI/AdvancedSearchTask/noResultsMessage=No photos found matching the criteria.")
            return
        end

        local photos = {}
        for _, result in ipairs(results) do
            local photo = catalog:findPhotoByUuid(result.uuid)
            if photo then
                table.insert(photos, photo)
            else
                log:warn(LOC("$$$/LrGeniusAI/AdvancedSearchTask/photoNotFound=Photo with UUID ^1 not found in catalog.", result.uuid))
            end
        end

        if #photos > 0 then
            local collectionSet = nil
            local collection = nil
            
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
