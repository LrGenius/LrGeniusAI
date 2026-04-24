require("DevelopEditManager")

local function copyOptions(source)
	local copied = {}
	for key, value in pairs(source or {}) do
		copied[key] = value
	end
	return copied
end

local function safePromptTable(rawPrompts)
	if type(rawPrompts) ~= "table" then
		log:warn(
			"AI Edit prompt table invalid type: " .. tostring(type(rawPrompts)) .. ". Falling back to default prompt."
		)
		return { Default = Defaults.defaultEditSystemInstruction }
	end
	return rawPrompts
end

local function buildModelItems()
	local items = {}
	local openaiKey = (prefs and not Util.nilOrEmpty(prefs.chatgptApiKey)) and prefs.chatgptApiKey or nil
	local geminiKey = (prefs and not Util.nilOrEmpty(prefs.geminiApiKey)) and prefs.geminiApiKey or nil
	local modelsResp = SearchIndexAPI.getModels(openaiKey, geminiKey)
	if modelsResp and modelsResp.models then
		for provider, modelList in pairs(modelsResp.models) do
			for _, model in ipairs(modelList) do
				table.insert(items, {
					title = provider .. ": " .. model,
					value = provider .. "::" .. model,
				})
			end
		end
	end
	table.sort(items, function(a, b)
		return a.title < b.title
	end)
	return items
end

local function getEditIntentPresetInstruction(presetValue)
	for _, preset in ipairs(Defaults.editIntentPresets or {}) do
		if preset.value == presetValue then
			return preset.instruction
		end
	end
	return nil
end

local function hasEditIntentPresetValue(presetValue)
	for _, preset in ipairs(Defaults.editIntentPresets or {}) do
		if preset.value == presetValue then
			return true
		end
	end
	return false
end

local function buildEditIntentPresetItems()
	local items = {}
	for _, preset in ipairs(Defaults.editIntentPresets or {}) do
		table.insert(items, { title = preset.title, value = preset.value })
	end
	if #items == 0 then
		table.insert(items, {
			title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/Custom=Custom"),
			value = Defaults.editIntentCustomValue or "custom",
		})
	end
	return items
end

local function hasCompositionModeValue(value)
	for _, item in ipairs(Defaults.compositionModes or {}) do
		if item.value == value then
			return true
		end
	end
	return false
end

local function showPhotoInstructionDialog(ctx, photo)
	local f = LrView.osFactory()
	local bind = LrView.bind

	local props = LrBinding.makePropertyTable(ctx)
	props.photoContextData = photo:getPropertyForPlugin(_PLUGIN, "photoContext") or ""
	props.skipFromHere = false

	local dialogView = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:row({
			f:static_text({
				title = photo:getFormattedMetadata("fileName") or "Photo",
			}),
		}),
		f:row({
			alignment = "center",
			f:catalog_photo({
				photo = photo,
				width = 300,
			}),
		}),
		f:row({
			f:static_text({
				title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/PerPhotoInstructions=Per-photo edit instructions"),
			}),
		}),
		f:row({
			f:edit_field({
				value = bind("photoContextData"),
				width_in_chars = 50,
				height_in_lines = 10,
			}),
		}),
		f:row({
			f:checkbox({
				value = bind("skipFromHere"),
			}),
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/TaskAiEditPhotos/UseForFollowing=Use these instructions for all following photos."
				),
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/PhotoSpecificInstructions=Photo-specific edit instructions"),
		contents = dialogView,
		actionVerb = LOC("$$$/LrGeniusAI/common/Continue=Continue"),
	})

	return result, props.photoContextData, props.skipFromHere
end

local function showAiEditDialog(ctx)
	log:trace("showAiEditDialog: start")
	local f = LrView.osFactory()
	local bind = LrView.bind
	local share = LrView.share
	local props = LrBinding.makePropertyTable(ctx)

	props.scope = prefs.aiEditScope or "selected"
	props.modelKey = prefs.aiEditModelKey or prefs.modelKey
	props.temperature = prefs.aiEditTemperature or prefs.temperature or 0.1
	props.language = prefs.aiEditLanguage or prefs.generateLanguage or "English"
	props.styleStrength = prefs.aiEditStyleStrength or Defaults.defaultEditStyleStrength or 0.5
	props.editIntentPresetItems = buildEditIntentPresetItems()
	props.customEditIntentText = prefs.aiEditIntentCustomText or prefs.aiEditIntent or Defaults.defaultEditIntent
	if type(props.customEditIntentText) ~= "string" or props.customEditIntentText == "" then
		props.customEditIntentText = Defaults.defaultEditIntent
	end
	props.editIntentPreset = prefs.aiEditIntentPreset
		or Defaults.defaultEditIntentPresetValue
		or (Defaults.editIntentCustomValue or "custom")
	if not hasEditIntentPresetValue(props.editIntentPreset) then
		props.editIntentPreset = Defaults.editIntentCustomValue or "custom"
	end
	props.isCustomEditIntent = props.editIntentPreset == (Defaults.editIntentCustomValue or "custom")
	if props.isCustomEditIntent then
		props.editIntent = props.customEditIntentText
	else
		props.editIntent = getEditIntentPresetInstruction(props.editIntentPreset) or Defaults.defaultEditIntent
	end
	props.reviewBeforeApply = prefs.aiEditReviewBeforeApply ~= false
	props.applyMasks = prefs.aiEditApplyMasks ~= false
	props.adjustWhiteBalance = prefs.aiEditAdjustWhiteBalance ~= false
	props.adjustBasicTone = prefs.aiEditAdjustBasicTone ~= false
	props.adjustPresence = prefs.aiEditAdjustPresence ~= false
	props.adjustColorMix = prefs.aiEditAdjustColorMix ~= false
	props.doColorGrading = prefs.aiEditDoColorGrading ~= false
	props.useToneCurve = prefs.aiEditUseToneCurve ~= false
	props.usePointCurve = prefs.aiEditUsePointCurve ~= false
	props.adjustDetail = prefs.aiEditAdjustDetail ~= false
	props.adjustEffects = prefs.aiEditAdjustEffects ~= false
	props.adjustLensCorrections = prefs.aiEditAdjustLensCorrections ~= false
	props.allowAutoCrop = prefs.aiEditAllowAutoCrop ~= false
	props.compositionModes = Defaults.compositionModes or {}
	props.compositionMode = prefs.aiEditCompositionMode or Defaults.defaultCompositionMode or "subtle"
	if not hasCompositionModeValue(props.compositionMode) then
		props.compositionMode = Defaults.defaultCompositionMode or "subtle"
	end
	props.submitKeywords = prefs.aiEditSubmitKeywords ~= false
	props.submitFolderName = prefs.aiEditSubmitFolderName or false
	props.showPhotoContextDialog = prefs.aiEditShowPhotoContextDialog ~= false
	props.useTrainingStyle = prefs.aiEditUseTrainingStyle ~= false
	props.promptTitles = {}
	props.prompts = safePromptTable(prefs.editPrompts or { Default = Defaults.defaultEditSystemInstruction })
	log:trace("showAiEditDialog: prompt source type=" .. tostring(type(props.prompts)))
	props.prompt = prefs.editPrompt or Defaults.defaultEditPromptName
	if type(props.prompt) ~= "string" or props.prompt == "" then
		props.prompt = Defaults.defaultEditPromptName
	end
	props.selectedPrompt = props.prompts[props.prompt]
	if type(props.selectedPrompt) ~= "string" or props.selectedPrompt == "" then
		props.prompt = Defaults.defaultEditPromptName
		props.selectedPrompt = props.prompts[props.prompt] or Defaults.defaultEditSystemInstruction
	end

	for title, prompt in pairs(props.prompts) do
		if type(title) == "string" and title ~= "" and type(prompt) == "string" then
			table.insert(props.promptTitles, { title = title, value = title })
		end
	end
	log:trace("showAiEditDialog: promptTitles count=" .. tostring(#props.promptTitles))
	if #props.promptTitles == 0 then
		props.prompts = { Default = Defaults.defaultEditSystemInstruction }
		props.prompt = Defaults.defaultEditPromptName
		props.selectedPrompt = Defaults.defaultEditSystemInstruction
		table.insert(
			props.promptTitles,
			{ title = Defaults.defaultEditPromptName, value = Defaults.defaultEditPromptName }
		)
	end
	table.sort(props.promptTitles, function(a, b)
		return a.title < b.title
	end)

	props:addObserver("prompt", function(properties, key, newValue)
		properties.selectedPrompt = properties.prompts[newValue]
	end)
	props:addObserver("selectedPrompt", function(properties, key, newValue)
		properties.prompts[properties.prompt] = newValue
	end)
	props:addObserver("editIntentPreset", function(properties, key, newValue)
		local customValue = Defaults.editIntentCustomValue or "custom"
		properties.isCustomEditIntent = newValue == customValue
		if properties.isCustomEditIntent then
			properties.editIntent = properties.customEditIntentText or Defaults.defaultEditIntent
		else
			properties.editIntent = getEditIntentPresetInstruction(newValue) or Defaults.defaultEditIntent
		end
	end)
	props:addObserver("editIntent", function(properties, key, newValue)
		if properties.isCustomEditIntent then
			properties.customEditIntentText = newValue
		end
	end)

	local modelItems = buildModelItems()
	log:trace("showAiEditDialog: modelItems count=" .. tostring(#modelItems))
	if #modelItems == 0 then
		table.insert(modelItems, { title = "chatgpt: gpt-4.1", value = "chatgpt::gpt-4.1" })
	end
	if not props.modelKey or props.modelKey == "" then
		props.modelKey = modelItems[1].value
	end

	props.promptTitleMenu = f:popup_menu({
		items = bind("promptTitles"),
		value = bind("prompt"),
	})

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/common/Scope=Scope"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/common/ApplyTo=Apply to:"),
					width = share("labelWidth"),
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
			title = LOC("$$$/LrGeniusAI/common/AiSettings=AI Settings"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/common/AiModel=AI model:"),
					width = share("labelWidth"),
				}),
				f:popup_menu({
					value = bind("modelKey"),
					items = modelItems,
					width = 300,
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/common/Temperature=Temperature:"),
					width = share("labelWidth"),
				}),
				f:slider({
					value = bind("temperature"),
					min = 0.0,
					max = 0.5,
					integral = false,
					width = 300,
				}),
				f:static_text({
					title = bind("temperature"),
					width = 40,
				}),
			}),
			f:row({
				f:static_text({
					width = share("labelWidth"),
					title = LOC("$$$/LrGeniusAI/common/Prompt=Prompt:"),
				}),
				props.promptTitleMenu,
				f:push_button({
					title = LOC("$$$/LrGeniusAI/common/Add=Add"),
					action = function()
						local ok, err = LrTasks.pcall(function()
							PromptConfigProvider.addPrompt(props)
						end)
						if not ok then
							log:error("AI Edit prompt add failed: " .. tostring(err))
							LrDialogs.showError(
								LOC("$$$/LrGeniusAI/PromptConfig/AddFailed=Adding prompt failed: ^1"),
								tostring(err)
							)
						end
					end,
				}),
				f:push_button({
					title = LOC("$$$/LrGeniusAI/common/Delete=Delete"),
					action = function()
						local ok, err = LrTasks.pcall(function()
							PromptConfigProvider.deletePrompt(props)
						end)
						if not ok then
							log:error("AI Edit prompt delete failed: " .. tostring(err))
							LrDialogs.showError(
								LOC("$$$/LrGeniusAI/PromptConfig/DeleteFailed=Deleting prompt failed: ^1"),
								tostring(err)
							)
						end
					end,
				}),
			}),
			f:row({
				f:static_text({
					width = share("labelWidth"),
					title = LOC("$$$/LrGeniusAI/common/SystemInstruction=System instruction:"),
				}),
				f:edit_field({
					value = bind("selectedPrompt"),
					width_in_chars = 50,
					height_in_lines = 4,
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/common/SummaryLanguage=Summary language:"),
					width = share("labelWidth"),
				}),
				f:combo_box({
					value = bind("language"),
					items = Defaults.generateLanguages,
				}),
			}),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/EditInstructions=Edit Instructions"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/OverallLook=Overall look:"),
					width = share("labelWidth"),
				}),
				f:popup_menu({
					value = bind("editIntentPreset"),
					items = bind("editIntentPresetItems"),
					width = 300,
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/CustomIntent=Custom intent:"),
					width = share("labelWidth"),
				}),
				f:edit_field({
					value = bind("editIntent"),
					width_in_chars = 50,
					enabled = bind("isCustomEditIntent"),
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/StyleStrength=Style strength:"),
					width = share("labelWidth"),
				}),
				f:slider({
					value = bind("styleStrength"),
					min = 0.0,
					max = 1.0,
					integral = false,
					width = 300,
				}),
				f:static_text({
					title = bind("styleStrength"),
					width = 40,
				}),
			}),
			f:row({
				f:checkbox({
					value = bind("reviewBeforeApply"),
				}),
				f:static_text({
					title = LOC(
						"$$$/LrGeniusAI/TaskAiEditPhotos/ReviewProposed=Review each proposed edit before applying it"
					),
				}),
			}),
			f:row({
				f:checkbox({
					value = bind("applyMasks"),
				}),
				f:static_text({
					title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/AskMasks=Ask the AI for subject/sky/background masks"),
				}),
			}),
			f:row({
				f:checkbox({
					value = bind("showPhotoContextDialog"),
				}),
				f:static_text({
					title = LOC(
						"$$$/LrGeniusAI/TaskAiEditPhotos/AllowPerPhoto=Allow per-photo edit instructions before generation"
					),
				}),
			}),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/CreativeControls=Creative Controls"),
			fill_horizontal = 1,
			f:row({
				f:column({
					spacing = f:control_spacing(),
					f:row({
						f:checkbox({
							value = bind("adjustWhiteBalance"),
						}),
						f:static_text({
							title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/AdjustWB=Adjust white balance"),
						}),
					}),
					f:row({
						f:checkbox({
							value = bind("adjustBasicTone"),
						}),
						f:static_text({
							title = LOC(
								"$$$/LrGeniusAI/TaskAiEditPhotos/AdjustBasicTone=Adjust basic tone (exposure/contrast/highlights/shadows/whites/blacks)"
							),
						}),
					}),
					f:row({
						f:checkbox({
							value = bind("adjustPresence"),
						}),
						f:static_text({
							title = LOC(
								"$$$/LrGeniusAI/TaskAiEditPhotos/AdjustPresence=Adjust presence (texture/clarity/dehaze)"
							),
						}),
					}),
					f:row({
						f:checkbox({
							value = bind("adjustColorMix"),
						}),
						f:static_text({
							title = LOC(
								"$$$/LrGeniusAI/TaskAiEditPhotos/AdjustColorMix=Adjust color mix (vibrance/saturation/HSL)"
							),
						}),
					}),
					f:row({
						f:checkbox({
							value = bind("doColorGrading"),
						}),
						f:static_text({
							title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/DoColorGrading=Do color grading"),
						}),
					}),
				}),
				f:column({
					spacing = f:control_spacing(),
					f:row({
						f:checkbox({
							value = bind("useToneCurve"),
						}),
						f:static_text({
							title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/UseToneCurve=Use tone curve"),
						}),
					}),
					f:row({
						f:checkbox({
							value = bind("usePointCurve"),
							enabled = bind("useToneCurve"),
						}),
						f:static_text({
							title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/UsePointCurve=Use point curve"),
						}),
					}),
					f:row({
						f:checkbox({
							value = bind("adjustDetail"),
						}),
						f:static_text({
							title = LOC(
								"$$$/LrGeniusAI/TaskAiEditPhotos/AdjustDetail=Adjust detail (sharpening/noise reduction)"
							),
						}),
					}),
					f:row({
						f:checkbox({
							value = bind("adjustEffects"),
						}),
						f:static_text({
							title = LOC(
								"$$$/LrGeniusAI/TaskAiEditPhotos/AdjustEffects=Adjust effects (vignette/grain)"
							),
						}),
					}),
					f:row({
						f:checkbox({
							value = bind("adjustLensCorrections"),
						}),
						f:static_text({
							title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/AdjustLens=Adjust lens corrections"),
						}),
					}),
					f:row({
						f:checkbox({
							value = bind("allowAutoCrop"),
						}),
						f:static_text({
							title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/AllowAutoCrop=Allow AI auto crop"),
						}),
					}),
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/CompositionMode=Composition mode:"),
					width = share("labelWidth"),
				}),
				f:popup_menu({
					value = bind("compositionMode"),
					items = bind("compositionModes"),
					width = 300,
				}),
			}),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/common/Context=Context"),
			fill_horizontal = 1,
			f:row({
				f:column({
					spacing = f:control_spacing(),
					f:row({
						f:checkbox({
							value = bind("submitKeywords"),
						}),
						f:static_text({
							title = LOC(
								"$$$/LrGeniusAI/TaskAiEditPhotos/SendKeywords=Send existing Lightroom keywords"
							),
						}),
					}),
				}),
				f:column({
					spacing = f:control_spacing(),
					f:row({
						f:checkbox({
							value = bind("submitFolderName"),
						}),
						f:static_text({
							title = LOC("$$$/LrGeniusAI/TaskAiEditPhotos/SendFolders=Send folder names"),
						}),
					}),
					f:row({
						f:checkbox({
							value = bind("useTrainingStyle"),
						}),
						f:static_text({
							title = LOC(
								"$$$/LrGeniusAI/Training/UseTrainingCheckbox=Apply my saved edit style (training examples)"
							),
						}),
					}),
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
	prefs.aiEditModelKey = props.modelKey
	prefs.aiEditTemperature = props.temperature
	prefs.aiEditLanguage = props.language
	prefs.aiEditStyleStrength = props.styleStrength
	prefs.aiEditIntent = props.editIntent
	prefs.aiEditIntentPreset = props.editIntentPreset
	prefs.aiEditIntentCustomText = props.customEditIntentText
	prefs.aiEditReviewBeforeApply = props.reviewBeforeApply
	prefs.aiEditApplyMasks = props.applyMasks
	prefs.aiEditAdjustWhiteBalance = props.adjustWhiteBalance
	prefs.aiEditAdjustBasicTone = props.adjustBasicTone
	prefs.aiEditAdjustPresence = props.adjustPresence
	prefs.aiEditAdjustColorMix = props.adjustColorMix
	prefs.aiEditDoColorGrading = props.doColorGrading
	prefs.aiEditUseToneCurve = props.useToneCurve
	prefs.aiEditUsePointCurve = props.usePointCurve
	prefs.aiEditAdjustDetail = props.adjustDetail
	prefs.aiEditAdjustEffects = props.adjustEffects
	prefs.aiEditAdjustLensCorrections = props.adjustLensCorrections
	prefs.aiEditAllowAutoCrop = props.allowAutoCrop
	prefs.aiEditCompositionMode = props.compositionMode
	prefs.aiEditSubmitKeywords = props.submitKeywords
	prefs.aiEditSubmitFolderName = props.submitFolderName
	prefs.aiEditShowPhotoContextDialog = props.showPhotoContextDialog
	prefs.aiEditUseTrainingStyle = props.useTrainingStyle
	prefs.editPrompts = props.prompts
	prefs.editPrompt = props.prompt

	local providerFromKey, modelFromKey
	local sep = props.modelKey and string.find(props.modelKey, "::", 1, true) or nil
	if sep then
		providerFromKey = string.sub(props.modelKey, 1, sep - 1)
		modelFromKey = string.sub(props.modelKey, sep + 2)
	else
		providerFromKey = props.modelKey
	end

	local options = {
		scope = props.scope,
		provider = providerFromKey,
		model = modelFromKey,
		language = props.language,
		temperature = props.temperature,
		prompt = props.selectedPrompt,
		edit_intent = props.editIntent,
		style_strength = props.styleStrength,
		include_masks = props.applyMasks,
		adjust_white_balance = props.adjustWhiteBalance,
		adjust_basic_tone = props.adjustBasicTone,
		adjust_presence = props.adjustPresence,
		adjust_color_mix = props.adjustColorMix,
		do_color_grading = props.doColorGrading,
		use_tone_curve = props.useToneCurve,
		use_point_curve = props.usePointCurve,
		adjust_detail = props.adjustDetail,
		adjust_effects = props.adjustEffects,
		adjust_lens_corrections = props.adjustLensCorrections,
		allow_auto_crop = props.allowAutoCrop,
		composition_mode = props.compositionMode,
		applyMasks = props.applyMasks,
		reviewBeforeApply = props.reviewBeforeApply,
		submit_keywords = props.submitKeywords,
		submit_folder_names = props.submitFolderName,
		showPhotoContextDialog = props.showPhotoContextDialog,
		use_training_style = props.useTrainingStyle ~= false,
	}

	if providerFromKey == "chatgpt" then
		if prefs and not Util.nilOrEmpty(prefs.chatgptApiKey) then
			options.api_key = prefs.chatgptApiKey
		else
			LrDialogs.showError(
				LOC(
					"$$$/LrGeniusAI/AnalyzeAndIndex/MissingChatGPTAPIKey=ChatGPT API key is not configured. Please set it in the plugin preferences."
				)
			)
			return nil
		end
	elseif providerFromKey == "gemini" then
		if prefs and not Util.nilOrEmpty(prefs.geminiApiKey) then
			options.api_key = prefs.geminiApiKey
		else
			LrDialogs.showError(
				LOC(
					"$$$/LrGeniusAI/AnalyzeAndIndex/MissingGeminiAPIKey=Gemini API key is not configured. Please set it in the plugin preferences."
				)
			)
			return nil
		end
	end
	return options
end

local function enrichPhotoOptions(photo, baseOptions, userContext)
	log:trace("enrichPhotoOptions: start for " .. tostring(photo and photo:getFormattedMetadata("fileName") or "nil"))
	local photoOptions = copyOptions(baseOptions)
	if photoOptions.submit_keywords then
		local keywords = photo:getFormattedMetadata("keywordTagsForExport")
		if keywords then
			if type(keywords) == "string" then
				photoOptions.existing_keywords = Util.string_split(keywords, ",")
			else
				photoOptions.existing_keywords = keywords
			end
		end
	end
	if photoOptions.submit_folder_names then
		local originalFilePath = photo:getRawMetadata("path")
		if originalFilePath then
			photoOptions.folder_names = Util.getStringsFromRelativePath(originalFilePath)
		end
	end
	local datetime = photo:getRawMetadata("dateTime")
	if datetime ~= nil and type(datetime) == "number" then
		photoOptions.date_time = LrDate.timeToW3CDate(datetime)
		photoOptions.capture_time = datetime -- Unix timestamp for style engine
	end

	-- Add EXIF fields for style engine matching using standardized utility.
	local exif = Util.getPhotoExif(photo)
	for k, v in pairs(exif) do
		photoOptions[k] = v
	end
	photoOptions.user_context = userContext or photo:getPropertyForPlugin(_PLUGIN, "photoContext") or ""
	return photoOptions
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("AiEditPhotosTask", function(ctx)
		LrDialogs.attachErrorDialogToFunctionContext(ctx)
		log:info("AI Edit task started")

		-- Check server connection and health (ensure AI providers are configured)
		if not Util.waitForServerDialog({ requireProviders = true }) then
			log:warn("AI Edit task aborted: backend server unavailable")
			return
		end

		local options = showAiEditDialog(ctx)
		if not options then
			log:info("AI Edit task canceled by user in options dialog")
			return
		end
		log:trace(
			"AI Edit options selected: scope="
				.. tostring(options.scope)
				.. " provider="
				.. tostring(options.provider)
				.. " model="
				.. tostring(options.model)
				.. " review="
				.. tostring(options.reviewBeforeApply)
				.. " styleStrength="
				.. tostring(options.style_strength)
				.. " masks="
				.. tostring(options.applyMasks)
				.. " wb="
				.. tostring(options.adjust_white_balance)
				.. " basicTone="
				.. tostring(options.adjust_basic_tone)
				.. " presence="
				.. tostring(options.adjust_presence)
				.. " colorMix="
				.. tostring(options.adjust_color_mix)
				.. " grading="
				.. tostring(options.do_color_grading)
				.. " toneCurve="
				.. tostring(options.use_tone_curve)
				.. " pointCurve="
				.. tostring(options.use_point_curve)
				.. " detail="
				.. tostring(options.adjust_detail)
				.. " effects="
				.. tostring(options.adjust_effects)
				.. " lens="
				.. tostring(options.adjust_lens_corrections)
				.. " crop="
				.. tostring(options.allow_auto_crop)
				.. " composition="
				.. tostring(options.composition_mode)
		)

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

		local successCount = 0
		local skippedCount = 0
		local errorCount = 0
		local errorMessages = {}
		local backendWarnings = {}
		local reuseContext = false
		local sharedContext = ""

		for index, photo in ipairs(photos) do
			if progressScope:isCanceled() then
				break
			end

			local fileName = photo:getFormattedMetadata("fileName") or "Photo"
			progressScope:setCaption(
				"Processing " .. fileName .. " (" .. tostring(index) .. " of " .. tostring(#photos) .. ")"
			)
			progressScope:setPortionComplete(index - 1, #photos)
			local continueProcessing = true

			local userContext = photo:getPropertyForPlugin(_PLUGIN, "photoContext") or ""
			log:trace(
				"AI Edit photo loop start: index="
					.. tostring(index)
					.. " photo="
					.. tostring(fileName)
					.. " initialContextLen="
					.. tostring(type(userContext) == "string" and #userContext or 0)
			)
			if options.showPhotoContextDialog then
				if not reuseContext then
					local result
					result, sharedContext, reuseContext = showPhotoInstructionDialog(ctx, photo)
					if result == "cancel" then
						progressScope:done()
						return
					end
				end
				userContext = sharedContext or ""
				LrApplication.activeCatalog():withPrivateWriteAccessDo(function()
					photo:setPropertyForPlugin(_PLUGIN, "photoContext", userContext)
				end, Defaults.catalogWriteAccessOptions)
			end

			local photoId, photoIdErr = SearchIndexAPI.getPhotoIdForPhoto(photo)
			if not photoId then
				log:error("Failed to resolve photo ID for " .. fileName .. ": " .. tostring(photoIdErr))
				table.insert(errorMessages, fileName .. ": " .. tostring(photoIdErr))
				errorCount = errorCount + 1
				continueProcessing = false
			else
				log:trace("Resolved photo ID for " .. fileName .. ": " .. tostring(photoId))
			end

			local response = nil
			if continueProcessing then
				local photoOptions = enrichPhotoOptions(photo, options, userContext)
				local exportedPath = SearchIndexAPI.exportPhotoForIndexing(photo)
				if not exportedPath then
					log:error("Failed to export photo for AI edit generation: " .. fileName)
					table.insert(errorMessages, fileName .. ": export failed")
					errorCount = errorCount + 1
					continueProcessing = false
				end

				if continueProcessing then
					log:trace("AI Edit calling API for " .. fileName .. " exportedPath=" .. tostring(exportedPath))
					local ok, apiOk, apiResponse = LrTasks.pcall(function()
						if options.use_training_style then
							-- Route to the new Style Engine (LLM-free matching)
							-- Fallback to LLM is handled server-side if use_llm_fallback is true
							photoOptions.use_llm_fallback = true
							return SearchIndexAPI.styleEdit(photoId, exportedPath, photoOptions)
						else
							-- Regular LLM edit (prompt-driven)
							return SearchIndexAPI.generateEditRecipePhoto(photoId, exportedPath, photoOptions)
						end
					end)
					LrTasks.pcall(function()
						if exportedPath and LrFileUtils.exists(exportedPath) then
							LrFileUtils.delete(exportedPath)
						end
					end)
					if not ok then
						log:error("AI edit generation threw for " .. fileName .. ": " .. tostring(apiOk))
						table.insert(errorMessages, fileName .. ": exception thrown: " .. tostring(apiOk))
						errorCount = errorCount + 1
						continueProcessing = false
					else
						response = apiResponse
						if response and response.warning then
							table.insert(backendWarnings, fileName .. ": " .. tostring(response.warning))
						end
					end
					if
						continueProcessing
						and (not apiOk or not response or type(response) ~= "table" or response.status ~= "success")
					then
						local errMsg = "Unknown error"
						if not apiOk then
							errMsg = tostring(apiResponse)
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
						table.insert(errorMessages, fileName .. ": " .. errMsg)
						errorCount = errorCount + 1
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
				log:trace("Persisting generated recipe for " .. fileName)
				local okPersist, persistErr = LrTasks.pcall(function()
					DevelopEditManager.persistEditRecipe(photo, response, nil, "generated")
				end)
				if not okPersist then
					log:error("Persist generated recipe threw for " .. fileName .. ": " .. tostring(persistErr))
					table.insert(errorMessages, fileName .. ": could not persist recipe: " .. tostring(persistErr))
					errorCount = errorCount + 1
					continueProcessing = false
				end

				local applyOptions = {
					applyGlobal = true,
					applyMasks = options.applyMasks,
				}

				if options.reviewBeforeApply then
					log:trace("Showing review dialog for " .. fileName)
					local result, validated = DevelopEditManager.showValidationDialog(ctx, photo, response, options)
					log:trace("Review dialog result for " .. fileName .. ": " .. tostring(result))
					if result == "cancel" then
						skippedCount = skippedCount + 1
						continueProcessing = false
					elseif validated then
						applyOptions = validated
					end
				end

				if continueProcessing and not applyOptions.applyGlobal and not applyOptions.applyMasks then
					skippedCount = skippedCount + 1
					continueProcessing = false
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
					local applied, warnings = DevelopEditManager.applyRecipe(photo, response, applyOptions)
					log:trace(
						"Apply result for "
							.. fileName
							.. ": applied="
							.. tostring(applied)
							.. " warningsCount="
							.. tostring(type(warnings) == "table" and #warnings or 0)
					)
					if applied then
						successCount = successCount + 1
					else
						errorCount = errorCount + 1
						table.insert(errorMessages, fileName .. ": failed to apply recipe")
					end
					if warnings and #warnings > 0 then
						log:warn("AI edit warnings for " .. fileName .. ": " .. table.concat(warnings, " | "))
					end
				end
			end
		end

		progressScope:done()

		if errorCount > 0 or #backendWarnings > 0 then
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
				LOC("$$$/LrGeniusAI/TaskAiEditPhotos/Summary=Applied edits to ^1 photo(s).", tostring(successCount))
			if skippedCount > 0 then
				combinedReport = combinedReport
					.. "\n"
					.. LOC("$$$/LrGeniusAI/common/Skipped=Skipped: ^1", tostring(skippedCount))
			end
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

			if #backendWarnings > 0 then
				combinedReport = combinedReport
					.. "\n\n"
					.. LOC("$$$/LrGeniusAI/common/BackendWarnings=Backend Warnings:")
					.. "\n"
				for i = 1, math.min(5, #backendWarnings) do
					combinedReport = combinedReport .. "- " .. backendWarnings[i] .. "\n"
				end
				if #backendWarnings > 5 then
					combinedReport = combinedReport
						.. LOC(
							"$$$/LrGeniusAI/common/MoreWarnings=... and ^1 more warnings",
							tostring(#backendWarnings - 5)
						)
				end
			end

			ErrorHandler.handleError(
				LOC("$$$/LrGeniusAI/TaskAiEditPhotos/CompletionTitle=AI Edit Completed"),
				combinedReport
			)
		else
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/TaskAiEditPhotos/SuccessTitle=AI Lightroom Edit"),
				LOC(
					"$$$/LrGeniusAI/TaskAiEditPhotos/SuccessSummary=Applied edits to ^1 photo(s).\nSkipped: ^2",
					tostring(successCount),
					tostring(skippedCount)
				),
				"info"
			)
		end
		log:info(
			"AI Edit task completed. success="
				.. tostring(successCount)
				.. " skipped="
				.. tostring(skippedCount)
				.. " errors="
				.. tostring(errorCount)
		)
	end)
end)
