using System;
using System.Globalization;
using System.IO;
using System.Net;
using System.Text;

namespace LrGeniusAI.CaptureOne.Core
{
    /// <summary>The outcome of one HTTP round trip.</summary>
    /// <remarks>
    /// A transport failure and an HTTP error are different things to the user - one
    /// says "start the backend", the other says "the backend refused this" - so
    /// they are kept apart rather than flattened into one exception.
    /// </remarks>
    internal sealed class HttpOutcome
    {
        /// <summary>HTTP status, or 0 when the request never reached the server.</summary>
        public int StatusCode { get; private set; }

        /// <summary>Response body, or null after a transport failure.</summary>
        public string Body { get; private set; }

        /// <summary>
        /// Plain-language reason the connection failed, or null when it did not.
        /// Phrased to slot into a sentence, e.g. "nothing is listening there".
        /// </summary>
        public string TransportError { get; private set; }

        /// <summary>True for 2xx.</summary>
        public bool IsSuccess
        {
            get { return StatusCode >= 200 && StatusCode < 300; }
        }

        /// <summary>True when the server was never reached.</summary>
        public bool IsTransportFailure
        {
            get { return TransportError != null; }
        }

        public static HttpOutcome Http(int statusCode, string body)
        {
            return new HttpOutcome { StatusCode = statusCode, Body = body };
        }

        public static HttpOutcome Transport(string reason)
        {
            return new HttpOutcome { StatusCode = 0, TransportError = reason };
        }
    }

    /// <summary>
    /// Talks to the local LrGeniusAI backend.
    /// </summary>
    /// <remarks>
    /// <para>
    /// <b>Deliberately synchronous, and deliberately <see cref="HttpWebRequest"/>.</b>
    /// Capture One hosts plug-ins out of process and we do not know what - if
    /// anything - it installs as a synchronization context on the thread that calls
    /// us. Blocking on a Task in that situation is the classic way to deadlock a
    /// plug-in host, and going async all the way would mean guessing at the SDK's
    /// threading contract as well as its signatures. Straight-line blocking calls
    /// with explicit timeouts have neither problem.
    /// </para>
    /// <para>
    /// Cancellation is therefore cooperative: the caller checks between requests,
    /// and the timeouts below bound how long any single request can delay that
    /// check.
    /// </para>
    /// </remarks>
    internal sealed class HandoffClient
    {
        /// <summary>Where the backend listens unless configured otherwise.</summary>
        public const string DefaultBaseUrl = "http://127.0.0.1:19819";

        /// <summary>
        /// The hand-off endpoint.
        /// </summary>
        /// <remarks>
        /// <b>This endpoint does not exist yet.</b> As of this writing the backend
        /// exposes no <c>/v1/host/handoff</c> route - the shape below is this
        /// plug-in's proposal, documented in README.md, and a 404 from it is an
        /// expected answer today rather than a malfunction. The caller treats 404
        /// as "this backend predates the endpoint" and falls back to the helper
        /// executable, so the plug-in is useful before the backend catches up.
        /// No backend change is made by this component.
        /// </remarks>
        public const string HandoffPath = "/v1/host/handoff";

        private const int PingTimeoutMs = 2000;
        private const int HandoffTimeoutMs = 30000;
        private const int PollTimeoutMs = 15000;

        private readonly string _baseUrl;

        public HandoffClient(string baseUrl)
        {
            _baseUrl = string.IsNullOrEmpty(baseUrl)
                ? DefaultBaseUrl
                : baseUrl.TrimEnd('/');
        }

        /// <summary>The base URL actually in use, for messages.</summary>
        public string BaseUrl
        {
            get { return _baseUrl; }
        }

        /// <summary>
        /// Cheap liveness probe against <c>GET /ping</c>, which answers with the
        /// plain text "pong".
        /// </summary>
        /// <remarks>
        /// Worth its round trip: hashing a few hundred RAW files takes real time,
        /// and finding out only afterwards that neither the backend nor the helper
        /// is available would waste all of it.
        /// </remarks>
        public bool IsBackendAlive()
        {
            HttpOutcome outcome = Send("GET", "/ping", null, PingTimeoutMs);
            return outcome.IsSuccess
                   && outcome.Body != null
                   && outcome.Body.IndexOf("pong", StringComparison.OrdinalIgnoreCase) >= 0;
        }

        /// <summary>Posts the hand-off request.</summary>
        public HttpOutcome PostHandoff(string requestJson)
        {
            return Send("POST", HandoffPath, requestJson, HandoffTimeoutMs);
        }

        /// <summary>
        /// Reads a job's status once.
        /// </summary>
        /// <remarks>
        /// <b>This read is destructive.</b> The backend hands a finished job's
        /// result back exactly once and then drops it, and an untouched job expires
        /// after 600 seconds. So a caller must consume everything it needs from the
        /// response that first reports <c>done</c> or <c>error</c> - there is no
        /// second chance - and two pollers must never share a job id.
        /// </remarks>
        public HttpOutcome PollJob(string jobId)
        {
            return Send("GET", "/v1/jobs/" + Uri.EscapeDataString(jobId), null, PollTimeoutMs);
        }

        private HttpOutcome Send(string method, string path, string jsonBody, int timeoutMs)
        {
            HttpWebRequest request;
            try
            {
                request = (HttpWebRequest)WebRequest.Create(_baseUrl + path);
            }
            catch (Exception ex) when (ex is UriFormatException
                                       || ex is NotSupportedException
                                       || ex is InvalidCastException
                                       || ex is ArgumentException)
            {
                return HttpOutcome.Transport(
                    "\"" + _baseUrl + "\" is not a usable http:// address (" + ex.Message + ")");
            }

            request.Method = method;
            request.Timeout = timeoutMs;
            request.ReadWriteTimeout = timeoutMs;
            request.KeepAlive = false;
            request.Accept = "application/json";
            request.UserAgent = PluginInfo.UserAgent;

            // A configured system or corporate proxy must never be consulted for
            // 127.0.0.1. Leaving this at the default is a well-known way to make a
            // localhost call fail on a managed Windows machine.
            request.Proxy = null;

            try
            {
                if (jsonBody != null)
                {
                    byte[] payload = new UTF8Encoding(false).GetBytes(jsonBody);
                    request.ContentType = "application/json; charset=utf-8";
                    request.ContentLength = payload.Length;
                    using (Stream body = request.GetRequestStream())
                    {
                        body.Write(payload, 0, payload.Length);
                    }
                }

                using (var response = (HttpWebResponse)request.GetResponse())
                {
                    return HttpOutcome.Http((int)response.StatusCode, ReadBody(response));
                }
            }
            catch (WebException ex)
            {
                // A 4xx/5xx arrives here as an exception carrying the response.
                // That is an answer from the server, not a transport failure.
                var response = ex.Response as HttpWebResponse;
                if (response != null)
                {
                    using (response)
                    {
                        return HttpOutcome.Http((int)response.StatusCode, ReadBody(response));
                    }
                }

                return HttpOutcome.Transport(DescribeTransportFailure(ex, timeoutMs));
            }
            catch (IOException ex)
            {
                return HttpOutcome.Transport("the connection dropped (" + ex.Message + ")");
            }
        }

        private static string ReadBody(HttpWebResponse response)
        {
            try
            {
                using (Stream stream = response.GetResponseStream())
                {
                    if (stream == null)
                    {
                        return string.Empty;
                    }

                    using (var reader = new StreamReader(stream, Encoding.UTF8))
                    {
                        return reader.ReadToEnd();
                    }
                }
            }
            catch (IOException)
            {
                // The status code is the useful part; a truncated body is not worth
                // turning a real answer into a transport failure.
                return string.Empty;
            }
        }

        private static string DescribeTransportFailure(WebException ex, int timeoutMs)
        {
            switch (ex.Status)
            {
                case WebExceptionStatus.ConnectFailure:
                case WebExceptionStatus.NameResolutionFailure:
                case WebExceptionStatus.ProxyNameResolutionFailure:
                    return "nothing is listening there";
                case WebExceptionStatus.Timeout:
                    return "it did not answer within "
                           + (timeoutMs / 1000).ToString(CultureInfo.InvariantCulture)
                           + " seconds";
                case WebExceptionStatus.ConnectionClosed:
                case WebExceptionStatus.KeepAliveFailure:
                    return "the connection was closed mid-request";
                default:
                    return ex.Message;
            }
        }
    }
}
