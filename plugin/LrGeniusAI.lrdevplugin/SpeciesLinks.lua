-- SpeciesLinks.lua
-- Builds the web links that turn a BioCLIP identification into something the
-- user can click.
--
-- Two levels of quality, and the difference is why this module exists:
--
--  * Offline, from the name alone: a *search* URL on each provider. Always
--    available, never wrong, but lands on a results page.
--  * From the backend's /species/links: a *deep* link to the taxon's own page,
--    because the backend resolved the name to a GBIF usage key and an
--    iNaturalist taxon id (see `species_links.rs`).
--
-- The deep links are what gets written into `speciesInatUrl` /
-- `speciesWikipediaUrl`, which Lightroom renders as clickable links in the
-- Metadata panel. The search URLs are the fallback when the backend is
-- unreachable or the name is not in either database — a link one click from
-- the answer beats an empty field.

SpeciesLinks = {}

-- Session cache, keyed by name|rank|lang. The backend caches across restarts
-- too, but a catalog of bird photos asks for the same twenty names thousands
-- of times, and this keeps that from becoming thousands of HTTP round trips to
-- localhost.
local sessionCache = {}

local function cacheKey(name, rank, lang)
	return string.lower(tostring(name)) .. "|" .. string.lower(tostring(rank or "")) .. "|" .. tostring(lang or "en")
end

---
-- Percent-encodes a string for use in a query parameter.
--
-- `LrStringUtils.encodeUrl` is not usable here: it leaves `&` and `=` intact
-- (it encodes a whole URL, not a component), so a species name containing one
-- would break out of its parameter.
--
-- @param s string
-- @return string
function SpeciesLinks.encodeComponent(s)
	if type(s) ~= "string" then
		return ""
	end
	return (s:gsub("[^%w%-%._~]", function(c)
		return string.format("%%%02X", string.byte(c))
	end))
end

---
-- Normalizes a Lightroom language tag to a Wikipedia subdomain.
--
-- Lightroom reports things like `de` or `zh-Hans`; Wikipedia hosts are the bare
-- two-letter language, and an unknown one would 404 the whole link.
--
-- @param lang string|nil
-- @return string
function SpeciesLinks.wikiLang(lang)
	local base = tostring(lang or ""):match("^(%a%a)") or ""
	if base == "" then
		return "en"
	end
	return string.lower(base)
end

---
-- The interface language to build links for.
--
-- `LrLocalization.currentLanguage` is not in the published SDK reference, so it
-- is called defensively: a Lightroom build without it must fall back to
-- English, not throw inside a metadata write.
--
-- @return string two-letter language code
function SpeciesLinks.currentLanguage()
	local configured = prefs and prefs.speciesLinkLang
	if type(configured) == "string" and configured ~= "" and configured ~= "auto" then
		return SpeciesLinks.wikiLang(configured)
	end
	local ok, lang = pcall(function()
		return LrLocalization.currentLanguage()
	end)
	if ok and type(lang) == "string" and lang ~= "" then
		return SpeciesLinks.wikiLang(lang)
	end
	return "en"
end

---
-- Name-based search URLs — the offline fallback, and what the backend returns
-- for a taxon neither database knows.
--
-- @param name string scientific name
-- @param lang string|nil
-- @return table provider id -> url
function SpeciesLinks.searchUrls(name, lang)
	if type(name) ~= "string" or name == "" then
		return {}
	end
	local q = SpeciesLinks.encodeComponent(name)
	local wiki = SpeciesLinks.wikiLang(lang)
	return {
		inaturalist = "https://www.inaturalist.org/search?q=" .. q,
		wikipedia = "https://" .. wiki .. ".wikipedia.org/w/index.php?search=" .. q,
		gbif = "https://www.gbif.org/species/search?q=" .. q,
		-- Wikispecies files scientific names as article titles verbatim, so it
		-- is the one provider that deep-links without a lookup.
		wikispecies = "https://species.wikimedia.org/wiki/" .. SpeciesLinks.encodeComponent(name),
	}
end

---
-- Resolves links for one identification, preferring deep links.
--
-- @param species table backend species block (scientific_name, rank, ...)
-- @param options table|nil `offline` skips the backend entirely.
-- @return table|nil provider id -> url, or nil when there is no name to link.
function SpeciesLinks.forSpecies(species, options)
	if type(species) ~= "table" then
		return nil
	end
	local name = species.scientific_name
	if type(name) ~= "string" or name == "" then
		return nil
	end
	options = options or {}
	local lang = options.lang or SpeciesLinks.currentLanguage()
	local rank = species.rank
	if rank == "none" then
		rank = nil
	end

	local key = cacheKey(name, rank, lang)
	local cached = sessionCache[key]
	if cached then
		return cached
	end

	local urls = SpeciesLinks.searchUrls(name, lang)
	if not options.offline then
		local resolved = SearchIndexAPI.getSpeciesLinks(name, rank, lang)
		if type(resolved) == "table" and type(resolved.urls) == "table" then
			for provider, url in pairs(resolved.urls) do
				if type(url) == "string" and url ~= "" then
					urls[provider] = url
				end
			end
			-- Only cache an answer that came from the backend. Caching the
			-- offline fallback would pin the search URLs for the rest of the
			-- session, so a backend that finishes starting up mid-run would
			-- never get a chance to upgrade them.
			sessionCache[key] = urls
		end
	end
	return urls
end

---
-- Drops the session cache. Called when the backend's reachability changes so a
-- run that started offline can pick up deep links later.
function SpeciesLinks.clearCache()
	sessionCache = {}
end
