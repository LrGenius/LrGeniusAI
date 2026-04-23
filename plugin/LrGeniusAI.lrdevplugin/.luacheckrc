std = "lua51"

-- Lightroom globals and project-specific globals
globals = {
    "import",
    "LOC",
    "MAC_ENV",
    "WIN_ENV",
    "_PLUGIN",
    "prefs",
    "log",
    "recipe",
    "JSON",
    "Util",
    "ErrorHandler",
    "Info",
    "UpdateCheck",
    "Defaults",
    "KeywordConfigProvider",
    "MetadataManager",
    "OnboardingWizard",
    "PhotoContextData",
    "PhotoSelector",
    "PluginInfoDialogSections",
    "PromptConfigProvider",
    "SearchIndexAPI",
    "SkipPhotoContextDialog",
    "DevelopEditManager"
}

-- Read-only Lightroom SDK namespaces
read_globals = {
    "LrApplication",
    "LrApplicationView",
    "LrBinding",
    "LrColor",
    "LrDate",
    "LrDevelopController",
    "LrDialogs",
    "LrExportSession",
    "LrFileUtils",
    "LrFunctionContext",
    "LrHttp",
    "LrMD5",
    "LrPathUtils",
    "LrProgressScope",
    "LrShell",
    "LrStringUtils",
    "LrTasks",
    "LrView"
}

-- You can selectively ignore certain standard checks here if needed
ignore = {
    "212", -- Unused argument (e.g. in UI callbacks where you don't need 'view')
    "631", -- Line is too long
}
