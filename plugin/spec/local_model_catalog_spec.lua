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

-- "Recommended for this computer": the backend picks the model that fits the
-- computer it runs on (`recommended` in /v1/llm/catalog), and the settings and
-- the setup wizard mark it.
describe("LocalModelCatalog recommendation", function()
	local entries = {
		{ id = "mlx-gemma4-12b", label = "Gemma 4 12B", approx_bytes = 11.0e9, recommended = false },
		{ id = "mlx-gemma4-e4b", label = "Gemma 4 E4B (best all-round)", approx_bytes = 5.2e9, recommended = true },
		{ id = "mlx-gemma4-e2b", label = "Gemma 4 E2B", approx_bytes = 3.6e9, installed = true },
	}

	it("marks the recommended entry in the picker, and only that one", function()
		local choices = LocalModelCatalog.downloadChoices(entries)
		assert.are.equal("Gemma 4 12B (11.0 GB)", choices[1].title)
		assert.are.equal("Gemma 4 E4B (best all-round) (5.2 GB) — Recommended for this computer", choices[2].title)
		-- Installed models are not offered; the custom entry closes the list.
		assert.are.equal(3, #choices)
		assert.are.equal(LocalModelCatalog.CUSTOM_CHOICE, choices[3].value)
	end)

	it("preselects the recommended model when nothing is chosen yet", function()
		local choices = LocalModelCatalog.downloadChoices(entries)
		assert.are.equal("mlx-gemma4-e4b", LocalModelCatalog.pickValidChoice(choices, nil, "mlx-gemma4-e4b"))
	end)

	it("keeps the user's own choice over the recommendation", function()
		local choices = LocalModelCatalog.downloadChoices(entries)
		assert.are.equal(
			"mlx-gemma4-12b",
			LocalModelCatalog.pickValidChoice(choices, "mlx-gemma4-12b", "mlx-gemma4-e4b")
		)
	end)

	it("falls back to the first entry when the recommendation is not offered", function()
		local choices = LocalModelCatalog.downloadChoices(entries)
		-- Installed already, or a backend older than the field.
		assert.are.equal("mlx-gemma4-12b", LocalModelCatalog.pickValidChoice(choices, nil, "mlx-gemma4-e2b"))
		assert.are.equal("mlx-gemma4-12b", LocalModelCatalog.pickValidChoice(choices, nil, nil))
		assert.are.equal("mlx-gemma4-12b", LocalModelCatalog.pickValidChoice(choices, "gone", nil))
	end)

	it("says why, and what to do, for each state of the recommendation", function()
		local reason = "Gemma 4 E4B is the best model that fits next to Lightroom on a computer with 16 GB of memory."
		assert.are.equal(
			reason .. " Pick it below and click Download.",
			LocalModelCatalog.recommendationText({ id = "mlx-gemma4-e4b", reason = reason })
		)
		assert.are.equal(
			reason .. " It is installed.",
			LocalModelCatalog.recommendationText({
				id = "mlx-gemma4-e4b",
				reason = reason,
				installed_name = "gemma-4-e4b-it-4bit",
			})
		)
		local none = "This computer has 4 GB of memory; the smallest local model needs 8 GB next to Lightroom."
		assert.are.equal(none, LocalModelCatalog.recommendationText({ reason = none }))
		-- A backend older than the field says nothing.
		assert.are.equal("", LocalModelCatalog.recommendationText(nil))
	end)

	it("marks the recommended model in the installed list", function()
		local names = LocalModelCatalog.installedNames(
			{ { name = "gemma-4-e4b-it-4bit" }, { name = "other" } },
			{ installed_name = "gemma-4-e4b-it-4bit" }
		)
		assert.are.same({ "gemma-4-e4b-it-4bit (recommended for this computer)", "other" }, names)
		assert.are.same({ "other" }, LocalModelCatalog.installedNames({ { name = "other" } }, nil))
	end)
end)
