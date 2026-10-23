-- Unit tests for Util.aestheticFieldValues: what the Metadata panel shows for
-- a photo's aesthetic score and genre.
--
-- Run from the repo root with:  busted

local Util = require("Util")

describe("Util.aestheticFieldValues", function()
	it("formats the score to one decimal, as text", function()
		local score, genre = Util.aestheticFieldValues({ score = 7.4321, genre_label = "Macro" })
		assert.are.equal("7.4", score)
		assert.are.equal("Macro", genre)
	end)

	it("rounds rather than truncates", function()
		assert.are.equal("7.5", (Util.aestheticFieldValues({ score = 7.46 })))
		assert.are.equal("1.0", (Util.aestheticFieldValues({ score = 1 })))
	end)

	it("does not write a score from another export of the model", function()
		-- Not comparable with the current scores, so it must not sit beside them.
		local score, genre = Util.aestheticFieldValues({ score = 6.0, outdated = true, genre_label = "Street" })
		assert.is_nil(score)
		assert.are.equal("Street", genre)
	end)

	it("reports a genre without a score and a score without a genre", function()
		local score, genre = Util.aestheticFieldValues({ genre_label = "Landscape" })
		assert.is_nil(score)
		assert.are.equal("Landscape", genre)
		score, genre = Util.aestheticFieldValues({ score = 5 })
		assert.are.equal("5.0", score)
		assert.are.equal("", genre)
	end)

	it("tolerates a photo with neither", function()
		for _, input in ipairs({ {}, { score = "n/a" }, { genre_label = 12 } }) do
			local score, genre = Util.aestheticFieldValues(input)
			assert.is_nil(score)
			assert.are.equal("", genre)
		end
		local score, genre = Util.aestheticFieldValues(nil)
		assert.is_nil(score)
		assert.are.equal("", genre)
	end)
end)
