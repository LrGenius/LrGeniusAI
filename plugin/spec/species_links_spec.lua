-- The links written into `speciesInatUrl` / `speciesWikipediaUrl` are the only
-- part of the species feature the user can click, so the two things that must
-- hold are: a name always produces *some* working URL (the backend may be
-- down), and a name is never pasted into a URL unencoded.

require("SpeciesLinks")

describe("SpeciesLinks.encodeComponent", function()
	it("encodes spaces, which every binomial has", function()
		assert.are.equal("Panthera%20leo", SpeciesLinks.encodeComponent("Panthera leo"))
	end)

	it("encodes characters that would break out of the query parameter", function()
		assert.are.equal("a%26b%3Dc", SpeciesLinks.encodeComponent("a&b=c"))
	end)

	it("leaves unreserved characters alone", function()
		assert.are.equal("Abc-123_x.y~z", SpeciesLinks.encodeComponent("Abc-123_x.y~z"))
	end)

	it("survives a non-string", function()
		assert.are.equal("", SpeciesLinks.encodeComponent(nil))
	end)
end)

describe("SpeciesLinks.wikiLang", function()
	it("keeps a plain two-letter code", function()
		assert.are.equal("de", SpeciesLinks.wikiLang("de"))
	end)

	it("strips a region tag, which Wikipedia has no host for", function()
		assert.are.equal("zh", SpeciesLinks.wikiLang("zh-Hans"))
		assert.are.equal("pt", SpeciesLinks.wikiLang("pt_BR"))
	end)

	it("falls back to English rather than an invented host", function()
		assert.are.equal("en", SpeciesLinks.wikiLang(nil))
		assert.are.equal("en", SpeciesLinks.wikiLang(""))
		assert.are.equal("en", SpeciesLinks.wikiLang("123"))
	end)
end)

describe("SpeciesLinks.searchUrls", function()
	it("builds a usable URL for every provider", function()
		local urls = SpeciesLinks.searchUrls("Panthera leo", "en")
		for _, provider in ipairs({ "inaturalist", "wikipedia", "gbif", "wikispecies" }) do
			local url = urls[provider]
			assert.is_string(url)
			assert.is_truthy(url:match("^https://"))
			assert.is_nil(url:find(" ", 1, true))
		end
	end)

	it("points Wikipedia at the requested language edition", function()
		assert.is_truthy(SpeciesLinks.searchUrls("Bombus", "de").wikipedia:match("^https://de%.wikipedia%.org/"))
	end)

	it("returns nothing for an empty name", function()
		assert.are.same({}, SpeciesLinks.searchUrls("", "en"))
		assert.are.same({}, SpeciesLinks.searchUrls(nil, "en"))
	end)
end)

describe("SpeciesLinks.forSpecies", function()
	before_each(function()
		SpeciesLinks.clearCache()
		_G.prefs = { speciesLinkLang = "en" }
	end)

	it("returns nil when there is no name to link to", function()
		assert.is_nil(SpeciesLinks.forSpecies(nil))
		assert.is_nil(SpeciesLinks.forSpecies({ rank = "none", scientific_name = "" }))
	end)

	it("falls back to search URLs when the backend answers nothing", function()
		_G.SearchIndexAPI = {
			getSpeciesLinks = function()
				return nil, "connection refused"
			end,
		}
		local urls = SpeciesLinks.forSpecies({ scientific_name = "Panthera leo", rank = "species" })
		assert.are.equal("https://www.inaturalist.org/search?q=Panthera%20leo", urls.inaturalist)
	end)

	it("prefers the backend's deep links over the search fallback", function()
		_G.SearchIndexAPI = {
			getSpeciesLinks = function()
				return {
					resolved = true,
					urls = {
						inaturalist = "https://www.inaturalist.org/taxa/41964",
						wikipedia = "https://en.wikipedia.org/wiki/Lion",
					},
				}
			end,
		}
		local urls = SpeciesLinks.forSpecies({ scientific_name = "Panthera leo", rank = "species" })
		assert.are.equal("https://www.inaturalist.org/taxa/41964", urls.inaturalist)
		assert.are.equal("https://en.wikipedia.org/wiki/Lion", urls.wikipedia)
		-- Providers the backend did not deep-link keep their search URL.
		assert.is_truthy(urls.wikispecies:match("^https://species%.wikimedia%.org/"))
	end)

	it("asks the backend once per taxon, not once per photo", function()
		local calls = 0
		_G.SearchIndexAPI = {
			getSpeciesLinks = function()
				calls = calls + 1
				return { resolved = true, urls = { inaturalist = "https://www.inaturalist.org/taxa/41964" } }
			end,
		}
		local species = { scientific_name = "Panthera leo", rank = "species" }
		SpeciesLinks.forSpecies(species)
		SpeciesLinks.forSpecies(species)
		assert.are.equal(1, calls)
	end)

	it("does not cache a failed lookup, so a late backend still gets a chance", function()
		local calls = 0
		_G.SearchIndexAPI = {
			getSpeciesLinks = function()
				calls = calls + 1
				return nil, "connection refused"
			end,
		}
		local species = { scientific_name = "Panthera leo", rank = "species" }
		SpeciesLinks.forSpecies(species)
		SpeciesLinks.forSpecies(species)
		assert.are.equal(2, calls)
	end)

	it("skips the backend entirely when asked to stay offline", function()
		_G.SearchIndexAPI = {
			getSpeciesLinks = function()
				error("must not be called")
			end,
		}
		local urls = SpeciesLinks.forSpecies({ scientific_name = "Bombus", rank = "genus" }, { offline = true })
		assert.is_truthy(urls.gbif:match("^https://www%.gbif%.org/species/search"))
	end)
end)
