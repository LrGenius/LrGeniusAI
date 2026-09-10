using System;
using System.Globalization;
using System.IO;
using System.Security.Cryptography;
using System.Text;

namespace LrGeniusAI.CaptureOne.Core
{
    /// <summary>
    /// Computes the host-neutral <c>file1:</c> photo identifier.
    /// </summary>
    /// <remarks>
    /// <para>
    /// The identifier is <c>"file1:" + MD5(&lt;size&gt; ":" &lt;head&gt; ":" &lt;tail&gt;)</c>, where
    /// <c>&lt;size&gt;</c> is the file length in bytes as ASCII decimal digits,
    /// <c>&lt;head&gt;</c> is the first 4 MiB of the file and <c>&lt;tail&gt;</c> the last 4 MiB.
    /// Files shorter than 4 MiB are covered twice, once as head and once as tail;
    /// that is intentional and deterministic, not a bug to "fix" later.
    /// </para>
    /// <para>
    /// The backend treats <c>photo_id</c> as an opaque string and derives nothing
    /// from it, so the only requirement is that the same file always produces the
    /// same id on every host.
    /// </para>
    /// <para>
    /// <b>Deliberately no modification time.</b> The Lightroom plug-in's fallback
    /// scheme (<c>md5p:&lt;size&gt;:&lt;mtime&gt;:...</c>) mixes mtime in, which makes the id
    /// change when a file is copied to another volume or restored from a backup.
    /// Content plus length is stable across all of that.
    /// </para>
    /// <para>
    /// <b>Open decision, deliberately not implemented here.</b> Lightroom's primary
    /// scheme is <c>meta1:</c>, an MD5 over eleven values that include four
    /// *formatted* Lightroom display strings ("1/125 s", "f/2.8", "70 mm",
    /// "ISO 400"). Capture One cannot reproduce those strings character for
    /// character, so a <c>file1:</c> id will never collide with an existing
    /// Lightroom index entry for the same photo. Whether the two namespaces should
    /// later be reconciled - and if so, in which direction - is an open product
    /// decision. This plug-in does not attempt it: it only ever emits
    /// <c>file1:</c> ids, and never rewrites or matches against <c>meta1:</c> ones.
    /// </para>
    /// <para>
    /// Two distinct files sharing a length, a first 4 MiB and a last 4 MiB would
    /// collide. For camera originals that does not happen; for synthetic test files
    /// padded in the middle it can.
    /// </para>
    /// </remarks>
    internal static class PhotoId
    {
        /// <summary>Namespace prefix identifying the algorithm.</summary>
        public const string Prefix = "file1:";

        /// <summary>Name of the algorithm, as sent to the backend.</summary>
        public const string AlgorithmName = "file1";

        private const int ChunkBytes = 4 * 1024 * 1024;
        private const int CopyBufferBytes = 64 * 1024;

        /// <summary>
        /// Checks once, up front, whether MD5 can be used at all.
        /// </summary>
        /// <returns>
        /// Null when MD5 is available, otherwise the platform's reason.
        /// </returns>
        /// <remarks>
        /// Windows machines with the "System cryptography: Use FIPS compliant
        /// algorithms" policy enabled refuse to hand out MD5. That would otherwise
        /// fail on the very first photo and again on every one after it, so the
        /// caller probes once and reports a single actionable message instead of
        /// one failure per file.
        /// </remarks>
        public static string ProbeUnavailableReason()
        {
            try
            {
                using (MD5.Create())
                {
                    return null;
                }
            }
            catch (Exception ex)
            {
                // Intentionally broad: this is a capability probe, and every
                // failure mode here means the same thing to the caller.
                return ex.Message;
            }
        }

        /// <summary>Computes the <c>file1:</c> identifier for a file on disk.</summary>
        /// <exception cref="IOException">The file could not be read in full.</exception>
        /// <exception cref="UnauthorizedAccessException">Access was denied.</exception>
        public static string Compute(string path)
        {
            // FileShare.ReadWrite | FileShare.Delete: Capture One very likely has
            // the file open already, and a plug-in must not be the reason a read
            // fails. We only ever read.
            using (var md5 = MD5.Create())
            using (var stream = new FileStream(
                       path,
                       FileMode.Open,
                       FileAccess.Read,
                       FileShare.ReadWrite | FileShare.Delete,
                       CopyBufferBytes,
                       FileOptions.SequentialScan))
            {
                long size = stream.Length;

                AbsorbAscii(md5, size.ToString(CultureInfo.InvariantCulture));
                AbsorbAscii(md5, ":");

                int headLength = (int)Math.Min(ChunkBytes, size);
                AbsorbRange(stream, md5, 0, headLength, path);

                AbsorbAscii(md5, ":");

                long tailStart = Math.Max(0, size - ChunkBytes);
                int tailLength = (int)Math.Min(ChunkBytes, size);
                AbsorbRange(stream, md5, tailStart, tailLength, path);

                md5.TransformFinalBlock(Array.Empty<byte>(), 0, 0);
                return Prefix + ToHex(md5.Hash);
            }
        }

        private static void AbsorbAscii(HashAlgorithm hash, string text)
        {
            byte[] bytes = Encoding.ASCII.GetBytes(text);
            hash.TransformBlock(bytes, 0, bytes.Length, null, 0);
        }

        private static void AbsorbRange(FileStream stream, HashAlgorithm hash, long offset, int count, string path)
        {
            if (count <= 0)
            {
                return;
            }

            stream.Seek(offset, SeekOrigin.Begin);
            var buffer = new byte[CopyBufferBytes];
            int remaining = count;
            while (remaining > 0)
            {
                int wanted = Math.Min(buffer.Length, remaining);
                int read = stream.Read(buffer, 0, wanted);
                if (read <= 0)
                {
                    // The file shrank while we were reading it. Hashing the short
                    // read would mint an id that never reproduces, so refuse
                    // instead and let the caller skip this one photo by name.
                    throw new IOException(
                        "the file ended earlier than its own length promised - it is probably still being written");
                }

                hash.TransformBlock(buffer, 0, read, null, 0);
                remaining -= read;
            }
        }

        private static string ToHex(byte[] bytes)
        {
            var sb = new StringBuilder(bytes.Length * 2);
            foreach (byte b in bytes)
            {
                sb.Append(b.ToString("x2", CultureInfo.InvariantCulture));
            }

            return sb.ToString();
        }
    }
}
