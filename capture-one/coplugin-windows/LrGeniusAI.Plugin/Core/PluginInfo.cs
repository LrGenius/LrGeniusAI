using System;

namespace LrGeniusAI.CaptureOne.Core
{
    /// <summary>
    /// Constants that identify this plug-in. Everything here is a compile-time
    /// constant on purpose: Capture One asks for the name and identifier while it
    /// is building a menu, and that path must never touch the network, the disk,
    /// or any cached state.
    /// </summary>
    /// <remarks>
    /// <see cref="Version"/> is duplicated in <c>manifest.xml</c> and in the
    /// csproj. <c>scripts/package.ps1</c> fails the build when the three drift
    /// apart, so change all three together.
    /// </remarks>
    internal static class PluginInfo
    {
        /// <summary>Reverse-DNS plug-in identifier. Also the install folder name.</summary>
        public const string Identifier = "cloud.machek.lrgeniusai";

        /// <summary>Name shown in Capture One's menus.</summary>
        public const string DisplayName = "LrGeniusAI";

        /// <summary>Keep in sync with manifest.xml and LrGeniusAI.Plugin.csproj.</summary>
        public const string Version = "0.1.0";

        /// <summary>Label for the single "Open With" action this plug-in contributes.</summary>
        public const string AnalyzeActionName = "Analyze with LrGeniusAI";

        /// <summary>Stable action id sent to the backend so it knows what was asked for.</summary>
        public const string AnalyzeActionId = "analyze";

        /// <summary>Sent as the User-Agent so backend logs can tell hosts apart.</summary>
        public static readonly string UserAgent = "LrGeniusAI-CaptureOne-Windows/" + Version;

        /// <summary>
        /// Where per-user settings live. The plug-in object itself holds no state,
        /// so every run re-reads this file; the settings UI writes it.
        /// </summary>
        public static string ConfigFilePath
        {
            get
            {
                string root = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
                return System.IO.Path.Combine(
                    System.IO.Path.Combine(root, "LrGeniusAI"),
                    "capture-one-plugin.json");
            }
        }
    }
}
