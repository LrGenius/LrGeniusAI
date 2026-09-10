using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;

namespace LrGeniusAI.CaptureOne.Core
{
    /// <summary>
    /// A minimal, dependency-free JSON writer and reader.
    /// </summary>
    /// <remarks>
    /// <para>
    /// Why this exists instead of a framework serializer:
    /// </para>
    /// <list type="bullet">
    /// <item><description>
    /// <c>System.Text.Json</c> does not exist in .NET Framework 4.7.2, and taking
    /// it from NuGet would mean shipping several assemblies into a plug-in host we
    /// do not control the load context of.
    /// </description></item>
    /// <item><description>
    /// <c>DataContractJsonSerializer</c> needs one CLR type per JSON shape. The
    /// backend's job envelope is <c>{status, result, error, progress}</c> where
    /// <c>result</c> and <c>progress</c> are free-form (<c>serde_json::Value</c> on
    /// the Rust side, deliberately job-specific). There is no contract type to
    /// write, and any type we invented would start silently dropping fields the
    /// first time the backend added one.
    /// </description></item>
    /// <item><description>
    /// <c>JavaScriptSerializer</c> could do it, but it lives in
    /// System.Web.Extensions, and dragging the ASP.NET assembly into a photo
    /// editor's plug-in host to parse a 200-byte reply is a poor trade.
    /// </description></item>
    /// </list>
    /// <para>
    /// So: about 200 lines, no dependencies, and it throws <see cref="FormatException"/>
    /// on malformed input rather than quietly handing back a default-constructed
    /// object that the caller would report as a successful empty result.
    /// </para>
    /// <para>
    /// Parsed values are plain CLR objects: <see cref="Dictionary{TKey,TValue}"/> of
    /// <c>string</c> to <c>object</c>, <see cref="List{T}"/> of <c>object</c>,
    /// <c>string</c>, <c>double</c>, <c>bool</c>, or <c>null</c>.
    /// </para>
    /// </remarks>
    internal static class Json
    {
        // ---------------------------------------------------------------- writing

        /// <summary>
        /// Renders a string as a JSON string literal, including the surrounding
        /// quotes. Returns the literal <c>null</c> for a null input.
        /// </summary>
        public static string Quote(string value)
        {
            if (value == null)
            {
                return "null";
            }

            var sb = new StringBuilder(value.Length + 2);
            sb.Append('"');
            foreach (char c in value)
            {
                switch (c)
                {
                    case '"':
                        sb.Append("\\\"");
                        break;
                    case '\\':
                        sb.Append("\\\\");
                        break;
                    case '\b':
                        sb.Append("\\b");
                        break;
                    case '\f':
                        sb.Append("\\f");
                        break;
                    case '\n':
                        sb.Append("\\n");
                        break;
                    case '\r':
                        sb.Append("\\r");
                        break;
                    case '\t':
                        sb.Append("\\t");
                        break;
                    default:
                        if (c < ' ')
                        {
                            sb.Append("\\u").Append(((int)c).ToString("x4", CultureInfo.InvariantCulture));
                        }
                        else
                        {
                            sb.Append(c);
                        }

                        break;
                }
            }

            sb.Append('"');
            return sb.ToString();
        }

        /// <summary>
        /// True when <paramref name="value"/> can survive a round trip through UTF-8
        /// JSON.
        /// </summary>
        /// <remarks>
        /// Windows file names are UTF-16 code-unit sequences and are not required to
        /// be well-formed Unicode: a file copied off damaged media can carry an
        /// unpaired surrogate. Encoding one as UTF-8 produces bytes the backend's
        /// JSON parser rejects, which would fail the whole batch instead of the one
        /// bad file. Callers use this to skip such a path and warn about it by name.
        /// </remarks>
        public static bool IsRepresentable(string value)
        {
            if (value == null)
            {
                return false;
            }

            for (int i = 0; i < value.Length; i++)
            {
                char c = value[i];
                if (char.IsHighSurrogate(c))
                {
                    if (i + 1 >= value.Length || !char.IsLowSurrogate(value[i + 1]))
                    {
                        return false;
                    }

                    i++;
                    continue;
                }

                if (char.IsLowSurrogate(c))
                {
                    return false;
                }
            }

            return true;
        }

        // ---------------------------------------------------------------- reading

        /// <summary>Parses a complete JSON document.</summary>
        /// <exception cref="FormatException">The text is not valid JSON.</exception>
        public static object Parse(string text)
        {
            if (text == null)
            {
                throw new FormatException("empty response body");
            }

            int index = 0;
            object value = ParseValue(text, ref index);
            SkipWhitespace(text, ref index);
            if (index != text.Length)
            {
                throw new FormatException("unexpected trailing content at offset " + index.ToString(CultureInfo.InvariantCulture));
            }

            return value;
        }

        private static void SkipWhitespace(string s, ref int i)
        {
            while (i < s.Length)
            {
                char c = s[i];
                if (c == ' ' || c == '\t' || c == '\n' || c == '\r')
                {
                    i++;
                }
                else
                {
                    break;
                }
            }
        }

        private static object ParseValue(string s, ref int i)
        {
            SkipWhitespace(s, ref i);
            if (i >= s.Length)
            {
                throw new FormatException("unexpected end of input");
            }

            switch (s[i])
            {
                case '{':
                    return ParseObject(s, ref i);
                case '[':
                    return ParseArray(s, ref i);
                case '"':
                    return ParseString(s, ref i);
                case 't':
                    ExpectLiteral(s, ref i, "true");
                    return true;
                case 'f':
                    ExpectLiteral(s, ref i, "false");
                    return false;
                case 'n':
                    ExpectLiteral(s, ref i, "null");
                    return null;
                default:
                    return ParseNumber(s, ref i);
            }
        }

        private static void ExpectLiteral(string s, ref int i, string literal)
        {
            if (i + literal.Length > s.Length
                || string.CompareOrdinal(s, i, literal, 0, literal.Length) != 0)
            {
                throw new FormatException("invalid literal at offset " + i.ToString(CultureInfo.InvariantCulture));
            }

            i += literal.Length;
        }

        private static Dictionary<string, object> ParseObject(string s, ref int i)
        {
            var result = new Dictionary<string, object>(StringComparer.Ordinal);
            i++; // consume '{'
            SkipWhitespace(s, ref i);
            if (i < s.Length && s[i] == '}')
            {
                i++;
                return result;
            }

            while (true)
            {
                SkipWhitespace(s, ref i);
                if (i >= s.Length || s[i] != '"')
                {
                    throw new FormatException("expected an object key at offset " + i.ToString(CultureInfo.InvariantCulture));
                }

                string key = ParseString(s, ref i);
                SkipWhitespace(s, ref i);
                if (i >= s.Length || s[i] != ':')
                {
                    throw new FormatException("expected ':' at offset " + i.ToString(CultureInfo.InvariantCulture));
                }

                i++;
                result[key] = ParseValue(s, ref i);
                SkipWhitespace(s, ref i);
                if (i >= s.Length)
                {
                    throw new FormatException("unterminated object");
                }

                if (s[i] == ',')
                {
                    i++;
                    continue;
                }

                if (s[i] == '}')
                {
                    i++;
                    return result;
                }

                throw new FormatException("expected ',' or '}' at offset " + i.ToString(CultureInfo.InvariantCulture));
            }
        }

        private static List<object> ParseArray(string s, ref int i)
        {
            var result = new List<object>();
            i++; // consume '['
            SkipWhitespace(s, ref i);
            if (i < s.Length && s[i] == ']')
            {
                i++;
                return result;
            }

            while (true)
            {
                result.Add(ParseValue(s, ref i));
                SkipWhitespace(s, ref i);
                if (i >= s.Length)
                {
                    throw new FormatException("unterminated array");
                }

                if (s[i] == ',')
                {
                    i++;
                    continue;
                }

                if (s[i] == ']')
                {
                    i++;
                    return result;
                }

                throw new FormatException("expected ',' or ']' at offset " + i.ToString(CultureInfo.InvariantCulture));
            }
        }

        private static string ParseString(string s, ref int i)
        {
            // The caller has already checked that s[i] is the opening quote.
            i++;
            var sb = new StringBuilder();
            while (true)
            {
                if (i >= s.Length)
                {
                    throw new FormatException("unterminated string");
                }

                char c = s[i++];
                if (c == '"')
                {
                    return sb.ToString();
                }

                if (c != '\\')
                {
                    sb.Append(c);
                    continue;
                }

                if (i >= s.Length)
                {
                    throw new FormatException("unterminated escape sequence");
                }

                char escape = s[i++];
                switch (escape)
                {
                    case '"':
                        sb.Append('"');
                        break;
                    case '\\':
                        sb.Append('\\');
                        break;
                    case '/':
                        sb.Append('/');
                        break;
                    case 'b':
                        sb.Append('\b');
                        break;
                    case 'f':
                        sb.Append('\f');
                        break;
                    case 'n':
                        sb.Append('\n');
                        break;
                    case 'r':
                        sb.Append('\r');
                        break;
                    case 't':
                        sb.Append('\t');
                        break;
                    case 'u':
                        if (i + 4 > s.Length)
                        {
                            throw new FormatException("truncated \\u escape at offset " + i.ToString(CultureInfo.InvariantCulture));
                        }

                        ushort unit;
                        if (!ushort.TryParse(
                                s.Substring(i, 4),
                                NumberStyles.HexNumber,
                                CultureInfo.InvariantCulture,
                                out unit))
                        {
                            throw new FormatException("invalid \\u escape at offset " + i.ToString(CultureInfo.InvariantCulture));
                        }

                        // Appending the raw UTF-16 code unit lets a surrogate pair
                        // written as two \u escapes reassemble on its own.
                        sb.Append((char)unit);
                        i += 4;
                        break;
                    default:
                        throw new FormatException("invalid escape '\\" + escape + "' at offset " + i.ToString(CultureInfo.InvariantCulture));
                }
            }
        }

        private static double ParseNumber(string s, ref int i)
        {
            int start = i;
            while (i < s.Length)
            {
                char c = s[i];
                bool isNumeric = (c >= '0' && c <= '9')
                                 || c == '-' || c == '+' || c == '.'
                                 || c == 'e' || c == 'E';
                if (!isNumeric)
                {
                    break;
                }

                i++;
            }

            if (i == start)
            {
                throw new FormatException("expected a value at offset " + start.ToString(CultureInfo.InvariantCulture));
            }

            double value;
            if (!double.TryParse(
                    s.Substring(start, i - start),
                    NumberStyles.Float,
                    CultureInfo.InvariantCulture,
                    out value))
            {
                throw new FormatException("invalid number at offset " + start.ToString(CultureInfo.InvariantCulture));
            }

            return value;
        }

        // -------------------------------------------------------------- accessors
        // Deliberately tolerant: a missing or differently-typed member yields null
        // or an empty list. The response's shape is the backend's business, and a
        // reply that merely gained a field must not fail the run. Genuinely broken
        // JSON is caught by Parse instead.

        /// <summary>Returns a member of a JSON object, or null when absent.</summary>
        public static object Member(object node, string key)
        {
            var map = node as Dictionary<string, object>;
            object value;
            if (map != null && map.TryGetValue(key, out value))
            {
                return value;
            }

            return null;
        }

        /// <summary>Returns a string member, or null when absent or not a string.</summary>
        public static string GetString(object node, string key)
        {
            return Member(node, key) as string;
        }

        /// <summary>Returns a numeric member, or null when absent or not a number.</summary>
        public static double? GetNumber(object node, string key)
        {
            object value = Member(node, key);
            if (value is double)
            {
                return (double)value;
            }

            return null;
        }

        /// <summary>
        /// Returns the non-empty strings of an array member. Never null; an absent
        /// member yields an empty list.
        /// </summary>
        public static IList<string> GetStringArray(object node, string key)
        {
            var result = new List<string>();
            var list = Member(node, key) as List<object>;
            if (list == null)
            {
                return result;
            }

            foreach (object item in list)
            {
                var text = item as string;
                if (!string.IsNullOrEmpty(text))
                {
                    result.Add(text);
                }
            }

            return result;
        }
    }
}
