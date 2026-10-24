-- Unit tests for SmartCollectionTemplates: the search descriptors the
-- "Create Smart Collection Templates" task hands to createSmartCollection,
-- and what a re-run does with collections that already exist.
--
-- Run from the repo root with:  busted

require("SmartCollectionTemplates")

local T = SmartCollectionTemplates
local PLUGIN = "LrGeniusAI"
local GENRES = {
	{ id = "landscape", label = "Landscape" },
	{ id = "wildlife", label = "Wildlife & animals" },
	{ id = "event", label = "Event & wedding" },
}

local function byId(templates)
	local map = {}
	for _, template in ipairs(templates) do
		map[template.id] = template
	end
	return map
end

describe("SmartCollectionTemplates.build", function()
	it("addresses plug-in fields as sdktext:<plug-in id>.<field id>", function()
		assert.are.equal("sdktext:LrGeniusAI.qualityFlags", T.fieldCriteria(PLUGIN, "qualityFlags"))
	end)

	it("reads only fields the metadata provider declares searchable", function()
		local provider = dofile("plugin/LrGeniusAI.lrdevplugin/MetadataProvider.lua")
		local searchable = {}
		for _, field in ipairs(provider.metadataFieldsForPhotos) do
			if field.searchable then
				searchable[field.id] = true
			end
		end
		local function walk(desc)
			if desc.criteria then
				local field = desc.criteria:match("^sdktext:" .. PLUGIN .. "%.(.+)$")
				if field then
					assert.is_true(searchable[field] == true, field .. " is not a searchable field")
				end
			end
			for _, child in ipairs(desc) do
				walk(child)
			end
		end
		for _, template in ipairs(T.build(PLUGIN, GENRES)) do
			walk(template.searchDesc)
		end
	end)

	it("gives every template a unique id and a unique name within its group", function()
		local ids, names = {}, {}
		for _, template in ipairs(T.build(PLUGIN, GENRES)) do
			assert.is_nil(ids[template.id], "duplicate id " .. template.id)
			ids[template.id] = true
			local key = template.group .. "/" .. template.name
			assert.is_nil(names[key], "duplicate name " .. key)
			names[key] = true
			assert.truthy(template.requires and template.requires ~= "")
		end
	end)

	it("spells aesthetic score 7+ as a union of leading digits", function()
		local best = byId(T.build(PLUGIN, nil))["quality.best"].searchDesc
		assert.are.equal("union", best.combine)
		local digits = {}
		for _, rule in ipairs(best) do
			assert.are.equal("sdktext:LrGeniusAI.aestheticScore", rule.criteria)
			assert.are.equal("beginsWith", rule.operation)
			digits[#digits + 1] = rule.value
		end
		assert.are.same({ "7", "8", "9" }, digits)
	end)

	it("matches the leading digit of every score the plug-in writes from 7.0 up, and none below", function()
		-- The field holds Util.aestheticFieldValues's "%.1f" text.
		local function matches(text)
			for _, rule in ipairs(byId(T.build(PLUGIN, nil))["quality.best"].searchDesc) do
				if text:sub(1, #rule.value) == rule.value then
					return true
				end
			end
			return false
		end
		for tenth = 10, 90 do
			local text = string.format("%.1f", tenth / 10)
			assert.are.equal(tenth >= 70, matches(text), text)
		end
	end)

	it("finds picks without keywords as an intersection with an empty keyword field", function()
		local desc = byId(T.build(PLUGIN, nil))["culling.picks_no_keywords"].searchDesc
		assert.are.equal("intersect", desc.combine)
		assert.are.same({ criteria = "sdktext:LrGeniusAI.cullDecision", operation = "any", value = "pick" }, desc[1])
		assert.are.same({ criteria = "keywords", operation = "empty", value = "" }, desc[2])
	end)

	it("uses cull decision words that are not part of one another", function()
		-- "contains" stands in for an exact match only while that holds.
		local decisions = { "pick", "alternate", "reject_candidate" }
		for _, word in ipairs({ "pick", "reject" }) do
			local hits = 0
			for _, decision in ipairs(decisions) do
				if decision:find(word, 1, true) then
					hits = hits + 1
				end
			end
			assert.are.equal(1, hits, word)
		end
	end)

	it("matches quality flags by a word shared by exactly the labels it means", function()
		local labels = { "blurred", "motion blur", "highlights clipped", "shadows clipped" }
		local function count(word)
			local hits = 0
			for _, label in ipairs(labels) do
				if label:find(word, 1, true) then
					hits = hits + 1
				end
			end
			return hits
		end
		local templates = byId(T.build(PLUGIN, nil))
		assert.are.equal(2, count(templates["quality.blurred"].searchDesc.value))
		assert.are.equal(2, count(templates["quality.clipped"].searchDesc.value))
		assert.are.equal("notEmpty", templates["quality.flagged"].searchDesc.operation)
	end)

	it("has one collection per taxonomic class, matched in the taxonomy path", function()
		local templates = byId(T.build(PLUGIN, nil))
		assert.are.equal("notEmpty", templates["species.identified"].searchDesc.operation)
		local birds = templates["species.birds"].searchDesc
		assert.are.same({ criteria = "sdktext:LrGeniusAI.speciesTaxonomy", operation = "any", value = "Aves" }, birds)
	end)

	it("adds one genre collection per backend genre, matching all words of the label", function()
		local templates = byId(T.build(PLUGIN, GENRES))
		local wildlife = templates["genre.wildlife"]
		assert.are.equal("Wildlife & animals", wildlife.name)
		assert.are.same(
			{ criteria = "sdktext:LrGeniusAI.genre", operation = "all", value = "Wildlife & animals" },
			wildlife.searchDesc
		)
	end)

	it("leaves the genre group out without a genre list, and skips malformed entries", function()
		for _, genres in ipairs({ nil, {}, { { id = "x" }, { label = "Y" }, "bad" } }) do
			for _, template in ipairs(T.build(PLUGIN, genres)) do
				assert.are_not.equal("genre", template.group)
			end
		end
	end)

	it("only uses groups the dialog knows", function()
		local known = {}
		for _, group in ipairs(T.GROUPS) do
			known[group.id] = true
		end
		for _, template in ipairs(T.build(PLUGIN, GENRES)) do
			assert.is_true(known[template.group], template.group)
		end
	end)
end)

describe("SmartCollectionTemplates.plan", function()
	local templates = T.build(PLUGIN, GENRES)
	local all = {}
	for _, template in ipairs(templates) do
		all[template.id] = true
	end

	it("creates everything selected on a first run", function()
		local plan = T.plan(templates, all, {})
		assert.are.equal(#templates, #plan.create)
		assert.are.equal(0, #plan.kept)
	end)

	it("only creates what was ticked", function()
		local plan = T.plan(templates, { ["quality.best"] = true }, {})
		assert.are.equal(1, #plan.create)
		assert.are.equal("quality.best", plan.create[1].id)
	end)

	it("keeps a smart collection that already has the name, and reports an ordinary one", function()
		local plan = T.plan(templates, all, {
			quality = { ["Best photos (aesthetic score 7+)"] = "smart", ["Has quality flags"] = "regular" },
		})
		assert.are.equal(1, #plan.kept)
		assert.are.equal("quality.best", plan.kept[1].id)
		assert.are.equal(1, #plan.conflicts)
		assert.are.equal("quality.flagged", plan.conflicts[1].id)
		assert.are.equal(#templates - 2, #plan.create)
	end)

	it("judges a name only within its own group's set", function()
		local plan = T.plan(templates, all, { culling = { ["Best photos (aesthetic score 7+)"] = "smart" } })
		assert.are.equal(0, #plan.kept)
	end)
end)

describe("SmartCollectionTemplates.summary", function()
	it("reports created, kept, conflicts and failures, each with what to do", function()
		local text = T.summary({ created = 3, kept = 2, conflicts = { "Has quality flags" }, failed = { "Birds" } })
		assert.truthy(text:find("Created 3 smart collection", 1, true))
		assert.truthy(text:find("2 already existed", 1, true))
		assert.truthy(text:find("Has quality flags", 1, true))
		assert.truthy(text:find("refused to create: Birds", 1, true))
		assert.truthy(text:find("Analyze & Index Photos", 1, true))
	end)

	it("says nothing about empty collections when none were created", function()
		local text = T.summary({ created = 0, kept = 4 })
		assert.is_nil(text:find("stays empty", 1, true))
	end)
end)
