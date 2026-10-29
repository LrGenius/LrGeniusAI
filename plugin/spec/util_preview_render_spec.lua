-- Util.jpegDimensions: reads a JPEG's size from its frame header, which is how
-- a Lightroom preview is checked against the size an export would have had.

local Util = require("Util")

local function u16(n)
	return string.char(math.floor(n / 256), n % 256)
end

-- SOI, an APP0 segment, then a frame header of the given marker and size.
local function jpeg(marker, width, height)
	local app0 = "\255\224" .. u16(16) .. "JFIF\0" .. string.rep("\0", 9)
	local sof = "\255" .. string.char(marker) .. u16(17) .. "\8" .. u16(height) .. u16(width) .. string.rep("\0", 10)
	return "\255\216" .. app0 .. sof .. "\255\218"
end

describe("Util.jpegDimensions", function()
	it("reads a baseline frame header after other segments", function()
		local w, h = Util.jpegDimensions(jpeg(0xC0, 1024, 683))
		assert.are.equal(1024, w)
		assert.are.equal(683, h)
	end)

	it("reads a progressive frame header", function()
		local w, h = Util.jpegDimensions(jpeg(0xC2, 640, 960))
		assert.are.equal(640, w)
		assert.are.equal(960, h)
	end)

	it("skips a Huffman table, which shares the SOF marker range", function()
		local dht = "\255\196" .. u16(5) .. "\0\0\0"
		local data = "\255\216" .. dht .. jpeg(0xC0, 300, 200):sub(3)
		local w, h = Util.jpegDimensions(data)
		assert.are.equal(300, w)
		assert.are.equal(200, h)
	end)

	it("reads a real file", function()
		local fh = assert(io.open("server-rs/testdata/astronaut.jpg", "rb"))
		local data = fh:read("*a")
		fh:close()
		local w, h = Util.jpegDimensions(data)
		assert.are.equal(512, w)
		assert.are.equal(512, h)
	end)

	it("answers nil for anything that is not a usable JPEG", function()
		assert.is_nil(Util.jpegDimensions(nil))
		assert.is_nil(Util.jpegDimensions(""))
		assert.is_nil(Util.jpegDimensions("\137PNG\r\n\26\n"))
		-- Truncated inside the frame header.
		assert.is_nil(Util.jpegDimensions(jpeg(0xC0, 10, 10):sub(1, 26)))
		-- Image data before any frame header.
		assert.is_nil(Util.jpegDimensions("\255\216\255\218\0\0"))
	end)
end)

describe("Util.exportChunkSize", function()
	it("defaults when nothing usable is saved", function()
		assert.are.equal(Util.DEFAULT_EXPORT_CHUNK_SIZE, Util.exportChunkSize(nil))
		assert.are.equal(Util.DEFAULT_EXPORT_CHUNK_SIZE, Util.exportChunkSize("lots"))
		assert.are.equal(Util.DEFAULT_EXPORT_CHUNK_SIZE, Util.exportChunkSize(0 / 0))
	end)

	it("keeps a saved size within bounds", function()
		assert.are.equal(40, Util.exportChunkSize("40"))
		assert.are.equal(7, Util.exportChunkSize(7.9))
		assert.are.equal(1, Util.exportChunkSize(-3))
		assert.are.equal(200, Util.exportChunkSize(5000))
	end)
end)

describe("Util.previewFallbackWarning", function()
	it("says how many were exported and what makes the next run use previews", function()
		local text = Util.previewFallbackWarning(3, 40, 768)
		assert.truthy(text:find("3 of 40 photos", 1, true))
		assert.truthy(text:find("768 px", 1, true))
		assert.truthy(text:find("Build Standard-Sized Previews", 1, true))
	end)
end)
