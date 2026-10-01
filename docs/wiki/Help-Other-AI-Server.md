# Help: Other AI Server

**Other AI server** lets LrGeniusAI use any server that speaks the OpenAI
chat API. You only need it if the AI should run somewhere else than on this
computer — for example:

- **[OpenRouter](https://openrouter.ai)** — one key for hundreds of cloud
  models, including free ones.
- **llama.cpp's `llama-server`**, **vLLM** or **LiteLLM** on your own machine
  or network.
- **LM Studio** or **Ollama** running on *another* computer (a desktop with a
  big GPU, a NAS, …).

You do **not** need it for:

- the built-in local model (*AI model (on this computer)* in Plug-in Manager —
  see [Local AI Models](Help-Local-AI-Models));
- Ollama or LM Studio running on **this** computer — LrGeniusAI finds them
  automatically at their default address;
- OpenAI, Google Gemini and Anthropic (Claude) — they have their own key
  fields.

**Anthropic does not work here.** Anthropic offers an OpenAI-compatible
address (`https://api.anthropic.com/v1`), but it only answers chat requests:
the model list behind it is Anthropic's own and fails with *"HTTP 400
(anthropic-version: header is required)"*, and it ignores the answer format
LrGeniusAI asks for. Enter your key in the **Anthropic key** field instead —
or, if you want Claude through OpenRouter, use OpenRouter here.

## Setting it up

*File → Plug-in Manager → LrGeniusAI → Optional AI providers*:

1. **Other AI server** — the server's address.
2. **API key** — only if the server needs one. Leave it empty otherwise.

The line under the two fields checks the server as soon as you leave a field:
it shows how many of the server's models can read photos
(`OpenRouter: 212 models that can read photos`), or what is wrong. The models
then appear in every task's **AI Model** list under the server's name —
`OpenRouter · google/gemini-2.5-flash`, or the host and port for your own
server (`192.168.1.20:8080 · gemma-3-12b-it`).

Only models that can take a photo are listed: text-only and embedding models
are left out where the server says which is which.

## Addresses

Paste the address the server's documentation gives. LrGeniusAI is lenient
about the form:

| You enter | Used as |
|---|---|
| `https://openrouter.ai/api/v1` | as is |
| `openrouter.ai/api/v1` | `https://openrouter.ai/api/v1` — no scheme means https for internet addresses… |
| `192.168.1.20:8080` | `http://192.168.1.20:8080/v1` — …and http on your network or with a port; no path means `/v1` |
| `http://gpu-box.local:1234/v1/chat/completions` | `http://gpu-box.local:1234/v1` — a pasted endpoint is cut back |

Examples:

| Server | Address | API key |
|---|---|---|
| OpenRouter | `https://openrouter.ai/api/v1` | your OpenRouter key |
| llama.cpp `llama-server` | `http://<host>:8080` | only if started with `--api-key` |
| LiteLLM proxy | `http://<host>:4000` | the proxy key, if a master key is set |
| vLLM | `http://<host>:8000` | only if started with `--api-key` |
| LM Studio on another computer | `http://<host>:1234` | only if its server requires a token |
| Ollama on another computer | `http://<host>:11434` | none |

## Things to know

- **The server gets your photos.** Every photo you analyse is sent to it,
  together with the context you chose to send (keywords, GPS, folder names).
  For a cloud service like OpenRouter that means the photo leaves your
  computer, and the provider's terms apply.
- **Costs.** Paid models on OpenRouter cost credits per photo, like OpenAI or
  Gemini. OpenRouter's `:free` models cost nothing but are rate limited.
- **One photo at a time.** Requests to your own server are sent one after the
  other, as for Ollama and LM Studio: most such servers run a single model
  that would process them one by one anyway.
- **`llama-server` needs its vision projector.** Start it with `--mmproj`
  (or `-hf`, which downloads it), or every photo fails with "cannot read
  photos".
- **Ollama on another computer** is reached through its OpenAI-compatible API,
  which cannot set the context size per request. If long prompts get cut off,
  raise it on that computer, e.g. `OLLAMA_CONTEXT_LENGTH=8192 ollama serve`.
- **Answer format.** LrGeniusAI asks the server to hold the model to a JSON
  schema. A server that cannot do that is asked again with a looser format
  (it remembers this per model, so only the first photo pays for the extra
  request), and the photos answered that way carry a warning saying so. The
  answers are still checked, but a weaker model may leave a field out more
  often — that shows up as a warning on the photo, as with any other provider.

## When it does not work

The status line and the task's error dialog say what went wrong, in words:

| Message | What to do |
|---|---|
| *Could not reach …* | Check the address and that the server is running and reachable from this computer (firewall, VPN). |
| *… rejected the API key* / *… needs an API key* | Check the key; copy it again from the provider's page. |
| *… has no OpenAI-compatible API at this address* | The address usually ends in `/v1` — check the server's documentation. |
| *… does not know the model …* | The model was removed or renamed; pick another in the AI Model list. |
| *… cannot read photos* | Pick a vision model; for `llama-server`, add `--mmproj`. |
| *… is rate limiting requests* | Wait a little, or use a model without a free-tier limit. |
| *… no credit left on the account* | Top up the account with the provider. |

## Moved from the old Ollama / LM Studio fields

Earlier versions had separate address fields for Ollama and LM Studio. If you
had changed one of them (the app ran on another computer), that address was
moved here once, and your model choice with it; the Plug-in Manager shows a
note the first time it opens. If you had changed *both*, only the one you
were using was moved — enter the other here if you need it instead.
