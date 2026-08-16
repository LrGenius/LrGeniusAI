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
