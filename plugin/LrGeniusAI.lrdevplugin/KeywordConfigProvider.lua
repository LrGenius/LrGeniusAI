KeywordConfigProvider = {}

local function loadKeywords()
	if prefs.keywordCategories == nil or type(prefs.keywordCategories) ~= "table" then
		log:trace("prefs.keywordCategories is not a table, using default categories.")
		prefs.keywordCategories = Defaults.defaultKeywordCategories
	end
	return prefs.keywordCategories
end

local function trim(s)
	if type(s) ~= "string" then
		return ""
	end
	return (string.gsub(s, "^%s*(.-)%s*$", "%1"))
end

local function keywordsToText(keywords)
	local sorted = {}
	for _, v in ipairs(keywords) do
		table.insert(sorted, v)
	end
	table.sort(sorted)
	return table.concat(sorted, "\n")
end

local function textToKeywords(text)
	local result = {}
	local seen = {}
	for line in string.gmatch(text .. "\n", "([^\n]*)\n") do
		local v = trim(line)
		if v ~= "" and not seen[v] then
			seen[v] = true
			table.insert(result, v)
		end
	end
	return result
end

local function countLines(text)
	local n = 0
	for line in string.gmatch(text .. "\n", "([^\n]*)\n") do
		if trim(line) ~= "" then
			n = n + 1
		end
	end
	return n
end

function KeywordConfigProvider.showKeywordCategoryDialog()
	LrFunctionContext.callWithContext("KeywordConfigProvider.showKeywordCategoryDialog", function(context)
		local f = LrView.osFactory()
		local bind = LrView.bind

		local keywords = loadKeywords()
		local props = LrBinding.makePropertyTable(context)
		props.categoriesText = keywordsToText(keywords)
		props.countLabel =
			LOC("$$$/LrGeniusAI/KeywordConfig/Count=^1 categories", tostring(countLines(props.categoriesText)))

		props:addObserver("categoriesText", function(p, _, value)
			p.countLabel = LOC("$$$/LrGeniusAI/KeywordConfig/Count=^1 categories", tostring(countLines(value or "")))
		end)

		local function resetToDefaults()
			local confirm = LrDialogs.confirm(
				LOC(
					"$$$/lrc-ai-assistant/ResponseStructure/ResetToDefaultKeywordStructure=Reset to default keyword structure?"
				)
			)
			if confirm == "ok" then
				props.categoriesText = keywordsToText(Defaults.defaultKeywordCategories)
			end
		end

		local dialogView = f:column({
			bind_to_object = props,
			spacing = f:control_spacing(),
			width = 360,

			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/KeywordConfig/Description=One category per line.\nEmpty lines are ignored when you save."
				),
			}),

			f:scrolled_view({
				width = 360,
				height = 440,
				horizontal_scroller = false,
				vertical_scroller = true,
				f:edit_field({
					value = bind("categoriesText"),
					immediate = true,
					width = 340,
					height_in_lines = 30,
					wraps = false,
				}),
			}),

			f:row({
				f:static_text({
					title = bind("countLabel"),
				}),
				f:spacer({ fill_horizontal = 1 }),
				f:push_button({
					title = LOC("$$$/LrGeniusAI/KeywordConfig/ResetButton=Reset to defaults"),
					action = resetToDefaults,
				}),
			}),
		})

		local result = LrDialogs.presentModalDialog({
			title = LOC("$$$/LrGeniusAI/KeywordConfig/Title=Configure Keyword Categories"),
			contents = dialogView,
		})

		if result == "ok" then
			prefs.keywordCategories = textToKeywords(props.categoriesText)
			log:trace("Saved keyword categories: " .. Util.dumpTable(prefs.keywordCategories))
		end
	end)
end

function KeywordConfigProvider.getKeywordCategories()
	return loadKeywords()
end
