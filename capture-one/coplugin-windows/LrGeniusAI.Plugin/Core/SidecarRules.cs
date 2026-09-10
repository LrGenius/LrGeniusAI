using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;

namespace LrGeniusAI.CaptureOne.Core
{
    /// <summary>
    /// Everything this plug-in knows about XMP sidecar files.
    /// </summary>
    /// <remarks>
    /// <para>
    /// Windows has no Capture One scripting bridge - no AppleScript equivalent, no
    /// COM server, no command line. The only way anything computed by LrGeniusAI
    /// can get back into Capture One on Windows is an XMP sidecar that the user
    /// then loads. That carries keywords (<c>dc:subject</c>), keyword hierarchy
    /// (<c>lr:hierarchicalSubject</c>), rating, colour label, title, description and
    /// the IPTC core fields - and nothing else. Albums, selections and develop
    /// adjustments cannot travel this way at all.
    /// </para>
    /// <para>
    /// Since the round trip depends on a preference the user has probably never
    /// touched, every successful run says so explicitly.
    /// </para>
    /// </remarks>
    internal static class SidecarRules
    {
        /// <summary>
        /// Closing instruction appended to every run that wrote, or will write,
        /// sidecars. Menu names are the English Capture One ones.
        /// </summary>
        public const string LoadMetadataInstruction =
            "Results are written to XMP sidecar files next to your images. Capture One does not "
            + "pick them up on its own: switch on Edit > Preferences > Image > \"Auto Sync Sidecar XMP\" "
            + "and set it to \"Load\", or select the images and run Image > \"Load Metadata\" after "
            + "each run. Keywords, rating, colour tag, title, description and the IPTC fields come "
            + "back this way; albums and develop adjustments do not.";

        /// <summary>
        /// The sidecar path for an image, following the Adobe convention Capture
        /// One expects: the original extension is replaced by <c>.XMP</c> rather
        /// than appended to it.
        /// </summary>
        public static string SidecarPathFor(string imagePath)
        {
            if (string.IsNullOrEmpty(imagePath))
            {
                return null;
            }

            string directory = Path.GetDirectoryName(imagePath);
            string stem = Path.GetFileNameWithoutExtension(imagePath);
            if (string.IsNullOrEmpty(stem))
            {
                return null;
            }

            return string.IsNullOrEmpty(directory)
                ? stem + ".XMP"
                : Path.Combine(directory, stem + ".XMP");
        }

        /// <summary>
        /// Finds selections in which several images would share one sidecar file.
        /// </summary>
        /// <returns>One ready-to-show message per colliding group; empty when clean.</returns>
        /// <remarks>
        /// Because the extension is replaced rather than appended, a RAW+JPEG pair
        /// shot by the same camera - <c>IMG_0001.CR2</c> and <c>IMG_0001.JPG</c> -
        /// maps onto the single file <c>IMG_0001.XMP</c>. Whatever is written last
        /// wins, and the user would silently get one variant's keywords on both.
        /// Detecting this is cheap and entirely local, so it happens before
        /// anything is sent anywhere.
        /// </remarks>
        public static IList<string> DescribeCollisions(IEnumerable<string> imagePaths)
        {
            var groups = new Dictionary<string, List<string>>(StringComparer.OrdinalIgnoreCase);
            var order = new List<string>();

            foreach (string path in imagePaths)
            {
                if (string.IsNullOrEmpty(path))
                {
                    continue;
                }

                string directory = Path.GetDirectoryName(path) ?? string.Empty;
                string stem = Path.GetFileNameWithoutExtension(path) ?? string.Empty;

                // NUL cannot occur in a Windows path, so it is a safe separator
                // between the two halves of the key.
                string key = directory + "\0" + stem;

                List<string> bucket;
                if (!groups.TryGetValue(key, out bucket))
                {
                    bucket = new List<string>();
                    groups.Add(key, bucket);
                    order.Add(key);
                }

                bucket.Add(path);
            }

            var messages = new List<string>();
            foreach (string key in order)
            {
                List<string> bucket = groups[key];
                if (bucket.Count < 2)
                {
                    continue;
                }

                var names = new List<string>(bucket.Count);
                foreach (string path in bucket)
                {
                    names.Add(Path.GetFileName(path));
                }

                names.Sort(StringComparer.OrdinalIgnoreCase);
                string stem = Path.GetFileNameWithoutExtension(bucket[0]);

                messages.Add(string.Format(
                    CultureInfo.InvariantCulture,
                    "{0} all map onto the single sidecar file {1}.XMP, so only the last one written "
                    + "would survive. Take all but one of them out of the selection, or run them in "
                    + "separate batches.",
                    string.Join(", ", names.ToArray()),
                    stem));
            }

            return messages;
        }
    }
}
