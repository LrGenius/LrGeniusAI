Info = {}

Info.MAJOR = 9
Info.MINOR = 9
Info.REVISION = 9
Info.BUILD = 99991212
Info.VERSION = { major = Info.MAJOR, minor = Info.MINOR, revision = Info.REVISION, build = Info.BUILD }

return {

	LrSdkVersion = 14.0,
	LrSdkMinimumVersion = 14.0,
	LrToolkitIdentifier = "LrGeniusAI",
	LrPluginName = "LrGeniusAI",
	LrInitPlugin = "Init.lua",
	LrPluginInfoProvider = "PluginInfo.lua",
	LrPluginInfoURL = "https://github.com/LrGenius",

	VERSION = Info.VERSION,

	LrMetadataProvider = "MetadataProvider.lua",
	LrMetadataTagsetFactory = "MetadataTagset.lua",

	LrLibraryMenuItems = {
		{
			title = LOC("$$$/LrGeniusAI/Menu/AnalyzeAndIndex=Analyze & Index Photos..."),
			file = "TaskAnalyzeAndIndex.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Info/AiEditPhotosTitle=AI Edit Photos..."),
			file = "TaskAiEditPhotos.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/AdvancedSearch=Advanced Search..."),
			file = "TaskSemanticSearch.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/CullPhotos=Cull Similar Photos..."),
			file = "TaskCullPhotos.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/RetrieveMetadata=Retrieve Metadata from Backend..."),
			file = "TaskRetrieveMetadata.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/ImportMetadata=Import Metadata from Catalog..."),
			file = "TaskImportMetadata.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/People=People..."),
			file = "TaskPeople.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/FindSimilarFaces=Find Similar Faces..."),
			file = "TaskFindSimilarFaces.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/FindSimilarImages=Find Similar Images..."),
			file = "TaskFindSimilarImages.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Training/MenuItem=Save Edits as AI Training Examples..."),
			file = "TaskTrainFromEdits.lua",
		},
	},

	LrExportMenuItems = {
		{
			title = LOC("$$$/LrGeniusAI/Menu/AnalyzeAndIndex=Analyze & Index Photos..."),
			file = "TaskAnalyzeAndIndex.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Info/AiEditPhotosTitle=AI Edit Photos..."),
			file = "TaskAiEditPhotos.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/AdvancedSearch=Advanced Search..."),
			file = "TaskSemanticSearch.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/CullPhotos=Cull Similar Photos..."),
			file = "TaskCullPhotos.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/RetrieveMetadata=Retrieve Metadata from Backend..."),
			file = "TaskRetrieveMetadata.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/ImportMetadata=Import Metadata from Catalog..."),
			file = "TaskImportMetadata.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/People=People..."),
			file = "TaskPeople.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/FindSimilarFaces=Find Similar Faces..."),
			file = "TaskFindSimilarFaces.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Menu/FindSimilarImages=Find Similar Images..."),
			file = "TaskFindSimilarImages.lua",
		},
		{
			title = LOC("$$$/LrGeniusAI/Training/MenuItem=Save Edits as AI Training Examples..."),
			file = "TaskTrainFromEdits.lua",
		},
	},

	LrHelpMenuItems = {
		{
			title = "Developer: Run Automated Tests...",
			file = "TaskAutomatedTests.lua",
		},
	},

	LrShutdownApp = "ShutdownApp.lua",
}
