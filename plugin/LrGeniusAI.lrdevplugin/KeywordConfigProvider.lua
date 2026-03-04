KeywordConfigProvider = {}

function KeywordConfigProvider.showKeywordCategoryDialog()
    local f = LrView.osFactory()
    local bind = LrView.bind
    local share = LrView.share

    local propertyTable = {}

    local keywords = {}
    -- keywords = Defaults.defaultKeywordCategories
    if prefs.keywordCategories ~= nil and type(prefs.keywordCategories) == "table" then
        keywords = prefs.keywordCategories
    else
        log:trace("prefs.keywordCategories is not a table, using default categories.")
        prefs.keywordCategories = Defaults.defaultKeywordCategories
        keywords = prefs.keywordCategories
    end

    table.sort(keywords)

    local editFields = {}
    for i = 1, #keywords do
        local key = "keywordCategory_" .. i
        propertyTable[key] = keywords[i]
        table.insert(editFields, f:edit_field { value = bind(key), immediate = true })
    end
    editFields.title = LOC "$$$/lrc-ai-assistant/ResponseStructure/ConfigureResponseStructure=Keywords"
    editFields.width = 280

    local keywordBox = f:group_box(editFields)

    local dialogView = f:row {
        bind_to_object = propertyTable,
        f:scrolled_view {
            keywordBox,
            height = 500,
            width = 300,
            horizontal_scroller = false,
            vertical_scroller = true,
        },
        f:column {
            f:group_box {
                title = LOC "$$$/lrc-ai-assistant/ResponseStructure/NewKeywordCategory=New Category",
                f:edit_field {
                    value = bind 'new',
                },
                f:push_button {
                    title = LOC "$$$/lrc-ai-assistant/ResponseStructure/AddKeywordCategory=Add",
                    action = function (button)
                        table.insert(prefs.keywordCategories, propertyTable.new)
                        LrDialogs.stopModalWithResult(keywordBox, "cancel")
                        KeywordConfigProvider.showKeywordCategoryDialog()
                    end
                }
            },
        },
    }

    local result = LrDialogs.presentModalDialog({
        title = LOC "$$$/lrc-ai-assistant/ResponseStructure/ConfigureResponseStructure=Configure data generation and mapping",
        contents = dialogView,
        otherVerb = LOC "$$$/lrc-ai-assistant/ResponseStructure/ResetToDefault=Reset to defaults"
    })

    if result == 'ok' then
        log:trace('Saving changes to keyword categories.')
        prefs.keywordCategories = {}
        for i = 1, #keywords do
            local key = "keywordCategory_" .. i
            if propertyTable[key] ~= nil and propertyTable[key] ~= "" then 
                table.insert(prefs.keywordCategories, propertyTable[key])
            end
        end
        log:trace(Util.dumpTable(prefs.keywordCategories))
    elseif result == 'other' then
        local confirm = LrDialogs.confirm(LOC "$$$/lrc-ai-assistant/ResponseStructure/ResetToDefaultKeywordStructure=Reset to default keyword structure?")
        if confirm == 'ok' then
            log:trace("Reset keyword categories to default")
            prefs.keywordCategories = Defaults.defaultKeywordCategories
        end
    end
end


function KeywordConfigProvider.getKeywordCategories()
    if prefs.keywordCategories == nil or type(prefs.keywordCategories) ~= "table" then
        log:trace("prefs.keywordCategories is not a table, using default categories.")
        prefs.keywordCategories = Defaults.defaultKeywordCategories
    end
    return prefs.keywordCategories
end