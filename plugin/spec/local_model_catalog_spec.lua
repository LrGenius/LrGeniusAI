-- Unit tests for the confirmation text shown before a model from Hugging Face
-- is downloaded ("Other model from Hugging Face…", #341).
--
-- Run from the repo root with:  busted

local LocalModelCatalog = require("LocalModelCatalog")

local GIB = 1024 * 1024 * 1024

local function mlxCheck(overrides)
	local check = {
		repo = "mlx-community/gemma-3-12b-it-qat-4bit",
		model_type = "gemma3",
		approx_bytes = 8.1e9,
		est_ram_gb = 11.2,
		warnings = { "This model is not in LrGeniusAI's tested list." },
	}
	for k, v in pairs(overrides or {}) do
		check[k] = v
	end
	return check
end

describe("LocalModelCatalog.customDownloadSummary", function()
	it("names the model, the download and the memory it needs against what there is", function()
		local text = LocalModelCatalog.customDownloadSummary(mlxCheck(), 32 * GIB)
		assert.truthy(text:find("mlx-community/gemma-3-12b-it-qat-4bit (gemma3)", 1, true))
		assert.truthy(text:find("8.1 GB to download", 1, true))
		assert.truthy(text:find("needs about 11 GB of memory", 1, true))
		assert.truthy(text:find("this computer has 32 GB", 1, true))
		assert.truthy(text:find("tested list", 1, true))
		assert.is_nil(text:find("comfortably spare", 1, true))
	end)

	it("warns when the model needs most of the computer's memory", function()
		-- The #341 reporter: 16 GB MacBook, a model that wants 11+ GB.
		local text = LocalModelCatalog.customDownloadSummary(mlxCheck(), 16 * GIB)
		assert.truthy(text:find("comfortably spare", 1, true))
	end)

	it("still works when the memory size is unknown", function()
		local text = LocalModelCatalog.customDownloadSummary(mlxCheck(), nil)
		assert.truthy(text:find("needs about 11 GB of memory.", 1, true))
		assert.is_nil(text:find("this computer has", 1, true))
	end)

	it("shows the quantization of a GGUF model", function()
		local text = LocalModelCatalog.customDownloadSummary(
			{ repo = "ggml-org/gemma-3-12b-it-GGUF", quant = "Q4_K_M", approx_bytes = 8.15e9, est_ram_gb = 11 },
			nil
		)
		assert.truthy(text:find("ggml-org/gemma-3-12b-it-GGUF (Q4_K_M)", 1, true))
	end)
end)
