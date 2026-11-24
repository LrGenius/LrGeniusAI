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

describe("MetadataManager.vocabularyRootExclusion", function()
	require("Defaults")
	local exclusion = MetadataManager.vocabularyRootExclusion

	local function kw(name, synonyms)
		return { name = name, synonyms = synonyms }
	end

	local access = {
		getName = function(k)
			return k.name
		end,
		getSynonyms = function(k)
			return k.synonyms
		end,
		topLevelNames = { "LrGeniusAI", "Motorsport" },
	}

	it("leaves out the species branch with everything under it", function()
		assert.are.equal("subtree", exclusion(kw(Defaults.defaultSpeciesKeyword), access))
		assert.are.equal("subtree", exclusion(kw(" species "), access))
	end)

	it("leaves out a plugin container but not the keywords under it", function()
		assert.are.equal("self", exclusion(kw("Motorsport"), access))
		assert.are.equal("self", exclusion(kw("My AI", { Defaults.topLevelKeywordSynonym }), access))
	end)

	it("keeps a root keyword of the user's own", function()
		assert.is_nil(exclusion(kw("Places"), access))
		assert.is_nil(exclusion(kw("Speciesism"), access))
	end)
end)

-- The catalog walk behind the vocabulary, against a fake catalog: what the
-- model is told to prefer is the user's terms and the generated ones, not the
-- taxonomy the species model wrote or the container they are filed under.
describe("MetadataManager.collectCatalogKeywordNames", function()
	require("Defaults")
	local saved = {}

	local function lrKeyword(name, photos, children, synonyms)
		local list = {}
		for i = 1, photos do
			list[i] = {}
		end
		return {
			getName = function()
				return name
			end,
			getPhotos = function()
				return list
			end,
			getChildren = function()
				return children or {}
			end,
			getSynonyms = function()
				return synonyms or {}
			end,
		}
	end

	local function catalogWith(path, roots)
		return {
			getPath = function()
				return path
			end,
			getKeywords = function()
				return roots
			end,
		}
	end

	before_each(function()
		saved.activeCatalog = _G.LrApplication.activeCatalog
		saved.currentTime = _G.LrDate.currentTime
		saved.known = prefs.knownTopLevelKeywords
		_G.LrDate.currentTime = function()
			return 0
		end
		prefs.knownTopLevelKeywords = { "LrGeniusAI" }
	end)

	after_each(function()
		_G.LrApplication.activeCatalog = saved.activeCatalog
		_G.LrDate.currentTime = saved.currentTime
		prefs.knownTopLevelKeywords = saved.known
	end)

	it("sends the user's and the generated keywords, not the taxonomy or the container", function()
		local species = lrKeyword(Defaults.defaultSpeciesKeyword, 0, {
			lrKeyword("Animalia", 900, { lrKeyword("Insecta", 800, { lrKeyword("Coleoptera", 700) }) }),
		})
		local container = lrKeyword("LrGeniusAI", 50, { lrKeyword("Motocross", 40), lrKeyword("Dirtbike", 30) })
		local places = lrKeyword("Places", 0, { lrKeyword("Eichenried", 20) })
		_G.LrApplication.activeCatalog = function()
			return catalogWith("/vocabulary/one.lrcat", { species, container, places })
		end

		local names = MetadataManager.collectCatalogKeywordNames(10)

		assert.are.same({ "Dirtbike", "Eichenried", "Motocross" }, names)
	end)

	it("keeps a container name further down the tree, which the user made", function()
		local nested = lrKeyword("Software", 0, { lrKeyword("LrGeniusAI", 5), lrKeyword("Species", 4) })
		_G.LrApplication.activeCatalog = function()
			return catalogWith("/vocabulary/two.lrcat", { nested })
		end

		assert.are.same({ "LrGeniusAI", "Species" }, MetadataManager.collectCatalogKeywordNames(10))
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

-- "Use existing catalog structure" sends the catalog's keyword tree as the
-- categories the model files keywords under. The plugin's container used to
-- be one of them, so the model answered with a "LrGeniusAI" category, which
-- applyMetadata then created under the container itself (#490).
describe("MetadataManager.catalogKeywordHierarchy", function()
	require("Defaults")
	local hierarchyOf = MetadataManager.catalogKeywordHierarchy

	local function kw(name, children, synonyms)
		return { name = name, children = children or {}, synonyms = synonyms }
	end

	local access = {
		getName = function(k)
			return k.name
		end,
		getChildren = function(k)
			return k.children
		end,
		getSynonyms = function(k)
			return k.synonyms
		end,
		topLevelNames = { "LrGeniusAI", "ChatGPT" },
	}

	it("files the container's categories at the root, not under a category named like it", function()
		local container = kw("LrGeniusAI", {
			kw("Activities", { kw("Hiking") }),
			kw("Animals", { kw("Dog") }),
			kw("Sunset"),
		})

		assert.are.same({ Activities = {}, Animals = {} }, hierarchyOf({ container }, access))
	end)

	it("merges a container's category into the user's root category of the same name", function()
		local roots = {
			kw("Places", { kw("Germany", { kw("Berlin") }) }),
			kw("LrGeniusAI", { kw("Places", { kw("France", { kw("Paris") }) }) }),
		}

		assert.are.same({ Places = { Germany = {}, France = {} } }, hierarchyOf(roots, access))
	end)

	it("recognises a container by its synonym, whatever it is called", function()
		local container = kw("My AI", { kw("Moods", { kw("Calm") }) }, { Defaults.topLevelKeywordSynonym })

		assert.are.same({ Moods = {} }, hierarchyOf({ container }, access))
	end)

	it("lifts the doubled container an earlier run left inside the container", function()
		local doubled = kw("LrGeniusAI", {
			kw("LrGeniusAI", { kw("Activities", { kw("Hiking") }) }),
			kw("Activities", { kw("Cycling") }),
		})

		assert.are.same({ Activities = {} }, hierarchyOf({ doubled }, access))
	end)

	it("leaves out the species taxonomy", function()
		local species = kw(Defaults.defaultSpeciesKeyword, { kw("Animalia", { kw("Chordata", { kw("Aves") }) }) })

		assert.are.same({}, hierarchyOf({ species }, access))
	end)

	it("keeps a container name below the root, which the user made", function()
		local software = kw("Software", { kw("LrGeniusAI", { kw("Plugins") }), kw("Species", { kw("X") }) })

		assert.are.same({ Software = { LrGeniusAI = {}, Species = {} } }, hierarchyOf({ software }, access))
	end)

	it("prunes a subtree whose children cannot be read", function()
		local broken = kw("Broken")
		broken.children = nil

		assert.are.same({ Places = {} }, hierarchyOf({ broken, kw("Places", { kw("Berlin") }) }, access))
	end)
end)

-- Applying a response that still carries the container as a category: from a
-- catalog structure sent before the fix, or a hierarchy the backend stored
-- then and "Retrieve Metadata" writes back.
describe("MetadataManager.addKeywordRecursively", function()
	local saved = {}

	local function newKeyword(name, parent)
		local k = { name = name, parent = parent, children = {} }
		k.getName = function()
			return k.name
		end
		k.getParent = function()
			return k.parent
		end
		k.getChildren = function()
			return k.children
		end
		k.getSynonyms = function()
			return {}
		end
		k.setAttributes = function() end
		return k
	end

	local function fakeCatalog()
		local catalog = { roots = {} }
		catalog.getKeywords = function()
			return catalog.roots
		end
		catalog.createKeyword = function(_, name, _, _, parent, _)
			local siblings = parent and parent.children or catalog.roots
			for _, existing in ipairs(siblings) do
				if existing.name == name then
					return existing
				end
			end
			local created = newKeyword(name, parent)
			table.insert(siblings, created)
			return created
		end
		return catalog
	end

	local function fakePhoto()
		local photo = { keywords = {} }
		photo.getRawMetadata = function()
			return photo.keywords
		end
		photo.addKeyword = function(_, k)
			table.insert(photo.keywords, k)
		end
		return photo
	end

	local function path(k)
		local names = {}
		while k do
			table.insert(names, 1, k.name)
			k = k.parent
		end
		return table.concat(names, " > ")
	end

	local function pathsOn(photo)
		local paths = {}
		for _, k in ipairs(photo.keywords) do
			table.insert(paths, path(k))
		end
		table.sort(paths)
		return paths
	end

	before_each(function()
		saved.hierarchy = prefs.useKeywordHierarchy
		saved.topLevel = prefs.topLevelKeyword
		prefs.useKeywordHierarchy = true
		prefs.topLevelKeyword = "LrGeniusAI"
	end)

	after_each(function()
		prefs.useKeywordHierarchy = saved.hierarchy
		prefs.topLevelKeyword = saved.topLevel
	end)

	it("files a category named like the top-level keyword under the top-level keyword itself", function()
		local catalog = fakeCatalog()
		local photo = fakePhoto()
		local top = catalog:createKeyword("LrGeniusAI", {}, false, nil, true)
		local warnings = {}

		MetadataManager.addKeywordRecursively(photo, catalog, {
			LrGeniusAI = { Activities = { "Hiking" } },
			Animals = { "Dog" },
		}, top, nil, "LrGeniusAI", {}, warnings)

		assert.are.same({ "LrGeniusAI > Activities > Hiking", "LrGeniusAI > Animals > Dog" }, pathsOn(photo))
		for _, child in ipairs(top.children) do
			assert.are_not.equal("LrGeniusAI", child.name)
		end
		assert.are.same({}, warnings)
	end)

	it("matches the top-level keyword without regard to case", function()
		local catalog = fakeCatalog()
		local photo = fakePhoto()
		local top = catalog:createKeyword("LrGeniusAI", {}, false, nil, true)

		MetadataManager.addKeywordRecursively(
			photo,
			catalog,
			{ lrgeniusai = { Moods = { "Calm" } } },
			top,
			nil,
			"LrGeniusAI",
			{},
			{}
		)

		assert.are.same({ "LrGeniusAI > Moods > Calm" }, pathsOn(photo))
	end)

	it("puts the categories at the root when there is no top-level keyword", function()
		local catalog = fakeCatalog()
		local photo = fakePhoto()

		MetadataManager.addKeywordRecursively(
			photo,
			catalog,
			{ LrGeniusAI = { Activities = { "Hiking" } } },
			nil,
			nil,
			nil,
			{},
			{}
		)

		assert.are.same({ "Activities > Hiking" }, pathsOn(photo))
	end)
end)
