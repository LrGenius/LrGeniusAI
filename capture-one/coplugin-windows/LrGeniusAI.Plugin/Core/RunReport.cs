using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;

namespace LrGeniusAI.CaptureOne.Core
{
    /// <summary>
    /// Collects everything that went wrong, or went less than fully right, during
    /// one run and renders it into the single string Capture One will show.
    /// </summary>
    /// <remarks>
    /// <para>
    /// This exists because the plug-in host gives us exactly one message per action
    /// result. A log line is not a report: if a problem does not end up in the text
    /// this class produces, the user never learns about it.
    /// </para>
    /// <para>
    /// The rules it implements:
    /// </para>
    /// <list type="bullet">
    /// <item><description>
    /// Warnings accumulate in a list, never in a single field - one slot means the
    /// second problem erases the first.
    /// </description></item>
    /// <item><description>
    /// A run-wide cause is reported once with a count and a few example file names,
    /// not once per photo and not silently dropped.
    /// </description></item>
    /// <item><description>
    /// A degraded success is not a success: the caller reports warnings even when
    /// the run as a whole worked.
    /// </description></item>
    /// </list>
    /// </remarks>
    internal sealed class RunReport
    {
        /// <summary>How many distinct problems or warnings are spelled out.</summary>
        public const int MaxItemsShown = 5;

        /// <summary>How many example file names accompany a grouped warning.</summary>
        public const int MaxExamplesPerWarning = 3;

        private static readonly string NL = Environment.NewLine;

        private readonly List<string> _errors = new List<string>();
        private readonly List<string> _warningOrder = new List<string>();
        private readonly Dictionary<string, WarningGroup> _warnings =
            new Dictionary<string, WarningGroup>(StringComparer.Ordinal);

        /// <summary>True when at least one hard failure was recorded.</summary>
        public bool HasErrors
        {
            get { return _errors.Count > 0; }
        }

        /// <summary>True when at least one degradation was recorded.</summary>
        public bool HasWarnings
        {
            get { return _warningOrder.Count > 0; }
        }

        /// <summary>
        /// Records a hard failure. Phrase it as what the user should do, not as
        /// what the code observed.
        /// </summary>
        public void AddError(string message)
        {
            if (string.IsNullOrEmpty(message))
            {
                return;
            }

            if (!_errors.Contains(message))
            {
                _errors.Add(message);
            }
        }

        /// <summary>Records a degradation with no particular file attached.</summary>
        public void AddWarning(string reason)
        {
            AddWarning(reason, null);
        }

        /// <summary>
        /// Records a degradation caused by one file. Identical reasons are folded
        /// into a single line carrying a count and up to
        /// <see cref="MaxExamplesPerWarning"/> example names.
        /// </summary>
        public void AddWarning(string reason, string exampleFileName)
        {
            if (string.IsNullOrEmpty(reason))
            {
                return;
            }

            WarningGroup group;
            if (!_warnings.TryGetValue(reason, out group))
            {
                group = new WarningGroup(reason);
                _warnings.Add(reason, group);
                _warningOrder.Add(reason);
            }

            group.Add(exampleFileName);
        }

        /// <summary>
        /// Renders the message shown in Capture One.
        /// </summary>
        /// <param name="headline">One sentence saying how the run ended.</param>
        /// <param name="footer">
        /// Optional closing instruction, e.g. how to load the sidecars back in.
        /// </param>
        public string Compose(string headline, string footer = null)
        {
            var sb = new StringBuilder();
            sb.Append(headline);

            if (_errors.Count > 0)
            {
                sb.Append(NL).Append(NL).Append("What went wrong:");
                AppendCapped(sb, _errors, _errors.Count, "problem");
            }

            if (_warningOrder.Count > 0)
            {
                var rendered = new List<string>(_warningOrder.Count);
                foreach (string reason in _warningOrder)
                {
                    rendered.Add(_warnings[reason].Render());
                }

                sb.Append(NL).Append(NL).Append("Worth knowing:");
                AppendCapped(sb, rendered, rendered.Count, "warning");
            }

            if (!string.IsNullOrEmpty(footer))
            {
                sb.Append(NL).Append(NL).Append(footer);
            }

            return sb.ToString();
        }

        private static void AppendCapped(StringBuilder sb, IList<string> items, int total, string noun)
        {
            int shown = 0;
            foreach (string item in items)
            {
                if (shown == MaxItemsShown)
                {
                    break;
                }

                sb.Append(NL).Append("  - ").Append(item);
                shown++;
            }

            int hidden = total - shown;
            if (hidden > 0)
            {
                sb.Append(NL)
                  .Append("  - and ")
                  .Append(hidden.ToString(CultureInfo.InvariantCulture))
                  .Append(" more ")
                  .Append(noun)
                  .Append(hidden == 1 ? "." : "s.");
            }
        }

        private sealed class WarningGroup
        {
            private readonly string _reason;
            private readonly List<string> _examples = new List<string>();
            private int _count;

            public WarningGroup(string reason)
            {
                _reason = reason;
            }

            public void Add(string exampleFileName)
            {
                _count++;
                if (!string.IsNullOrEmpty(exampleFileName) && _examples.Count < MaxExamplesPerWarning)
                {
                    _examples.Add(exampleFileName);
                }
            }

            public string Render()
            {
                if (_examples.Count == 0)
                {
                    return _count > 1
                        ? _reason + " (" + _count.ToString(CultureInfo.InvariantCulture) + " times)"
                        : _reason;
                }

                var sb = new StringBuilder(_reason);
                sb.Append(" (");
                if (_count > 1)
                {
                    sb.Append(_count.ToString(CultureInfo.InvariantCulture)).Append(" files: ");
                }

                sb.Append(string.Join(", ", _examples.ToArray()));
                if (_count > _examples.Count)
                {
                    sb.Append(", ...");
                }

                sb.Append(")");
                return sb.ToString();
            }
        }
    }
}
