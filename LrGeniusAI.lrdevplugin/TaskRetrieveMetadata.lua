-- TaskRetrieveMetadata.lua
-- Retrieves stored metadata and quality scores from the backend and writes them to the Lightroom catalog.
-- Allows user to select which data fields to transfer and optionally validate before applying.


---
-- Shows the configuration dialog for metadata retrieval.
-- @param ctx The LrFunctionContext for the dialog.
-- @return table with configuration options or nil if canceled.
--
local function showRetrieveMetadataDialog(ctx)
    local f = LrView.osFactory()
    local bind = LrView.bind
    local share = LrView.share

    local props = LrBinding.makePropertyTable(ctx)
    
    -- Scope settings
    props.scope = prefs.retrieveMetadataScope or "selected"
    
    -- Data fields to retrieve and apply
    props.applyKeywords = prefs.applyKeywords ~= false -- default true
    props.applyTitle = prefs.applyTitle ~= false -- default true
    props.applyCaption = prefs.applyCaption ~= false -- default true
    props.applyAltText = prefs.applyAltText ~= false -- default true
    props.applyQuality = prefs.applyQuality ~= false -- default true (includes ratings + critique)
    props.useTopLevelKeyword = prefs.useTopLevelKeyword ~= false -- default true
    props.topLevelKeyword = prefs.topLevelKeyword or "LrGeniusAI"
    
    -- Validation option
    props.enableValidation = prefs.retrieveEnableValidation or false

    local scopeItems = {
        { title = LOC "$$$/LrGeniusAI/Scope/Selected=Selected Photos", value = 'selected' },
        { title = LOC "$$$/LrGeniusAI/Scope/View=Current View", value = 'view' },
        { title = LOC "$$$/LrGeniusAI/Scope/All=All Photos", value = 'all' },
    }

    local dialogView = f:column {
        bind_to_object = props,
        spacing = f:control_spacing(),
        
        -- Scope Selection
        f:group_box {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Scope=Photo Selection",
            fill_horizontal = 1,
            f:row {
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/RetrieveMetadata/ScopeLabel=Apply to:",
                    width = share 'labelWidth',
                },
                f:popup_menu {
                    value = bind 'scope',
                    items = scopeItems,
                    fill_horizontal = 1,
                },
            },
        },
        
        -- Data Fields to Apply
        f:group_box {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/DataFields=Data Fields to Apply",
            fill_horizontal = 1,
            f:column {
                spacing = f:control_spacing(),
                f:row {
                    f:checkbox {
                        value = bind 'applyKeywords',
                        width = share 'checkboxWidth',
                    },
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Keywords=Keywords",
                    },
                },
                f:row {
                    f:checkbox {
                        value = bind 'applyTitle',
                        width = share 'checkboxWidth',
                    },
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Title=Title",
                    },
                },
                f:row {
                    f:checkbox {
                        value = bind 'applyCaption',
                        width = share 'checkboxWidth',
                    },
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Caption=Caption",
                    },
                },
                f:row {
                    f:checkbox {
                        value = bind 'applyAltText',
                        width = share 'checkboxWidth',
                    },
                    f:static_text {
                        title = LOC "$$$/LrGeniusAI/RetrieveMetadata/AltText=Alt Text",
                    },
                },
                -- f:row {
                --     f:checkbox {
                --         value = bind 'applyQuality',
                --         width = share 'checkboxWidth',
                --     },
                --     f:static_text {
                --         title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Quality=Quality Ratings & Critique",
                --     },
                -- },
            },
        },
        
        -- Validation Option
        f:group_box {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Validation=Validation",
            fill_horizontal = 1,
            f:row {
                f:checkbox {
                    value = bind 'enableValidation',
                },
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/RetrieveMetadata/EnableValidation=Review data before applying to catalog",
                },
            },
        },

        -- Keyword Option
        f:group_box {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/UseTopLevelKeyword=Use top-level keyword for applied keywords",
            fill_horizontal = 1,
            f:row {
                f:checkbox {
                    value = bind 'useTopLevelKeyword',
                },
                f:edit_field {
                    value = bind 'topLevelKeyword',
                    width_in_chars = 20,
                    enabled = bind 'useTopLevelKeyword',
                },
            },
        },
    }

    local result = LrDialogs.presentModalDialog {
        title = LOC "$$$/LrGeniusAI/RetrieveMetadata/DialogTitle=Retrieve Metadata from Backend",
        contents = dialogView,
        actionVerb = LOC "$$$/LrGeniusAI/RetrieveMetadata/ActionVerb=Retrieve",
    }

    if result == 'ok' then
        -- Save preferences
        prefs.retrieveMetadataScope = props.scope
        prefs.applyKeywords = props.applyKeywords
        prefs.applyTitle = props.applyTitle
        prefs.applyCaption = props.applyCaption
        prefs.applyAltText = props.applyAltText
        prefs.applyQuality = props.applyQuality
        prefs.retrieveEnableValidation = props.enableValidation
        prefs.useTopLevelKeyword = props.useTopLevelKeyword
        prefs.topLevelKeyword = props.topLevelKeyword
        
        return {
            scope = props.scope,
            applyKeywords = props.applyKeywords,
            applyTitle = props.applyTitle,
            applyCaption = props.applyCaption,
            applyAltText = props.applyAltText,
            applyQuality = props.applyQuality,
            enableValidation = props.enableValidation,
            useTopLevelKeyword = props.useTopLevelKeyword,
            topLevelKeyword = props.topLevelKeyword,
        }
    else
        return nil
    end
end


---
-- Main task function.
--
LrTasks.startAsyncTask(function()
    LrFunctionContext.callWithContext('retrieveMetadataTask', function(ctx)

        -- Check server connection
        if not Util.waitForServerDialog() then return end


        -- Show configuration dialog
        local options = showRetrieveMetadataDialog(ctx)
        if not options then
            log:info("Retrieve metadata task canceled by user")
            return
        end

        -- Get photos based on scope
        local photos, status = PhotoSelector.getPhotosInScope(options.scope)
        if not photos or #photos == 0 then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/RetrieveMetadata/NoPhotos=No Photos",
                LOC "$$$/LrGeniusAI/RetrieveMetadata/NoPhotosMessage=No photos found in the selected scope.",
                "info"
            )
            return
        end

        log:info(string.format("Starting metadata retrieval for %d photos", #photos))

        -- Progress indicator
        local progressScope = LrProgressScope {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/ProgressTitle=Retrieving Metadata from Backend",
        }

        local successCount = 0
        local skipCount = 0
        local errorCount = 0
        local skipValidation = false

        for i, photo in ipairs(photos) do
            if progressScope:isCanceled() then
                log:info("Retrieve metadata task canceled by user")
                break
            end

            local fileName = photo:getFormattedMetadata('fileName')
            progressScope:setPortionComplete(i - 1, #photos)
            progressScope:setCaption(string.format(LOC "$$$/LrGeniusAI/RetrieveMetadata/Processing=Processing %s (%d of %d)", 
                fileName, i, #photos))

            local uuid = photo:getRawMetadata('uuid')
            if uuid then
                if not SearchIndexAPI.pingServer() then
                    LrDialogs.message(
                        LOC "$$$/LrGeniusAI/RetrieveMetadata/ServerUnreachable=Server Unreachable",
                        LOC "$$$/LrGeniusAI/RetrieveMetadata/ServerUnreachableMessage=Cannot reach backend server. Please check your connection and try again.",
                        "error"
                    )
                    log:error("Backend server unreachable during metadata retrieval")
                    break
                end
                -- Retrieve data from backend
                log:trace("Retrieving data for UUID: " .. uuid)
                local retrievedData = SearchIndexAPI.getPhotoData(uuid)

                log:trace("Retrieved data: " .. Util.dumpTable(retrievedData))
                
                if retrievedData and retrievedData.status == "success" then
                    -- Validate if requested and not skipped
                    local shouldApply = true
                    local validatedData = nil
                    local result = nil
                    if options.enableValidation and not skipValidation then
                        result, validatedData = MetadataManager.showValidationDialog(ctx, photo, retrievedData, options)
                        
                        if result == "ok" then
                            successCount = successCount + 1
                            if validatedData ~= nil and validatedData.skipFromHere then
                                skipValidation = true
                            end
                        elseif result == "other" then
                            skipCount = skipCount + 1
                            shouldApply = false
                            validatedData = nil
                        else
                            -- Validation canceled
                            break
                        end
                    end

                    -- Apply metadata
                    if shouldApply then
                        MetadataManager.applyMetadata(photo, retrievedData, validatedData, options)
                        successCount = successCount + 1
                        log:trace("Metadata applied successfully for photo: " .. fileName)
                        
                        -- Overwrite with validated data if any
                        if result ~= nil and result == "ok" then
                            SearchIndexAPI.importMetadataFromCatalog({ photo }, progressScope)
                        end
                    end
                else
                    log:warn("No data found in backend for photo: " .. fileName)
                    errorCount = errorCount + 1
                end
            else
                log:warn("Photo has no UUID, skipping: " .. fileName)
                errorCount = errorCount + 1
            end
        end

        progressScope:done()

        log:info(string.format("Retrieve metadata task complete. Success: %d, Skipped: %d, Errors: %d", 
            successCount, skipCount, errorCount))
    end)
end)
