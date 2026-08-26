-- Unit tests for the face-tag helpers in Util.lua.
--
-- Lightroom hands a named face over as an ordinary keyword, so once the
-- keywords have been flattened to strings there is nothing left to say that
-- "Ivo" is a person. That is how a caption ended up describing "the rocky
-- shore of Ivo Beach" (issue #315). These cover the two halves of the fix:
-- recognising person keywords, and keeping them out of the plain keyword list.

local Util = require("Util")

--- A keyword double: `keywordType` "person" is what Lightroom sets on a face.
local function keyword(name, keywordType, synonyms)
	return {
		getName = function()
			return name
		end,
		getAttributes = function()
			return { keywordType = keywordType, synonyms = synonyms }
		end,
	}
end

--- A photo double whose `keywords` raw metadata is the given keyword list.
local function photoWithKeywords(keywords)
	return {
		getRawMetadata = function(_, key)
			if key == "keywords" then
				return keywords
			end
			return nil
		end,
	}
end

describe("Util.getPersonKeywordNames", function()
	it("returns only the keywords Lightroom marked as people", function()
		local photo = photoWithKeywords({
			keyword("Ivo", "person"),
			keyword("Beach", nil),
			keyword("Anna", "person"),
		})
		assert.are.same({ "Ivo", "Anna" }, Util.getPersonKeywordNames(photo))
	end)

	it("returns an empty list when no face was tagged", function()
		local photo = photoWithKeywords({ keyword("Beach", nil) })
		assert.are.same({}, Util.getPersonKeywordNames(photo))
	end)

	it("deduplicates names that differ only in case", function()
		local photo = photoWithKeywords({ keyword("Ivo", "person"), keyword("ivo", "person") })
		assert.are.same({ "Ivo" }, Util.getPersonKeywordNames(photo))
	end)

	it("counts a person keyword's synonyms as names too", function()
		-- Lightroom writes synonyms into the exported keyword list beside the
		-- name, so a nickname left out here would read as scenery again.
		local photo = photoWithKeywords({ keyword("Ivo Martins", "person", { "Ivo" }), keyword("Beach", nil) })
		assert.are.same({ "Ivo Martins", "Ivo" }, Util.getPersonKeywordNames(photo))
	end)

	it("ignores the synonyms of keywords that are not people", function()
		local photo = photoWithKeywords({ keyword("Beach", nil, { "Shore" }) })
		assert.are.same({}, Util.getPersonKeywordNames(photo))
	end)

	it("survives a keyword object without getAttributes", function()
		-- Not every keyword-like object implements it, and a missing method
		-- must degrade to "no face tags" rather than abort the indexing run.
		local photo = photoWithKeywords({ {
			getName = function()
				return "Beach"
			end,
		} })
		assert.are.same({}, Util.getPersonKeywordNames(photo))
	end)

	it("returns an empty list for a photo whose keywords cannot be read", function()
		assert.are.same({}, Util.getPersonKeywordNames(photoWithKeywords(nil)))
		assert.are.same({}, Util.getPersonKeywordNames(nil))
	end)
end)

describe("Util.partitionPersonKeywords", function()
	it("moves person names out of the keyword list", function()
		local plain, faces = Util.partitionPersonKeywords({ "beach", "Ivo", "sunset" }, { "Ivo" })
		assert.are.same({ "beach", "sunset" }, plain)
		assert.are.same({ "Ivo" }, faces)
	end)

	it("matches a person name regardless of case", function()
		local plain, faces = Util.partitionPersonKeywords({ "IVO", "beach" }, { "Ivo" })
		assert.are.same({ "beach" }, plain)
		-- Reported with the catalog's spelling, not the keyword list's.
		assert.are.same({ "Ivo" }, faces)
	end)

	it("only matches whole names", function()
		-- "Ivory Coast" is a place that merely starts with a person's name.
		local plain, faces = Util.partitionPersonKeywords({ "Ivory Coast", "Ivo" }, { "Ivo" })
		assert.are.same({ "Ivory Coast" }, plain)
		assert.are.same({ "Ivo" }, faces)
	end)

	it("trims the whitespace a split keyword string leaves behind", function()
		local plain, faces = Util.partitionPersonKeywords({ " beach", " Ivo " }, { "Ivo" })
		assert.are.same({ "beach" }, plain)
		assert.are.same({ "Ivo" }, faces)
	end)

	it("drops empty entries", function()
		local plain = Util.partitionPersonKeywords({ "beach", "", "   " }, {})
		assert.are.same({ "beach" }, plain)
	end)

	it("reports no face tags when none of the names is on this photo", function()
		-- A person keyword excluded from export never reaches the keyword
		-- list, and claiming they are in the frame would invent context.
		local plain, faces = Util.partitionPersonKeywords({ "beach" }, { "Ivo" })
		assert.are.same({ "beach" }, plain)
		assert.are.same({}, faces)
	end)

	it("returns the keywords untouched when there are no person names", function()
		local plain, faces = Util.partitionPersonKeywords({ "beach", "sunset" }, nil)
		assert.are.same({ "beach", "sunset" }, plain)
		assert.are.same({}, faces)
	end)

	it("handles a photo with no keywords at all", function()
		local plain, faces = Util.partitionPersonKeywords(nil, { "Ivo" })
		assert.are.same({}, plain)
		assert.are.same({}, faces)
	end)

	it("lists each person once even when a name is tagged twice", function()
		local _, faces = Util.partitionPersonKeywords({ "Ivo", "ivo" }, { "Ivo" })
		assert.are.same({ "Ivo" }, faces)
	end)
end)
