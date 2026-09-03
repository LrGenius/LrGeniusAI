-- The built-in prompt presets are *system* prompts and nothing else. The
-- backend writes the user prompt, and that is where the output fields, the
-- language, the keyword categories and the bilingual/alias encodings come from
-- (`prepare_user_prompt_split` in lrg-providers). A preset that specified any
-- of those would be contradicting the request it is attached to, which is a
-- failure nothing else in the pipeline would catch -- the run just gets worse.

require("Defaults")
require("Util")

describe("the built-in prompt presets", function()
	local presets = Defaults.builtinPrompts

	it("ships the default first, since it is the fallback everything resolves to", function()
		assert.are.equal(Defaults.defaultPromptName, presets[1].name)
	end)

	it("gives every preset a name and an instruction", function()
		for _, preset in ipairs(presets) do
			assert.is_string(preset.name)
			assert.is_true(Util.trim(preset.name) ~= "")
			assert.is_string(preset.instruction)
			assert.is_true(#Util.trim(preset.instruction) > 100, preset.name .. " has no real instruction")
		end
	end)

	it("uses each name only once", function()
		-- The presets are seeded into a name-keyed table, so a duplicate name
		-- would silently drop a preset instead of failing.
		local seen = {}
		for _, preset in ipairs(presets) do
			assert.is_nil(seen[preset.name], "duplicate preset name: " .. preset.name)
			seen[preset.name] = true
		end
	end)

	it("round-trips through builtinPromptTable, which is what Reset to defaults restores", function()
		local table_ = Defaults.builtinPromptTable()
		for _, preset in ipairs(presets) do
			assert.are.equal(preset.instruction, table_[preset.name])
		end
		assert.are.equal(#presets, #Util.promptMenuItems(table_, Defaults.defaultPromptName))
	end)

	it("never dictates the output format the backend's schema already fixes", function()
		-- The answer is schema-constrained; a preset asking for JSON, markdown
		-- or a field list can only conflict with it.
		local forbidden = { "json", "markdown", "yaml", "one per line", "comma%-separated" }
		for _, preset in ipairs(presets) do
			local lower = string.lower(preset.instruction)
			for _, term in ipairs(forbidden) do
				assert.is_nil(string.find(lower, term), preset.name .. " dictates output format: " .. term)
			end
		end
	end)

	it("never names an output language, which the request already sets", function()
		-- `prepare_user_prompt_split` appends "All results should be generated
		-- in <language>". A preset naming a language would fight that for every
		-- user whose catalog is in another one.
		local languages = { "in english", "in german", "auf deutsch", "in french", "in spanish" }
		for _, preset in ipairs(presets) do
			local lower = string.lower(preset.instruction)
			for _, term in ipairs(languages) do
				assert.is_nil(string.find(lower, term, 1, true), preset.name .. " names a language: " .. term)
			end
		end
	end)

	it("tells each genre what it must not invent", function()
		-- The failure mode that costs a photographer real time is a confident
		-- invention, and it differs per genre. Every preset added since has to
		-- draw that line somewhere.
		--
		-- `Default` is exempt on purpose: it is the voice this plugin has
		-- always had, and every catalog indexed with it was indexed with these
		-- exact words. Tightening it would silently change output people
		-- already have. A stricter voice is a preset away instead.
		for _, preset in ipairs(presets) do
			if preset.name ~= Defaults.defaultPromptName then
				local lower = string.lower(preset.instruction)
				assert.is_truthy(
					string.find(lower, "never invent") or string.find(lower, "never guess"),
					preset.name .. " does not say what it must not invent"
				)
			end
		end
	end)

	it("keeps a preset short enough to leave the context window for the photo", function()
		-- The system prompt sits in the run-constant KV prefix ahead of every
		-- photo's own context; the local engine's default share is 8192 tokens
		-- for prompt plus answer together.
		for _, preset in ipairs(presets) do
			assert.is_true(#preset.instruction < 2200, preset.name .. " is too long for a system prompt")
		end
	end)
end)
