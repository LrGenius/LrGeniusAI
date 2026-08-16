-- "Append to existing values instead of replacing" used to concatenate
-- unconditionally. On a delta run the backend hands back the *stored* text —
-- the same words that were written to the catalog last time — so appending it
-- to itself doubled the field, and doubled it again on every further run.

require("MetadataManager")

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
