---@diagnostic disable: undefined-global

-- Global imports
_G.LrHttp = import("LrHttp")
_G.LrDate = import("LrDate")
_G.LrPathUtils = import("LrPathUtils")
_G.LrFileUtils = import("LrFileUtils")
_G.LrTasks = import("LrTasks")
_G.LrErrors = import("LrErrors")
_G.LrDialogs = import("LrDialogs")
_G.LrView = import("LrView")
_G.LrBinding = import("LrBinding")
_G.LrColor = import("LrColor")
_G.LrFunctionContext = import("LrFunctionContext")
_G.LrApplication = import("LrApplication")
_G.LrPrefs = import("LrPrefs")
_G.LrProgressScope = import("LrProgressScope")
_G.LrExportSession = import("LrExportSession")
_G.LrStringUtils = import("LrStringUtils")
_G.LrMD5 = import("LrMD5")
_G.LrLocalization = import("LrLocalization")
_G.LrShell = import("LrShell")
_G.LrSystemInfo = import("LrSystemInfo")
_G.LrApplicationView = import("LrApplicationView")
_G.LrDevelopController = import("LrDevelopController")

-- Global initializations (move early)
_G.prefs = _G.LrPrefs.prefsForPlugin()
_G.log = import("LrLogger")("LrGeniusAI")
_G.prefs.logging = true
_G.log:enable("logfile")

-- Load modules early
_G.JSON = require("JSON")
require("Util")
require("Defaults")
require("MetadataManager")
require("KeywordConfigProvider")
require("PromptConfigProvider")
require("UpdateCheck")
require("ErrorHandler")
require("APISearchIndex")
require("SpeciesLinks")
require("PhotoSelector")
require("LocalModelCatalog")
require("OnboardingWizard")

if _G.prefs.ai == nil then
	_G.prefs.ai = ""
end

if _G.prefs.geminiApiKey == nil then
	_G.prefs.geminiApiKey = ""
end

if _G.prefs.chatgptApiKey == nil then
	_G.prefs.chatgptApiKey = ""
end

if _G.prefs.vertexProjectId == nil then
	_G.prefs.vertexProjectId = ""
end

if _G.prefs.vertexLocation == nil then
	_G.prefs.vertexLocation = "us-central1"
end

if _G.prefs.generateTitle == nil then
	_G.prefs.generateTitle = true
end

if _G.prefs.generateKeywords == nil then
	_G.prefs.generateKeywords = true
end

if _G.prefs.generateCaption == nil then
	_G.prefs.generateCaption = true
end

if _G.prefs.generateAltText == nil then
	_G.prefs.generateAltText = true
end

if _G.prefs.enableValidation == nil then
	_G.prefs.enableValidation = true
end

if _G.prefs.showCosts == nil then
	_G.prefs.showCosts = true
end

if _G.prefs.generateLanguage == nil then
	_G.prefs.generateLanguage = Defaults.defaultGenerateLanguage
end

if _G.prefs.bilingualKeywords == nil then
	_G.prefs.bilingualKeywords = Defaults.defaultBilingualKeywords
end

if _G.prefs.keywordSecondaryLanguage == nil then
	_G.prefs.keywordSecondaryLanguage = Defaults.defaultKeywordSecondaryLanguage
end

if _G.prefs.keywordAliases == nil then
	_G.prefs.keywordAliases = Defaults.defaultKeywordAliases
end

if _G.prefs.replaceSS == nil then
	_G.prefs.replaceSS = false
end

if _G.prefs.indexSubmitOriginals == nil then
	_G.prefs.indexSubmitOriginals = false
end

if _G.prefs.exportSize == nil then
	_G.prefs.exportSize = Defaults.defaultExportSize
end

if _G.prefs.exportQuality == nil then
	_G.prefs.exportQuality = Defaults.defaultExportQuality
end

if _G.prefs.showPhotoContextDialog == nil then
	_G.prefs.showPhotoContextDialog = true
end

if _G.prefs.submitKeywords == nil then
	_G.prefs.submitKeywords = true
end

-- Seeded from the keyword switch rather than simply defaulting to true.
-- Until now the names Lightroom put on faces travelled with the keywords and
-- were sent only when that box was ticked, so anyone who had it off had never
-- sent a name to a cloud provider. Defaulting the new switch on would start
-- doing that silently on upgrade; taking the answer they already gave keeps
-- every install exactly where it was. From here the two are independent.
if _G.prefs.submitFaceNames == nil then
	_G.prefs.submitFaceNames = _G.prefs.submitKeywords ~= false
end

if _G.prefs.temperature == nil then
	_G.prefs.temperature = Defaults.defaultTemperature
end

if _G.prefs.maxTokens == nil then
	_G.prefs.maxTokens = Defaults.defaultMaxTokens
end

if _G.prefs.useKeywordHierarchy == nil then
	_G.prefs.useKeywordHierarchy = true
end

if _G.prefs.useTopLevelKeyword == nil then
	_G.prefs.useTopLevelKeyword = true
end

-- The prompts that ship with the plugin, added once each -- see
-- Util.seedBuiltinPrompts for why "once" and not "whenever missing".
local seededPrompts, offeredPrompts, promptsChanged =
	Util.seedBuiltinPrompts(_G.prefs.prompts, _G.prefs.seededPrompts, Defaults.builtinPrompts)
if promptsChanged then
	-- Assigned back, not mutated in place: LrPrefs persists a table field when
	-- it is written, not when its contents change.
	_G.prefs.prompts = seededPrompts
	_G.prefs.seededPrompts = offeredPrompts
end

-- `Default` going missing is not a delete anyone could have made: the Delete
-- button refuses that name. So a table without it lost it, which is what
-- clearing a prompt's text used to do (see Util.storePromptText), and putting
-- it back is a repair rather than a re-offer of a preset the user removed.
-- An empty table is the same story with nothing left at all, and would leave
-- the prompt picker with no items.
if type(_G.prefs.prompts) ~= "table" or next(_G.prefs.prompts) == nil then
	_G.prefs.prompts = Defaults.builtinPromptTable()
elseif _G.prefs.prompts[Defaults.defaultPromptName] == nil then
	local repaired = Util.deepcopy(_G.prefs.prompts)
	repaired[Defaults.defaultPromptName] = Defaults.defaultSystemInstruction
	_G.prefs.prompts = repaired
end

-- Resolved, not just defaulted: a name that no longer exists in the table is
-- as broken as no name at all, and leaves the prompt picker showing a value
-- none of its items carry.
_G.prefs.prompt = Util.resolvePromptName(_G.prefs.prompts, _G.prefs.prompt, Defaults.defaultPromptName)

if _G.prefs.editPrompts == nil then
	_G.prefs.editPrompts = { Default = Defaults.defaultEditSystemInstruction }
end

if _G.prefs.editPrompt == nil then
	_G.prefs.editPrompt = Defaults.defaultEditPromptName
end

if _G.prefs.ollamaBaseUrl == nil then
	_G.prefs.ollamaBaseUrl = Defaults.defaultOllamaBaseUrl
end

if _G.prefs.lmstudioBaseUrl == nil then
	_G.prefs.lmstudioBaseUrl = Defaults.defaultLmStudioBaseUrl
end

if _G.prefs.backendServerUrl == nil or _G.prefs.backendServerUrl == "" then
	_G.prefs.backendServerUrl = Defaults.defaultBackendServerUrl
end

if _G.prefs.periodicalUpdateCheck == nil then
	_G.prefs.periodicalUpdateCheck = false
end

if _G.prefs.submitFolderName == nil then
	_G.prefs.submitFolderName = false
end

if _G.prefs.useGlobalPhotoId == nil then
	_G.prefs.useGlobalPhotoId = true
end

if _G.prefs.useLightroomKeywords == nil then
	_G.prefs.useLightroomKeywords = false
end

if _G.prefs.topLevelKeyword == nil then
	_G.prefs.topLevelKeyword = Defaults.defaultTopLevelKeyword
end

if _G.prefs.knownTopLevelKeywords == nil then
	_G.prefs.knownTopLevelKeywords = Defaults.defaultTopLevelKeywords
end

if _G.prefs.useClip == nil then
	_G.prefs.useClip = false
end

-- Advanced Search dialog options (persisted for convenience)
if _G.prefs.searchScope == nil then
	_G.prefs.searchScope = "all"
end
if _G.prefs.searchInSemanticSiglip == nil then
	_G.prefs.searchInSemanticSiglip = true
end
if _G.prefs.searchInSemanticVertex == nil then
	_G.prefs.searchInSemanticVertex = true
end
if _G.prefs.searchInMetadata == nil then
	_G.prefs.searchInMetadata = true
end
if _G.prefs.searchInMetadataKeywords == nil then
	_G.prefs.searchInMetadataKeywords = true
end
if _G.prefs.searchInMetadataCaption == nil then
	_G.prefs.searchInMetadataCaption = true
end
if _G.prefs.searchInMetadataTitle == nil then
	_G.prefs.searchInMetadataTitle = true
end
if _G.prefs.searchInMetadataAltText == nil then
	_G.prefs.searchInMetadataAltText = true
end

function _G.JSON.assert(v, message)
	if not v then
		log:error("JSON error: " .. (message or "assertion failed!"))
		error(message or "assertion failed!")
	end
	return v
end

-- Update check is now handled via Backend + Util.waitForServerDialog

-- if prefs.onboardingCompleted == nil then
-- 	Do not set to false yet, let the wizard trigger
-- end

LrTasks.startAsyncTask(function()
	-- Check if onboarding is needed
	-- if not prefs.onboardingCompleted then
	--     OnboardingWizard.show()
	-- end

	if SearchIndexAPI.startServer() then
		SearchIndexAPI.checkServerHealth()
		if prefs.useClip then
			SearchIndexAPI.isClipReady() -- To trigger load of the CLIP model.
		end
	end
end)
