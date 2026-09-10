using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Text;

namespace LrGeniusAI.CaptureOne.Core
{
    /// <summary>Result of trying to start the helper application.</summary>
    internal sealed class LaunchOutcome
    {
        /// <summary>True when the helper is running, or ran and exited cleanly.</summary>
        public bool Started { get; private set; }

        /// <summary>Actionable failure text, or null on success.</summary>
        public string Failure { get; private set; }

        public static LaunchOutcome Ok()
        {
            return new LaunchOutcome { Started = true };
        }

        public static LaunchOutcome Failed(string failure)
        {
            return new LaunchOutcome { Started = false, Failure = failure };
        }
    }

    /// <summary>
    /// Starts <c>lrgenius-c1.exe</c> when the backend cannot take the hand-off
    /// directly.
    /// </summary>
    /// <remarks>
    /// This is the path that actually works today, since the backend has no
    /// <c>/v1/host/handoff</c> route yet. The helper owns the run from the moment
    /// it starts: it shows its own progress, and it is responsible for deleting the
    /// request file it was handed.
    /// </remarks>
    internal static class FallbackLauncher
    {
        /// <summary>File name of the helper application.</summary>
        public const string ExecutableName = "lrgenius-c1.exe";

        /// <summary>
        /// How long to watch a freshly started helper before assuming it is fine.
        /// </summary>
        /// <remarks>
        /// A helper that dies immediately - missing runtime, broken install - would
        /// otherwise be reported to the user as a successful hand-off, and they
        /// would sit waiting for results that are never coming.
        /// </remarks>
        private const int StartupWatchMs = 1500;

        /// <summary>
        /// Every location searched for the helper, in order, for use both in the
        /// search itself and in the message shown when it is not found.
        /// </summary>
        public static IList<string> CandidatePaths(string configuredPath)
        {
            var candidates = new List<string>();

            if (!string.IsNullOrEmpty(configuredPath))
            {
                candidates.Add(configuredPath);
            }

            foreach (string directory in CandidateDirectories())
            {
                candidates.Add(Path.Combine(directory, ExecutableName));
            }

            return candidates;
        }

        private static IEnumerable<string> CandidateDirectories()
        {
            var directories = new List<string>();

            // Next to the plug-in itself, for a self-contained install.
            string pluginDirectory = TryGetPluginDirectory();
            if (!string.IsNullOrEmpty(pluginDirectory))
            {
                directories.Add(pluginDirectory);
                directories.Add(Path.Combine(pluginDirectory, "Server"));
            }

            foreach (string root in new[]
                     {
                         SafeFolder(Environment.SpecialFolder.LocalApplicationData),
                         SafeFolder(Environment.SpecialFolder.ProgramFiles),
                         SafeFolder(Environment.SpecialFolder.ProgramFilesX86),
                     })
            {
                if (string.IsNullOrEmpty(root))
                {
                    continue;
                }

                string installed = Path.Combine(root, "LrGeniusAI");
                directories.Add(installed);
                directories.Add(Path.Combine(installed, "Server"));

                // Per-user installers commonly land under Programs\.
                string programs = Path.Combine(Path.Combine(root, "Programs"), "LrGeniusAI");
                directories.Add(programs);
                directories.Add(Path.Combine(programs, "Server"));
            }

            return directories;
        }

        /// <summary>
        /// Returns the first candidate that exists, or null when the helper is not
        /// installed anywhere this plug-in knows to look.
        /// </summary>
        public static string Locate(string configuredPath)
        {
            foreach (string candidate in CandidatePaths(configuredPath))
            {
                try
                {
                    if (File.Exists(candidate))
                    {
                        return candidate;
                    }
                }
                catch (Exception ex) when (ex is IOException || ex is UnauthorizedAccessException)
                {
                    // An unreadable candidate is simply not a match.
                }
            }

            return null;
        }

        /// <summary>
        /// Writes the hand-off request to a temporary file the helper will read.
        /// </summary>
        /// <returns>The file path.</returns>
        /// <remarks>
        /// The request travels as a file rather than on the command line because a
        /// Windows command line tops out near 32767 characters, and a few hundred
        /// full image paths pass that comfortably. Truncation there would be silent
        /// and would drop photos without anyone noticing.
        /// </remarks>
        public static string WriteRequestFile(string requestJson)
        {
            string path = Path.Combine(
                Path.GetTempPath(),
                "lrgenius-c1-handoff-" + Guid.NewGuid().ToString("N") + ".json");
            File.WriteAllText(path, requestJson, new UTF8Encoding(false));
            return path;
        }

        /// <summary>Starts the helper against a request file.</summary>
        public static LaunchOutcome Launch(string executablePath, string requestFilePath)
        {
            var startInfo = new ProcessStartInfo(executablePath)
            {
                Arguments = "--request " + QuoteArgument(requestFilePath),
                UseShellExecute = false,
                CreateNoWindow = true,
                WorkingDirectory = Path.GetDirectoryName(executablePath) ?? string.Empty,
            };

            try
            {
                using (Process process = Process.Start(startInfo))
                {
                    if (process == null)
                    {
                        return LaunchOutcome.Failed(
                            "Windows did not start " + ExecutableName
                            + ". Reinstall LrGeniusAI, then run this again.");
                    }

                    if (process.WaitForExit(StartupWatchMs) && process.ExitCode != 0)
                    {
                        return LaunchOutcome.Failed(
                            ExecutableName + " stopped immediately with exit code " + process.ExitCode
                            + ". Run it once by hand to see what it reports, then run this again.");
                    }

                    return LaunchOutcome.Ok();
                }
            }
            catch (System.ComponentModel.Win32Exception ex)
            {
                return LaunchOutcome.Failed(
                    "Windows refused to start " + executablePath + " (" + ex.Message
                    + "). Check that the file is not blocked by SmartScreen or removed by "
                    + "virus scanning, then run this again.");
            }
            catch (Exception ex) when (ex is InvalidOperationException || ex is IOException)
            {
                return LaunchOutcome.Failed(
                    "LrGeniusAI could not be started from " + executablePath + " (" + ex.Message
                    + "). Reinstall LrGeniusAI, then run this again.");
            }
        }

        /// <summary>
        /// Quotes one argument the way the Windows C runtime parses it back.
        /// </summary>
        /// <remarks>
        /// A temporary path is unlikely to contain a quote, but "unlikely" is not a
        /// reason to hand-roll a version that breaks when it does.
        /// </remarks>
        internal static string QuoteArgument(string value)
        {
            if (string.IsNullOrEmpty(value))
            {
                return "\"\"";
            }

            var sb = new StringBuilder();
            sb.Append('"');

            int pendingBackslashes = 0;
            foreach (char c in value)
            {
                if (c == '\\')
                {
                    pendingBackslashes++;
                    continue;
                }

                if (c == '"')
                {
                    // Backslashes before a quote must be doubled, and the quote
                    // itself escaped.
                    sb.Append('\\', (pendingBackslashes * 2) + 1);
                    sb.Append('"');
                    pendingBackslashes = 0;
                    continue;
                }

                if (pendingBackslashes > 0)
                {
                    sb.Append('\\', pendingBackslashes);
                    pendingBackslashes = 0;
                }

                sb.Append(c);
            }

            // Backslashes before the closing quote must be doubled too.
            sb.Append('\\', pendingBackslashes * 2);
            sb.Append('"');
            return sb.ToString();
        }

        private static string TryGetPluginDirectory()
        {
            try
            {
                string location = Assembly.GetExecutingAssembly().Location;
                return string.IsNullOrEmpty(location) ? null : Path.GetDirectoryName(location);
            }
            catch (Exception ex) when (ex is NotSupportedException || ex is IOException)
            {
                return null;
            }
        }

        private static string SafeFolder(Environment.SpecialFolder folder)
        {
            try
            {
                return Environment.GetFolderPath(folder);
            }
            catch (Exception ex) when (ex is ArgumentException || ex is PlatformNotSupportedException)
            {
                return null;
            }
        }
    }
}
