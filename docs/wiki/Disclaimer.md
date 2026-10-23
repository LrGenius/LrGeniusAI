# Disclaimer

LrGeniusAI is free software, provided **as is**, without warranty of any kind.
Using it is at your own risk. Please read this page before you let LrGeniusAI —
and especially an AI agent — work on photos you care about.

---

## No warranty

LrGeniusAI is licensed under the GNU Affero General Public License, version 3
(AGPL-3.0). As that license states in its sections 15 and 16, the program comes **without any
warranty**, to the extent permitted by law. The authors are not liable for any
damage that results from using it: lost or changed metadata, damaged catalogs,
unwanted edits, or costs charged by AI providers. The full terms are in the
[LICENSE](https://github.com/LrGenius/LrGeniusAI/blob/main/LICENSE) file.

---

## Back up your catalog

LrGeniusAI changes your Lightroom catalog. It writes keywords, titles,
captions, alt text, its own metadata fields and — with AI Edit or an AI agent —
develop settings and collections. If *Automatically write changes into XMP* is
on in Catalog Settings, Lightroom also writes these changes into XMP sidecars
and into DNG, JPEG, TIFF and PSD files.

- **Back up your catalog** before the first run and before large operations.
- Try new features on a copy of a few photos first.
- Undo, snapshots and the backups Lightroom makes help. They do not reverse
  changes that Lightroom already wrote into files or that cloud or NAS sync
  already uploaded.

---

## AI results can be wrong

Everything an AI produces can be wrong, incomplete, biased or inappropriate.
That includes keywords, titles, captions, alt text, species identifications,
face recognition, culling grades and develop edits. Check AI-generated content
before you publish it or rely on it.

- **Species identification is no expert opinion.** Never use it to decide
  whether a mushroom, plant or animal is safe to eat, touch or keep.
- **Culling grades are suggestions.** Look at photos before you delete,
  reject or archive them.

---

## Faces and personal data

Face recognition creates biometric data about the people in your photos.
Depending on where you live, processing it, and especially naming people, may
need their consent (for example under the EU's GDPR). You are responsible for
using it lawfully. LrGeniusAI keeps face data on your computer and never shares
it unless you send it to an AI provider, for example with face names in
metadata requests.

---

## Cloud AI providers

When you use a cloud AI provider (OpenAI, Google Gemini, Anthropic, or another
AI server you configure), LrGeniusAI sends the photo previews and metadata
needed for the request to that provider. What happens to them there is governed
by **the provider's terms and privacy policy**, not by LrGeniusAI. The provider
also bills you for usage. With the built-in local models, Ollama or LM Studio
on your own computer, your photos stay on your computer.

---

## AI agents (MCP)

The agent bridge lets an AI app — Claude Code, Claude Desktop, LM Studio,
Cursor or any other MCP client — work in Lightroom for you. That brings risks
of its own:

- **An agent acts on its own.** It decides which tools to use, based on how it
  understands your request. It can misunderstand you and change many photos at
  once: ratings, flags, labels, keywords, titles, captions, collections and
  develop settings. Check what it did.
- **Text in your catalog can mislead it.** Agents read captions, keywords, file
  names and other metadata, including text on photos you received from
  others. Text written to trick an AI ("prompt injection") can make an agent do
  things you did not ask for.
- **Your AI app sees what the agent sees.** Tool results go to the AI app and,
  unless it runs a local model, to the company behind it, under that company's
  terms. These results can include photo previews, metadata, file paths and the
  names of recognised people.
- **The agent token is a key to your Lightroom.** Any program on your computer
  that has it can search your catalog and, if you allow changes, change your
  photos. Keep it secret. If it may have leaked, make a new one in the Plug-in
  Manager.
- **Developer commands run code.** With them on, a connected agent can run any
  code inside Lightroom. They exist for developing the plugin. Never turn them
  on for a catalog you care about.
- **The safeguards reduce the risk; they do not remove it.** Undo,
  develop snapshots, read-only mode and the stop button on the activity bar are
  there to limit mistakes. They cannot reverse everything, for example changes
  already written to files or synced.
- **AI apps are not part of LrGeniusAI.** Their behaviour, costs and data
  handling are the responsibility of their makers.

If you only want an agent to search and look, untick *Allow agents to change
photos* in the Plug-in Manager. If you do not want agents at all, untick
*Enable the agent bridge*. See [Help: AI Agents](Help-AI-Agents).

---

## Trademarks

Adobe, Lightroom and Lightroom Classic are trademarks or registered trademarks
of Adobe Inc. in the United States and other countries. LrGeniusAI is an
independent project and is not affiliated with, endorsed by or sponsored by
Adobe. All other product and company names are trademarks of their respective
owners.
