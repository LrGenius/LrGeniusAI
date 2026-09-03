PromptConfigProvider = {}

function PromptConfigProvider.deletePrompt(props)
	local promptTitle = props.prompt
	if promptTitle == Defaults.defaultPromptName then
		LrDialogs.showError(
			LOC("$$$/LrGeniusAI/PromptConfig/DefaultPromptCannotDelete=Default prompt cannot be deleted.")
		)
		return nil
	end

	if
		LrDialogs.confirm(
			LOC("$$$/LrGeniusAI/PromptConfig/DeletePromptConfirm=Do you really want to delete the prompt")
				.. " "
				.. promptTitle
		) == "ok"
	then
		props.prompts[promptTitle] = nil
		-- Rebuilt rather than nil'd out of the array: clearing one slot of a
		-- Lua array leaves a hole, and the menu then stops at it, hiding every
		-- prompt after the deleted one.
		props.promptTitles = Util.promptMenuItems(props.prompts, Defaults.defaultPromptName)
		if props.promptTitleMenu then
			props.promptTitleMenu.items = props.promptTitles
		end

		-- Before the menu is asked to show it: the selection has to name a
		-- prompt that is still there, or the picker is left with a value none
		-- of its items carry. Util.resolvePromptName has what that costs.
		props.prompt = Util.resolvePromptName(props.prompts, props.prompt, Defaults.defaultPromptName)
		props.selectedPrompt = (props.prompt ~= nil and props.prompts[props.prompt]) or ""
	end
end

function PromptConfigProvider.addPrompt(props)
	local f = LrView.osFactory()
	local bind = LrView.bind
	local share = LrView.share

	local propertyTable = {}

	local dialogView = f:column({
		bind_to_object = propertyTable,
		f:row({
			f:static_text({
				width = share("labelWidth"),
				title = LOC("$$$/LrGeniusAI/PromptConfig/PromptName=Prompt name"),
			}),
			f:edit_field({
				value = bind("name"),
				width = 500,
			}),
		}),
		f:row({
			f:static_text({
				width = share("labelWidth"),
				title = LOC("$$$/LrGeniusAI/PromptConfig/PromptField=Prompt"),
			}),
			f:scrolled_view({
				horizontal_scroller = false,
				vertical_scroller = true,
				width = 500,
				f:edit_field({
					value = bind("prompt"),
					width = 480,
					height_in_lines = 30,
					wraps = true,
				}),
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/PromptConfig/AddNewPrompt=Add new prompt"),
		contents = dialogView,
	})

	if result == "ok" then
		-- A name is what the prompt is stored and selected under, so an empty
		-- one is not a prompt with no name: `prompts[nil] = text` raises
		-- "table index is nil" from inside the dialog's button action, and an
		-- empty string would add a nameless entry to the picker.
		local name = Util.trim(propertyTable.name or "")
		if name == "" then
			LrDialogs.message(
				"The prompt needs a name.",
				"Give the prompt a name and add it again — the name is what the Template menu lists it under.",
				"warning"
			)
			return nil
		end

		Util.storePromptText(props.prompts, name, propertyTable.prompt)
		props.prompt = name
		props.selectedPrompt = props.prompts[name]
		props.promptTitles = Util.promptMenuItems(props.prompts, Defaults.defaultPromptName)
		if props.promptTitleMenu then
			props.promptTitleMenu.items = props.promptTitles
		end
		return name
	end

	return nil
end

function PromptConfigProvider.showPromptConfigDialog(propertyTable)
	local f = LrView.osFactory()
	local bind = LrView.bind
	local share = LrView.share

	propertyTable.promptTitles = Util.promptMenuItems(prefs.prompts, Defaults.defaultPromptName)

	propertyTable.prompts = prefs.prompts

	propertyTable.prompt = prefs.prompt

	propertyTable.selectedPrompt = prefs.prompts[prefs.prompt]

	propertyTable:addObserver("prompt", function(properties, key, newValue)
		properties.selectedPrompt = properties.prompts[newValue]
	end)

	propertyTable:addObserver("selectedPrompt", function(properties, key, newValue)
		properties.prompts[properties.prompt] = newValue
	end)

	local dropDown = f:popup_menu({
		items = bind("promptTitles"),
		value = bind("prompt"),
	})

	local dialogView = f:column({
		bind_to_object = propertyTable,
		f:row({
			f:static_text({
				width = share("labelWidth"),
				title = LOC("$$$/LrGeniusAI/PromptConfig/PromptName=Prompt name"),
			}),
			dropDown,
			f:push_button({
				title = LOC("$$$/LrGeniusAI/PromptConfig/Add=Add"),
				action = function(button)
					local newName = PromptConfigProvider.addPrompt(propertyTable)
					if newName ~= nil then
						LrDialogs.stopModalWithResult(dropDown, "cancel")
						PromptConfigProvider.showPromptConfigDialog(propertyTable)
					end
				end,
			}),
			f:push_button({
				title = LOC("$$$/LrGeniusAI/PromptConfig/Delete=Delete"),
				action = function(button)
					PromptConfigProvider.deletePrompt(propertyTable)
					LrDialogs.stopModalWithResult(dropDown, "cancel")
					PromptConfigProvider.showPromptConfigDialog(propertyTable)
				end,
			}),
			-- f:push_button {
			--     title = "Edit",
			--     action = function(button)
			--         editPrompt(propertyTable.prompt)
			--         LrDialogs.stopModalWithResult(dropDown)
			--         PromptConfigProvider.showPromptConfigDialog()
			--     end,
			-- },
			-- f:push_button {
			--     title = "Select",
			--     action = function(button)
			--         propertyTable.selectedPrompt = propertyTable.prompts[propertyTable.prompt]
			--     end,
			-- },
		}),
		f:row({
			f:static_text({
				width = share("labelWidth"),
				title = LOC("$$$/LrGeniusAI/PromptConfig/PromptField=Prompt"),
			}),
			f:edit_field({
				value = bind("selectedPrompt"),
				width_in_chars = 50,
				height_in_lines = 10,
				-- enabled = false,
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/PromptConfig/ConfigurePrompts=Configure Prompts"),
		contents = dialogView,
		otherVerb = LOC("$$$/lrc-ai-assistant/ResponseStructure/ResetToDefault=Reset to defaults"),
	})

	if result == "ok" then
		prefs.prompts = propertyTable.prompts
		prefs.prompt = propertyTable.prompt
	elseif result == "other" then
		prefs.prompts = Defaults.builtinPromptTable()
		prefs.prompt = Defaults.defaultPromptName
		-- The built-ins are back, so the record of what has been offered has to
		-- agree with that; otherwise nothing changes, since each name is only
		-- ever seeded once.
		local seeded = {}
		for _, preset in ipairs(Defaults.builtinPrompts) do
			seeded[preset.name] = true
		end
		prefs.seededPrompts = seeded
	end
end
