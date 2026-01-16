Info = {}

Info.MAJOR = 2
Info.MINOR = 0
Info.REVISION = 0
Info.VERSION = { major = Info.MAJOR, minor = Info.MINOR, revision = Info.REVISION, build = 0, }


return {

	LrSdkVersion = 14.0,
	LrSdkMinimumVersion = 14.0,
	LrToolkitIdentifier = 'LrGeniusAI',
	LrPluginName = "LrGeniusAI",
	LrInitPlugin = "Init.lua",
	LrPluginInfoProvider = 'PluginInfo.lua',
	LrPluginInfoURL = 'https://github.com/LrGenius',

	VERSION = Info.VERSION,

	LrMetadataProvider = "MetadataProvider.lua",
	LrMetadataTagsetFactory = "MetadataTagset.lua",


	LrLibraryMenuItems = {
		{
			title = LOC "$$$/LrGeniusAI/Menu/AnalyzeAndIndex=Analyze & Index Photos...",
			file = "TaskAnalyzeAndIndex.lua",
		},
		{
			title = LOC "$$$/LrGeniusAI/Menu/AdvancedSearch=Advanced Search...",
			file = "TaskSemanticSearch.lua",
		},
		{
			title = LOC "$$$/LrGeniusAI/Menu/RetrieveMetadata=Retrieve Metadata from Backend...",
			file = "TaskRetrieveMetadata.lua",
		},
		{
			title = LOC "$$$/LrGeniusAI/Menu/ImportMetadata=Import Metadata from Catalog...",
			file = "TaskImportMetadata.lua",
		},
	},

	LrExportMenuItems = {
		{
			title = LOC "$$$/LrGeniusAI/Menu/AnalyzeAndIndex=Analyze & Index Photos...",
			file = "TaskAnalyzeAndIndex.lua",
		},
		{
			title = LOC "$$$/LrGeniusAI/Menu/AdvancedSearch=Advanced Search...",
			file = "TaskSemanticSearch.lua",
		},
		{
			title = LOC "$$$/LrGeniusAI/Menu/RetrieveMetadata=Retrieve Metadata from Backend...",
			file = "TaskRetrieveMetadata.lua",
		},
		{
			title = LOC "$$$/LrGeniusAI/Menu/ImportMetadata=Import Metadata from Catalog...",
			file = "TaskImportMetadata.lua",
		},
	},

	LrShutdownApp = "ShutdownApp.lua",
}
