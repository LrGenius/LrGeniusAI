-- Regression tests for the prompt table: the state a prompt picker and its
-- text field share, and the two ways it used to go wrong.
--
-- A user cleared the "Instructions / Prompt" field and pressed Generate. The
-- dialogs wrote the field's value straight into `prompts[selectedName]`, so an
-- emptied field wrote nil, which in Lua *removes* the key. The prompt then no
-- longer existed: on macOS the picker snapped to another template and the edit
-- looked like it was never saved, and with no item matching the stored name
-- the bound value could go nil, at which point the same observer ran
-- `prompts[nil] = value` -- "table index is nil", raised from inside a
-- Lightroom binding callback.

require("Util")

describe("Util.resolvePromptName", function()
	local prompts = { Default = "d", Family = "f" }

	it("keeps a name the table still holds", function()
		assert.are.equal("Family", Util.resolvePromptName(prompts, "Family", "Default"))
	end)

	it("falls back to the default when the name is gone", function()
		assert.are.equal("Default", Util.resolvePromptName(prompts, "Deleted", "Default"))
	end)

	it("falls back to the default when there is no name at all", function()
		assert.are.equal("Default", Util.resolvePromptName(prompts, nil, "Default"))
	end)

	it("falls back to the first prompt in menu order when the default is gone too", function()
		-- Menu order, not hash order: the fallback is what the user sees at the
		-- top of the picker.
		assert.are.equal("Amber", Util.resolvePromptName({ Zulu = "z", Amber = "a" }, "Deleted", "Default"))
	end)

	it("returns nil only when there is nothing to select", function()
		assert.is_nil(Util.resolvePromptName({}, "Default", "Default"))
		assert.is_nil(Util.resolvePromptName(nil, "Default", "Default"))
	end)

	it("keeps an empty prompt selectable", function()
		-- The whole point: a prompt whose text the user cleared is still a
		-- prompt, and has to stay selected.
		assert.are.equal("Default", Util.resolvePromptName({ Default = "" }, "Default", "Default"))
	end)
end)

describe("Util.storePromptText", function()
	it("stores an emptied field as an empty prompt, not as a deleted one", function()
		local prompts = { Default = "text" }
		assert.is_true(Util.storePromptText(prompts, "Default", ""))
		assert.are.equal("", prompts.Default)
	end)

	it("treats a nil value the same way", function()
		local prompts = { Default = "text" }
		Util.storePromptText(prompts, "Default", nil)
		assert.are.equal("", prompts.Default)
	end)

	it("does not index the table with a nil name", function()
		local prompts = { Default = "text" }
		assert.is_false(Util.storePromptText(prompts, nil, "text"))
		assert.is_false(Util.storePromptText(prompts, "", "text"))
		assert.are.equal("text", prompts.Default)
	end)

	it("survives a missing prompt table", function()
		assert.is_false(Util.storePromptText(nil, "Default", "text"))
	end)

	it("stores ordinary text unchanged, whitespace included", function()
		local prompts = {}
		Util.storePromptText(prompts, "Mine", "  keep my indentation  ")
		assert.are.equal("  keep my indentation  ", prompts.Mine)
	end)
end)

describe("Util.promptForRequest", function()
	it("sends nothing for a blank prompt, so the backend uses its own default", function()
		assert.is_nil(Util.promptForRequest(""))
		assert.is_nil(Util.promptForRequest("   \n\t "))
		assert.is_nil(Util.promptForRequest(nil))
	end)

	it("trims what it does send", function()
		assert.are.equal("You are a bird expert.", Util.promptForRequest("  You are a bird expert.  "))
	end)

	it("ignores a non-string", function()
		assert.is_nil(Util.promptForRequest({}))
	end)
end)

describe("the prompt dialog's observer round trip", function()
	-- The two observers every prompt dialog installs, as the dialogs now write
	-- them. `prompt` is the picker's value, `selectedPrompt` the text field's.
	local function makeDialog(prompts, selected)
		local props = { prompts = prompts }
		props.prompt = Util.resolvePromptName(prompts, selected, "Default")
		props.selectedPrompt = (props.prompt ~= nil and prompts[props.prompt]) or ""

		function props.pick(name)
			props.prompt = name
			props.selectedPrompt = (name ~= nil and props.prompts[name]) or ""
		end
		function props.type(text)
			props.selectedPrompt = text
			Util.storePromptText(props.prompts, props.prompt, text)
		end
		return props
	end

	it("keeps an emptied prompt after switching away and back", function()
		local props = makeDialog({ Default = "d", Family = "f" }, "Default")
		props.type("")
		props.pick("Family")
		props.pick("Default")
		assert.are.equal("", props.selectedPrompt)
		assert.are.equal("", props.prompts.Default)
	end)

	it("does not lose the prompt from the picker when its text is cleared", function()
		local props = makeDialog({ Default = "d", Family = "f" }, "Default")
		props.type("")
		local titles = {}
		for _, item in ipairs(Util.promptMenuItems(props.prompts, "Default")) do
			titles[item.value] = true
		end
		assert.is_true(titles.Default)
	end)

	it("survives the picker losing its selection", function()
		-- What Lightroom leaves behind when the menu has no item matching its
		-- bound value. This used to raise "table index is nil" inside the
		-- binding callback.
		local props = makeDialog({ Default = "d" }, "Default")
		props.pick(nil)
		assert.has_no.errors(function()
			props.type("anything")
		end)
		assert.are.equal("d", props.prompts.Default)
	end)

	it("opens on a real prompt when the stored selection is gone", function()
		local props = makeDialog({ Family = "f" }, "Deleted")
		assert.are.equal("Family", props.prompt)
		assert.are.equal("f", props.selectedPrompt)
	end)
end)
