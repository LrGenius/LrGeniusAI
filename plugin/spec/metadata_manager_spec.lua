-- "Append to existing values instead of replacing" used to concatenate
-- unconditionally. On a delta run the backend hands back the *stored* text —
-- the same words that were written to the catalog last time — so appending it
-- to itself doubled the field, and doubled it again on every further run.

require("MetadataManager")
_G.Util = require("Util")

local appendText = MetadataManager.appendText

describe("MetadataManager.appendText", function()
	it("appends when the field holds something different", function()
		assert.are.equal("Old text\n\nNew text", appendText("Old text", "New text"))
	end)

	it("does not append text that is already there", function()
		assert.are.equal("A description", appendText("A description", "A description"))
	end)

	it("ignores surrounding whitespace when comparing", function()
		assert.are.equal("  A description  ", appendText("  A description  ", "A description\n"))
	end)

	it("does not re-append after an earlier append", function()
		local once = appendText("Original", "Generated")
		assert.are.equal("Original\n\nGenerated", once)
		assert.are.equal(once, appendText(once, "Generated"))
	end)

	it("returns the incoming value when the field is empty", function()
		assert.are.equal("New text", appendText("", "New text"))
		assert.are.equal("New text", appendText(nil, "New text"))
	end)

	it("leaves an empty incoming value alone", function()
		assert.is_nil(appendText("Old text", nil))
		assert.are.equal("", appendText("Old text", ""))
	end)

	it("keeps the existing value when the incoming one is only whitespace", function()
		assert.are.equal("Old text", appendText("Old text", "   "))
	end)
end)

-- The taxonomy chain is where the shape mismatch between the backend and the
-- catalog gets resolved: the backend reports the species rank as a bare
-- epithet ("major") because that is how TreeOfLife-200M stores it, while a
-- keyword called "major" would be useless in a catalog.
describe("MetadataManager.speciesKeywordChain", function()
	local chain = MetadataManager.speciesKeywordChain

	local function names(result)
		local out = {}
		for _, entry in ipairs(result or {}) do
			table.insert(out, entry.name)
		end
		return out
	end

	it("uses the common name as the leaf and the binomial as its synonym", function()
		local result = chain({
			rank = "species",
			taxonomy = "Animalia>Chordata>Aves>Passeriformes>Paridae>Parus>major",
			scientific_name = "Parus major",
			common_name = "Great Tit",
		})
		assert.are.same({
			"Animalia",
			"Chordata",
			"Aves",
			"Passeriformes",
			"Paridae",
			"Parus",
			"Great Tit",
		}, names(result))
		assert.are.same({ "Parus major" }, result[7].synonyms)
	end)

	it("falls back to the binomial when the taxon has no common name", function()
		local result = chain({
			rank = "species",
			taxonomy = "Animalia>Arthropoda>Insecta>Lepidoptera>Lycaenidae>Orthomiella>sinensis",
			scientific_name = "Orthomiella sinensis",
			common_name = "",
		})
		-- Never the bare epithet: "sinensis" alone identifies nothing.
		assert.are.equal("Orthomiella sinensis", result[7].name)
		assert.are.same({}, result[7].synonyms)
	end)

	it("stops at the rank the backend was confident about", function()
		local result = chain({
			rank = "order",
			taxonomy = "Animalia>Arthropoda>Insecta>Coleoptera",
			scientific_name = "Coleoptera",
			common_name = "",
		})
		assert.are.same({ "Animalia", "Arthropoda", "Insecta", "Coleoptera" }, names(result))
	end)

	it("writes nothing when the photo holds no organism", function()
		assert.is_nil(chain({ rank = "none", taxonomy = "", scientific_name = "", common_name = "" }))
		assert.is_nil(chain({ rank = "", taxonomy = "Animalia" }))
		assert.is_nil(chain(nil))
		assert.is_nil(chain("not a table"))
	end)

	-- A rank present without a taxonomy string would otherwise build an empty
	-- chain and silently create a bare "Species" root with nothing under it.
	it("writes nothing when the taxonomy string is missing or empty", function()
		assert.is_nil(chain({ rank = "species", taxonomy = "" }))
		assert.is_nil(chain({ rank = "species" }))
		assert.is_nil(chain({ rank = "species", taxonomy = ">>>" }))
	end)

	it("tolerates whitespace around the separators", function()
		local result = chain({
			rank = "genus",
			taxonomy = "Plantae > Tracheophyta > Magnoliopsida > Fagales > Fagaceae > Quercus",
			scientific_name = "Quercus",
		})
		assert.are.same({ "Plantae", "Tracheophyta", "Magnoliopsida", "Fagales", "Fagaceae", "Quercus" }, names(result))
	end)
end)

-- The review dialog's species section. Its only job beyond formatting is
-- deciding when there is nothing to show: issue #327 was the species being
-- written no matter which button the user pressed, and the fix is a section
-- with a checkbox — which must not appear on the overwhelming majority of
-- photos, the ones with no organism in them.
describe("MetadataManager.speciesSummary", function()
	local summary = MetadataManager.speciesSummary

	it("puts the common name first and the binomial in parentheses", function()
		local result = summary({
			rank = "species",
			taxonomy = "Animalia > Chordata > Mammalia",
			scientific_name = "Oryctolagus cuniculus",
			common_name = "European Rabbit",
			confidence = 0.873,
		})
		assert.are.equal("European Rabbit (Oryctolagus cuniculus)", result.name)
		assert.are.equal("species, 87% confidence", result.rank)
		assert.are.equal("Animalia > Chordata > Mammalia", result.taxonomy)
	end)

	it("falls back to whichever name the taxon has", function()
		assert.are.equal("Leporidae", summary({ rank = "family", scientific_name = "Leporidae" }).name)
		assert.are.equal("Rabbits", summary({ rank = "family", common_name = "Rabbits" }).name)
	end)

	it("leaves out the confidence when the backend sent none", function()
		assert.are.equal("genus", summary({ rank = "genus", scientific_name = "Quercus" }).rank)
	end)

	it("shows nothing when the photo holds no organism", function()
		assert.is_nil(summary({ rank = "none", scientific_name = "", common_name = "" }))
		assert.is_nil(summary({ rank = "" }))
		assert.is_nil(summary(nil))
		assert.is_nil(summary("not a table"))
	end)

	-- A rank with neither name behind it would show an empty box with a
	-- checkbox and nothing to decide about.
	it("shows nothing when the identification has no name", function()
		assert.is_nil(summary({ rank = "species", scientific_name = "", common_name = "" }))
		assert.is_nil(summary({ rank = "species" }))
	end)
end)

-- The import sends each photo's catalog keywords to the backend as a
-- hierarchy, and "Retrieve Metadata" later writes that hierarchy back under
-- the current top-level keyword. The plugin's own container ("LrGeniusAI",
-- "ChatGPT", ...) therefore has to be left out, or it comes back nested inside
-- itself. The check used to compare the LrKeyword object against the list of
-- names, which is never equal, so nothing was ever left out.
describe("MetadataManager.keywordPath", function()
	require("Defaults")
	local keywordPath = MetadataManager.keywordPath

	local function kw(name, parent, synonyms)
		return { name = name, parent = parent, synonyms = synonyms }
	end

	local function access(topLevelNames)
		return {
			getName = function(k)
				return k.name
			end,
			getParent = function(k)
				return k.parent
			end,
			getSynonyms = function(k)
				return k.synonyms
			end,
			topLevelNames = topLevelNames,
		}
	end

	local known = { "LrGeniusAI", "Ollama", "LM Studio", "ChatGPT", "Google Gemini" }

	it("leaves out a known container at the root", function()
		local dog = kw("Dog", kw("Animals", kw("LrGeniusAI")))
		assert.are.same({ "Animals", "Dog" }, keywordPath(dog, access(known)))
	end)

	it("leaves out every known container name, not only the default one", function()
		local dog = kw("Dog", kw("Animals", kw("ChatGPT")))
		assert.are.same({ "Animals", "Dog" }, keywordPath(dog, access(known)))
	end)

	-- The prefs keep the name as it was typed in the dialog, which need not
	-- match the catalog keyword's spelling.
	it("matches a container name regardless of case and surrounding spaces", function()
		local dog = kw("Dog", kw("lrgeniusai"))
		assert.are.same({ "Dog" }, keywordPath(dog, access({ " LrGeniusAI " })))
	end)

	-- A container the prefs never heard of: prefs reset, or a catalog shared
	-- with another machine. applyMetadata stamps this synonym on every one.
	it("leaves out a root carrying the container synonym even when its name is unknown", function()
		local dog = kw("Dog", kw("My AI Keywords", nil, { "Something", Defaults.topLevelKeywordSynonym }))
		assert.are.same({ "Dog" }, keywordPath(dog, access(known)))
	end)

	it("keeps a root keyword of the user's own", function()
		local berlin = kw("Berlin", kw("Places"))
		assert.are.same({ "Places", "Berlin" }, keywordPath(berlin, access(known)))
	end)

	-- Containers only ever live at the top of the keyword list; one of the same
	-- name further down was made by the user.
	it("keeps a container name that is not at the root", function()
		local chatgpt = kw("ChatGPT", kw("Software"))
		assert.are.same({ "Software", "ChatGPT" }, keywordPath(chatgpt, access(known)))

		local nested = kw("Dog", kw("LrGeniusAI", kw("LrGeniusAI")))
		assert.are.same({ "LrGeniusAI", "Dog" }, keywordPath(nested, access(known)))
	end)

	it("returns an empty path for the container itself", function()
		assert.are.same({}, keywordPath(kw("LrGeniusAI"), access(known)))
	end)

	it("keeps everything when there is no container list or synonym accessor", function()
		local dog = kw("Dog", kw("LrGeniusAI"))
		local bare = access(nil)
		bare.getSynonyms = nil
		assert.are.same({ "LrGeniusAI", "Dog" }, keywordPath(dog, bare))
	end)
end)

-- The SDK wrapper around keywordPath: the prefs list reaches the helper and
-- the container is gone from what the import sends.
describe("MetadataManager.getPhotoKeywordHierarchy", function()
	require("Defaults")
	local savedKnown

	before_each(function()
		savedKnown = prefs.knownTopLevelKeywords
		prefs.knownTopLevelKeywords = { "LrGeniusAI" }
	end)

	after_each(function()
		prefs.knownTopLevelKeywords = savedKnown
	end)

	local function lrKeyword(name, parent)
		return {
			getName = function()
				return name
			end,
			getParent = function()
				return parent
			end,
			getSynonyms = function()
				return {}
			end,
		}
	end

	local function photoWith(keywords)
		return {
			getRawMetadata = function(_, key)
				assert.are.equal("keywords", key)
				return keywords
			end,
		}
	end

	it("files keywords under their categories without the container", function()
		local root = lrKeyword("LrGeniusAI")
		local dog = lrKeyword("Dog", lrKeyword("Animals", root))
		local berlin = lrKeyword("Berlin", lrKeyword("Places"))

		local hierarchy = MetadataManager.getPhotoKeywordHierarchy(photoWith({ root, dog, berlin }))

		assert.is_nil(hierarchy.LrGeniusAI)
		assert.is_nil(hierarchy[1])
		assert.are.equal("Dog", hierarchy.Animals[1])
		assert.are.equal("Berlin", hierarchy.Places[1])
	end)

	it("returns an empty table for a photo without keywords", function()
		assert.are.same({}, MetadataManager.getPhotoKeywordHierarchy(photoWith({})))
		assert.are.same({}, MetadataManager.getPhotoKeywordHierarchy(photoWith(nil)))
	end)
end)
