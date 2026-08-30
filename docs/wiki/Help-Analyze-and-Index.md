# Help: Analyze and Index

This is the task that does the actual AI work: it sends your photos to a model,
gets keywords, titles, captions and alt text back, and builds the search index
the other features rely on.

## Start the task

In Lightroom Classic:

- `Library → Plug-in Extras → Analyze & Index Photos...`

The dialog has three tabs.

---

## General

### Scope

- **Selected photos only**
- **Current view** — the folder or collection currently open
- **All photos in catalog**
- **New or unprocessed photos** — only photos the backend has no data for

### AI Model

Picks the provider and model, plus **Temperature**, **Max Tokens**, output
**Language**, and **Replace ß with ss** for German output. Which providers
appear depends on what is configured in the Plug-in Manager — see
[Choosing an AI Model](Help-Choosing-AI-Model).

### Primary Tasks

- **Generate AI metadata (Keywords, Title, Caption)** — the LLM pass. Which of
  the four fields are actually produced is set on the *Metadata Options* tab.
- **Enable smart photo search** — computes the SigLIP embedding that
  [Advanced Search](Help-Advanced-Search) and
  [Find Similar](Help-Find-Similar) need. Greyed out until the search model has
  been downloaded in the Plug-in Manager.
- **Enable face detection** — detects and embeds faces for
  [People](Help-People-Faces).
- **Identify animal and plant species** — runs BioCLIP 2 and writes a
  taxonomic identification. Greyed out until the models have been downloaded
  in the Plug-in Manager, under *On-device AI models* — one **Download AI
  models** button fetches everything that is missing, so there is nothing to
  pick per feature. Two sub-options:
  - *Only where an animal or plant is detected* (on by default) — checks the
    already-computed search embedding first and skips photos with no organism
    in them. BioCLIP is a large model and most of a general photo library is
    portraits, architecture and landscapes, so leaving this on is typically the
    difference between minutes and hours. Turn it off if you know the selection
    is all wildlife.
  - *Also write the taxonomy as keywords* (off by default) — see below.

### Turning off AI metadata makes the rest of the run faster

Unchecking **Generate AI metadata** does more than skip the language model.
With no model in the loop there is nothing that has to see one photo at a time,
so — when *Submit original files* is on in the Plug-in Manager and the backend
runs on this machine — the plugin hands over a group of photos per request and
the backend reads, decodes and measures them across several CPU cores. See
[Performance Tips](Help-Performance-Tips) §5.

That makes "embeddings and faces now, metadata later" a genuinely cheaper way
to work through a large import than doing everything in one pass.

### If a model is not downloaded yet

Pressing **Start** checks that the on-device models the selected tasks need are
actually on disk, and asks before spending the indexing time if one is not:

- **Download now** — starts the download (it reports its own progress) and
  stops this run. Start it again once the download finishes and the photos are
  processed with everything you selected.
- **Continue without it** — indexes anyway. The photos are processed without
  that model's contribution, and the completion dialog reports it as a warning
  naming the **Download AI models** button. Running Analyze & Index over the
  same photos later fills in what was skipped.
- **Cancel** — nothing runs.

Only the models the run actually needs are checked: a search-only run is not
stopped by a missing species model, and tasks that run on an LLM or in the
cloud need nothing downloaded. If the check itself cannot reach the backend the
run proceeds, and any real failure is reported as usual.

---

## Species identification

Everything runs on your machine; nothing is uploaded.

### What gets written

Five fields in the *LrGeniusAI* metadata panel, on every photo that was
checked:

| Field | Example |
|---|---|
| Species (common name) | `Great Tit` |
| Species (scientific name) | `Parus major` |
| Species rank | `species`, `genus`, … or `none` |
| Species confidence | `0.91` |
| Species taxonomy | `Animalia>Chordata>Aves>Passeriformes>Paridae>Parus>Parus major` |

All five are searchable, so you can build a Smart Collection on e.g.
`Species rank` is `species`, or on a family name in `Species taxonomy`.

### Looking the species up on the web

Two more fields — **Species on iNaturalist** and **Species on Wikipedia** —
hold a link, and Lightroom draws them with a button that opens the page in your
browser. iNaturalist is the one to click when you want to check the
identification against photos, range maps and similar-looking species;
Wikipedia is the one for what the organism actually is.

The links point at the taxon's own page, not at a search: the backend resolves
the scientific name against [GBIF](https://www.gbif.org) and
[iNaturalist](https://www.inaturalist.org) — both free, and no account is
needed — and remembers the answer, so each taxon is looked up once per machine
and never again. If the lookup cannot be made (no internet at the time, or a
name neither database carries), the fields hold a search link on the same site
instead, which lands you one click away.

*Look-up links in* picks the Wikipedia edition and the language iNaturalist
reports common names in. **Automatic** follows Lightroom's own interface
language. The identification itself is language-independent — this only changes
which article opens.

Both fields are filled in as photos are analyzed, so photos identified before
this feature existed have them empty; re-running *Analyze & Index* over them,
or *Retrieve Metadata from Backend*, fills them in.

### The AI model is told what the classifier found

When species identification and AI metadata run together, the identification
is put into the prompt before the LLM writes anything, and the prompt says to
use it. A general vision model is confident and often wrong about animals and
plants — a photo of two rabbits came back titled *"Sheep in Green Pasture"*
while the species field on the same photo read *European Rabbit* — and BioCLIP
is a specialist trained on the tree of life, so where the two disagree the
classifier wins.

A coarse answer is passed on as a limit rather than a name: told *family:
Leporidae*, the model is asked to stay at that level instead of inventing a
species. Photos with no identification are unaffected, and a photo identified
by an earlier run contributes its stored answer to a later metadata-only run.

This needs no separate switch — ticking *Identify animal and plant species*
is what turns it on.

### Why the answer is sometimes just "Aves"

The model reports the **deepest rank it is actually confident about**. A clear
frame of a garden bird gets a species; a distant silhouette gets an order or a
class. That is deliberate — a coarse answer that is right is more useful than a
binomial that is wrong, and the two are easy to tell apart because the rank is
written alongside the name.

Photos where nothing cleared the confidence floor (including everything the
pre-filter skipped) get `Species rank = none` and empty name fields. They still
count as checked and are not re-examined on the next run.

### Common names are not always in English

The common name comes from GBIF's vernacular-name data, which has no reliable
per-language coverage. Where no English name exists, whatever language GBIF
lists first is used — so a run over European wildlife will produce Swedish and
Danish names among the English ones (`honungslök` for *Allium siculum*,
`Almindelig kuglebærerflue` for a fly). This is upstream data, not a setting.

**The scientific name is always correct and always present**, so use *Species
(scientific name)* when you need something dependable, and treat *Species
(common name)* as a convenience. The keyword branch below has the same
property: the leaf uses the common name when there is one, with the scientific
name attached as a synonym, so a search for the binomial always finds the photo.

### Keywords

With *Also write the taxonomy as keywords* on, the identification is
additionally written as a keyword hierarchy under a **`Species`** root,
separate from the `LrGeniusAI` root the AI keywords use:

```
Species > Animalia > Chordata > Aves > Passeriformes > Paridae > Parus > Great Tit
```

Only the leaf is marked for export, so a JPEG exported from Lightroom carries
`Great Tit` in its IPTC keywords and not the six ranks above it. The scientific
name is stored as a keyword synonym, so searching for `Parus major` finds the
photo too.

This is off by default because it changes your catalog's keyword tree and
travels with exported files, while the metadata fields stay inside the catalog.
The fields are written either way.

### Coverage

The bundled model covers the taxa a photographer is realistically pointing a
camera at — birds, mammals, insects, reptiles, amphibians, fish, flowering
plants, conifers, ferns, mushrooms — rather than all 867,000 taxa BioCLIP knows,
which would be a multi-gigabyte download. Outside that set the model does not
say "unknown"; it picks the closest thing it does know. The confidence floor is
what keeps those cases at a coarse rank instead of a confidently wrong species.

---

## Metadata Options

### Metadata Tasks

One checkbox each for **Keywords**, **Title**, **Caption**, **Alt Text**.

### Hierarchy & Language

- **Keyword Hierarchy → Enable** — produces category-based keywords instead of
  a flat list. *Edit categories* opens the category editor.
- **Use existing catalog structure** — takes the categories from the keyword
  hierarchy already in your catalog rather than the built-in list. Be careful
  with large or messy hierarchies: everything you have gets offered to the
  model.
- **Top-level Keyword** — nests everything generated under one keyword of your
  choosing, so AI keywords stay separable from your own.
- **Bilingual Keywords** — additionally produces keywords in a second language.
- **Keyword aliases** — reuses keywords already in your catalog instead of
  creating near-duplicates ("bicycle" vs "bike"). See
  [Keyword Dedup & Declutter](Help-Keyword-Dedup-and-Declutter).

### Instructions / Prompt

Prompt templates, plus a free-text custom prompt appended to the built-in one.

---

## Context & Save

### AI Context

Extra information sent alongside the photo to improve accuracy:

- **Existing Keywords** — the photo's keywords go into the prompt. Names that
  Lightroom's face recognition put on the photo are sent separately from the
  rest, and labelled as the people in the picture, so a person called *Ivo* is
  no longer read as scenery and turned into "Ivo Beach".
- **Folder Names**
- **Location (looked up from the photo's GPS coordinates)** — on by default.
  The backend turns the photo's own EXIF position into a place name and puts
  that in the prompt; raw coordinates are never sent. Untick it and nothing
  about where the photo was taken leaves your machine.
- **Ask for context before each batch** — opens a dialog where you can type
  context for the upcoming photos by hand

### Catalog Integration

- **Write generated data to Lightroom catalog** — off means results are stored
  in the backend only, and you apply them later with *Retrieve Metadata from
  Backend*.
- **Review/Edit each photo before saving** — opens a validation dialog per
  photo. A species identification appears there too, read-only (there is
  nothing to reword in a taxonomic name) with its own *Save species* tickbox.
  **Discard** and **Cancel** now drop the species along with everything else —
  before, the species fields, the look-up links and the species keywords were
  written whichever button you pressed.
- **Import metadata from catalog before indexing** — pushes the metadata you
  already have into the backend first, so the AI does not overwrite it blindly.

### Data Handling

- **Mode: Regenerate all (overwrite existing AI data)** vs **Skip photos with
  existing data** (default).
- **Write: Append to existing values instead of replacing** — adds the new text
  below what is already in the field rather than overwriting it.

---

## Related tasks

- **Retrieve Metadata from Backend** — apply stored results to the catalog
  later, or after a failed write.
- **Import Metadata from Catalog** — one-way sync of your Lightroom metadata
  into the backend database.
