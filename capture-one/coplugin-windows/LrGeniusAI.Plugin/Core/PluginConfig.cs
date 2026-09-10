using System;
using System.IO;
using System.Text;

namespace LrGeniusAI.CaptureOne.Core
{
    /// <summary>
    /// The two things a user may need to override, read fresh on every run.
    /// </summary>
    /// <remarks>
    /// <para>
    /// The plug-in class holds no fields of its own. Capture One may construct it
    /// per invocation, may keep it alive across many, and hosts it in a separate
    /// process that can be restarted at any time - so anything cached in memory is
    /// either pointless or stale. Settings therefore live in a small file that the
    /// settings UI writes and every run re-reads.
    /// </para>
    /// <para>
    /// Precedence is environment variable, then file, then default. The environment
    /// variables exist so a developer can point the plug-in at a debug build
    /// without touching the user's saved settings.
    /// </para>
    /// </remarks>
    internal sealed class PluginConfig
    {
        /// <summary>Overrides the backend address for one process.</summary>
        public const string BackendUrlEnvironmentVariable = "LRGENIUS_BACKEND_URL";

        /// <summary>Overrides the helper executable path for one process.</summary>
        public const string HelperPathEnvironmentVariable = "LRGENIUS_C1_EXE";

        /// <summary>Base URL of the local backend.</summary>
        public string BackendBaseUrl { get; set; }

        /// <summary>
        /// Full path to the helper executable, or null to search the usual places.
        /// </summary>
        public string HelperExecutablePath { get; set; }

        /// <summary>Settings as shipped.</summary>
        public static PluginConfig Defaults()
        {
            return new PluginConfig
            {
                BackendBaseUrl = HandoffClient.DefaultBaseUrl,
                HelperExecutablePath = null,
            };
        }

        /// <summary>
        /// Reads the settings file, falling back to defaults for anything missing
        /// or unreadable.
        /// </summary>
        /// <remarks>
        /// Never throws. A corrupt settings file must not stop a photographer from
        /// running the plug-in; it just means the defaults apply. The corruption is
        /// reported through <paramref name="report"/> so it is not silent either.
        /// </remarks>
        public static PluginConfig Load(RunReport report)
        {
            PluginConfig config = Defaults();
            string path = PluginInfo.ConfigFilePath;

            try
            {
                if (File.Exists(path))
                {
                    object root = Json.Parse(File.ReadAllText(path, Encoding.UTF8));
                    string url = Json.GetString(root, "backend_base_url");
                    if (!string.IsNullOrEmpty(url))
                    {
                        config.BackendBaseUrl = url;
                    }

                    string helper = Json.GetString(root, "helper_executable_path");
                    if (!string.IsNullOrEmpty(helper))
                    {
                        config.HelperExecutablePath = helper;
                    }
                }
            }
            catch (Exception ex) when (ex is IOException
                                       || ex is UnauthorizedAccessException
                                       || ex is FormatException)
            {
                if (report != null)
                {
                    report.AddWarning(
                        "Your LrGeniusAI plug-in settings could not be read (" + ex.Message
                        + "), so the built-in defaults were used. Open the plug-in settings and "
                        + "save them again to repair the file at " + path + ".");
                }
            }

            string urlOverride = SafeGetEnvironmentVariable(BackendUrlEnvironmentVariable);
            if (!string.IsNullOrEmpty(urlOverride))
            {
                config.BackendBaseUrl = urlOverride;
            }

            string helperOverride = SafeGetEnvironmentVariable(HelperPathEnvironmentVariable);
            if (!string.IsNullOrEmpty(helperOverride))
            {
                config.HelperExecutablePath = helperOverride;
            }

            return config;
        }

        /// <summary>
        /// Writes the settings file.
        /// </summary>
        /// <returns>Null on success, otherwise an actionable message.</returns>
        public string Save()
        {
            string path = PluginInfo.ConfigFilePath;
            try
            {
                string directory = Path.GetDirectoryName(path);
                if (!string.IsNullOrEmpty(directory) && !Directory.Exists(directory))
                {
                    Directory.CreateDirectory(directory);
                }

                var sb = new StringBuilder();
                sb.Append("{")
                  .Append("\"backend_base_url\":").Append(Json.Quote(BackendBaseUrl)).Append(",")
                  .Append("\"helper_executable_path\":").Append(Json.Quote(HelperExecutablePath))
                  .Append("}");

                File.WriteAllText(path, sb.ToString(), new UTF8Encoding(false));
                return null;
            }
            catch (Exception ex) when (ex is IOException || ex is UnauthorizedAccessException)
            {
                return "Your LrGeniusAI settings could not be saved to " + path + " (" + ex.Message
                       + "). Check that the folder exists and is writable, then save again.";
            }
        }

        private static string SafeGetEnvironmentVariable(string name)
        {
            try
            {
                return Environment.GetEnvironmentVariable(name);
            }
            catch (System.Security.SecurityException)
            {
                return null;
            }
        }
    }
}
