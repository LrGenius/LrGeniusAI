-- SmartCollectionTemplates.lua
-- Ready-made smart collections over LrGeniusAI's own metadata fields, created
-- by TaskCreateSmartCollections.lua under one "LrGeniusAI" collection set.
--
-- Pure (no catalog access), so the templates and the re-run rules are tested
-- headless in plugin/spec/smart_collection_templates_spec.lua.
--
-- How the search descriptors are built
-- ------------------------------------
-- `catalog:createSmartCollection(name, searchDesc, parent, canReturnPrior)`
-- takes the same descriptor as `catalog:findPhotos`. A plug-in field is
-- addressed as `sdktext:<plug-in id>.<field id>`; the field must be declared
-- `searchable` (MetadataProvider.lua) and is matched as text, never as a
-- number. Text operations: "any" (contains any of the words), "all" (contains
-- all of them), "words", "noneOf", "beginsWith", "endsWith", "empty",
-- "notEmpty", "==" and "!=" (the last two only "for fields that support exact
-- matching", which the SDK does not list).
--
-- Every numeric AI field of this plug-in is stored as text ("7.4"), so "score
-- 7 or better" is spelled as "starts with 7, 8 or 9" — exact for a one-decimal
-- score on the model's 1-9 scale.
--
-- None of the criteria below could be checked against a running Lightroom
-- when they were written. They are all in CRITERIA, one place, so that a
-- correction after the live test is a one-line change; the PR that added them
-- lists what to check for each.

SmartCollectionTemplates = {}

--- The collection set every template is created in, at the top level.
SmartCollectionTemplates.ROOT_SET = "LrGeniusAI"

--- Child sets, in the order the dialog shows them.
SmartCollectionTemplates.GROUPS = {
	{ id = "quality", title = "Quality" },
	{ id = "culling", title = "Culling" },
	{ id = "species", title = "Species" },
	{ id = "genre", title = "Genre" },
}

--- Taxonomic classes offered as their own collections: the ones a nature
--- photographer's library is mostly made of. Matched against the
--- `speciesTaxonomy` field, which holds the full path the backend wrote
--- ("Animalia>Chordata>Aves>Passeriformes>Paridae>Parus>Parus major").
SmartCollectionTemplates.TAXA = {
	{ id = "birds", name = "Birds", taxon = "Aves" },
	{ id = "mammals", name = "Mammals", taxon = "Mammalia" },
	{ id = "insects", name = "Insects", taxon = "Insecta" },
	{ id = "reptiles", name = "Reptiles", taxon = "Reptilia" },
	{ id = "amphibians", name = "Amphibians", taxon = "Amphibia" },
	{ id = "plants", name = "Plants", taxon = "Plantae" },
	{ id = "fungi", name = "Fungi", taxon = "Fungi" },
}

--- `sdktext:<plug-in id>.<field id>`, the criteria name of a searchable
--- plug-in field.
function SmartCollectionTemplates.fieldCriteria(pluginId, fieldId)
	return "sdktext:" .. tostring(pluginId) .. "." .. tostring(fieldId)
end

-- Every criterion the templates use. Each entry builds one search descriptor
-- from the plug-in id (and an argument where it takes one).
local CRITERIA = {}

-- Aesthetic score of 7.0 or better. The field holds "%.1f" text on a 1-9
-- scale (Util.aestheticFieldValues), so the first digit decides.
function CRITERIA.aestheticAtLeast7(pluginId)
	local field = SmartCollectionTemplates.fieldCriteria(pluginId, "aestheticScore")
	return {
		combine = "union",
		{ criteria = field, operation = "beginsWith", value = "7" },
		{ criteria = field, operation = "beginsWith", value = "8" },
		{ criteria = field, operation = "beginsWith", value = "9" },
	}
end

-- Any quality flag at all. The field is never written empty on a photo that
-- had none (MetadataManager.applyQualityFlags), and a cleared one holds "".
function CRITERIA.hasQualityFlags(pluginId)
	return {
		criteria = SmartCollectionTemplates.fieldCriteria(pluginId, "qualityFlags"),
		operation = "notEmpty",
		value = "",
	}
end

-- One word of the flag labels: "blur" is in "blurred" and in "motion blur",
-- "clipped" in both clipping flags. Lightroom's "contains" matches inside
-- words.
function CRITERIA.qualityFlagContains(pluginId, word)
	return {
		criteria = SmartCollectionTemplates.fieldCriteria(pluginId, "qualityFlags"),
		operation = "any",
		value = word,
	}
end

-- The cull's decision for the photo. The values are "pick", "alternate" and
-- "reject_candidate" (Util.cullDecisionOf); none contains another, so
-- "contains" is exact here without relying on "==".
function CRITERIA.cullDecision(pluginId, decision)
	return {
		criteria = SmartCollectionTemplates.fieldCriteria(pluginId, "cullDecision"),
		operation = "any",
		value = decision,
	}
end

-- Picks that nobody has keyworded yet: the photos worth an Analyze & Index
-- pass with AI metadata, or a keywording session.
function CRITERIA.picksWithoutKeywords(pluginId)
	return {
		combine = "intersect",
		CRITERIA.cullDecision(pluginId, "pick"),
		{ criteria = "keywords", operation = "empty", value = "" },
	}
end

-- A species was named. `speciesScientificName` is written "" when the
-- backend found no organism (MetadataManager.applySpecies), so "not empty"
-- is exactly "identified at some rank".
function CRITERIA.speciesIdentified(pluginId)
	return {
		criteria = SmartCollectionTemplates.fieldCriteria(pluginId, "speciesScientificName"),
		operation = "notEmpty",
		value = "",
	}
end

-- A taxon anywhere on the identification's path.
function CRITERIA.taxon(pluginId, taxon)
	return {
		criteria = SmartCollectionTemplates.fieldCriteria(pluginId, "speciesTaxonomy"),
		operation = "any",
		value = taxon,
	}
end

-- The genre's label, as written into the field ("Wildlife & animals").
-- "all" rather than "any": a label of several words must not match every
-- genre that shares one of them (the "&" of "Event & wedding" and "Product &
-- still life").
function CRITERIA.genre(pluginId, label)
	return {
		criteria = SmartCollectionTemplates.fieldCriteria(pluginId, "genre"),
		operation = "all",
		value = label,
	}
end

SmartCollectionTemplates.CRITERIA = CRITERIA

---
-- Every template, in dialog order.
--
-- @param pluginId string `_PLUGIN.id`.
-- @param genres table|nil The backend's genre list (`{ { id, label }, ... }`
--   from /v1/search/genres). nil or empty leaves the genre group out — the
--   labels are the backend's, and a guessed list could name genres it never
--   writes.
-- @return table List of `{ id, group, name, searchDesc, requires }`, where
--   `requires` names the task that fills the fields the collection reads.
--
function SmartCollectionTemplates.build(pluginId, genres)
	local templates = {}
	local function add(group, id, name, searchDesc, requires)
		templates[#templates + 1] = {
			id = group .. "." .. id,
			group = group,
			name = name,
			searchDesc = searchDesc,
			requires = requires,
		}
	end

	local smartSearch = "Analyze & Index Photos with smart photo search"
	local anyIndexing = "Analyze & Index Photos or Cull Similar Photos"
	local cull = "Cull Similar Photos"
	local species = "Analyze & Index Photos with species identification"

	add("quality", "best", "Best photos (aesthetic score 7+)", CRITERIA.aestheticAtLeast7(pluginId), smartSearch)
	add("quality", "flagged", "Has quality flags", CRITERIA.hasQualityFlags(pluginId), anyIndexing)
	add("quality", "blurred", "Blurred or motion blur", CRITERIA.qualityFlagContains(pluginId, "blur"), anyIndexing)
	add(
		"quality",
		"clipped",
		"Clipped highlights or shadows",
		CRITERIA.qualityFlagContains(pluginId, "clipped"),
		anyIndexing
	)

	add("culling", "picks", "Culling picks", CRITERIA.cullDecision(pluginId, "pick"), cull)
	add("culling", "picks_no_keywords", "Culling picks without keywords", CRITERIA.picksWithoutKeywords(pluginId), cull)
	add("culling", "rejects", "Culling reject candidates", CRITERIA.cullDecision(pluginId, "reject"), cull)

	add("species", "identified", "Species identified", CRITERIA.speciesIdentified(pluginId), species)
	for _, taxon in ipairs(SmartCollectionTemplates.TAXA) do
		add("species", taxon.id, taxon.name, CRITERIA.taxon(pluginId, taxon.taxon), species)
	end

	for _, genre in ipairs(type(genres) == "table" and genres or {}) do
		local label = type(genre) == "table" and genre.label or nil
		local id = type(genre) == "table" and genre.id or nil
		if type(label) == "string" and label ~= "" and type(id) == "string" and id ~= "" then
			add("genre", id, label, CRITERIA.genre(pluginId, label), smartSearch)
		end
	end
	return templates
end

---
-- What a run has to do, given what the catalog already holds.
--
-- A template whose name is already taken in its group's set is left alone:
-- the user may have edited it, and `createSmartCollection` with
-- `canReturnPrior` hands back the existing collection unchanged anyway. A
-- name taken by an ordinary (non-smart) collection is a conflict, reported
-- rather than overwritten.
--
-- @param templates table From `build`.
-- @param selected table Set of template ids the user ticked (`id -> true`).
-- @param existing table `group -> { name -> "smart" | "regular" }` for the
--   collections already in each group's set (absent group: nothing there yet).
-- @return table `{ create = {templates}, kept = {templates}, conflicts = {templates} }`.
--
function SmartCollectionTemplates.plan(templates, selected, existing)
	local result = { create = {}, kept = {}, conflicts = {} }
	selected = selected or {}
	existing = existing or {}
	for _, template in ipairs(templates or {}) do
		if selected[template.id] then
			local inGroup = existing[template.group] or {}
			local kind = inGroup[template.name]
			if kind == "smart" then
				table.insert(result.kept, template)
			elseif kind ~= nil then
				table.insert(result.conflicts, template)
			else
				table.insert(result.create, template)
			end
		end
	end
	return result
end

---
-- The completion message for a run.
--
-- @param outcome table `{ created = n, kept = n, conflicts = {names}, failed = {names} }`.
-- @return string
--
function SmartCollectionTemplates.summary(outcome)
	outcome = outcome or {}
	local lines = {}
	local created = tonumber(outcome.created) or 0
	local kept = tonumber(outcome.kept) or 0
	table.insert(
		lines,
		string.format(
			'Created %d smart collection(s) in the "%s" collection set.',
			created,
			SmartCollectionTemplates.ROOT_SET
		)
	)
	if kept > 0 then
		table.insert(
			lines,
			string.format(
				"%d already existed and were left as they are. Delete one in the Collections panel and run this again to recreate it from the template.",
				kept
			)
		)
	end
	local conflicts = outcome.conflicts or {}
	if #conflicts > 0 then
		table.insert(
			lines,
			string.format(
				"Not created because an ordinary collection already has the name: %s. Rename or move it, then run this again.",
				table.concat(conflicts, ", ")
			)
		)
	end
	local failed = outcome.failed or {}
	if #failed > 0 then
		table.insert(
			lines,
			string.format(
				"Lightroom refused to create: %s. Please report this with the plug-in log, so the template can be fixed.",
				table.concat(failed, ", ")
			)
		)
	end
	if created > 0 then
		table.insert(
			lines,
			"A collection stays empty until the task it reads from has run on your photos: Analyze & Index Photos for the score, genre, species and quality flags, Cull Similar Photos for the culling decisions."
		)
	end
	return table.concat(lines, "\n\n")
end
