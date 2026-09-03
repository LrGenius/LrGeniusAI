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

Prompt templates. The text box holds the selected template's instructions, and
what is in it *replaces* the backend's built-in persona for that run rather
than being appended to it.

A template is a *system* prompt: it decides which expert is looking at the
photo, which vocabulary they reach for, how specific they are allowed to be,
and what they must never invent. It does not decide the output fields, the
language or the keyword structure — those come from the request itself, from
the switches on this dialog. So picking a template changes the voice and the
precision of what you get, not its shape.

**Default** is the analytical voice: species, landmarks, vehicle makes,
described precisely. Unchanged, and still what every catalog uses unless you
pick otherwise.

**Family & Everyday Photos** is the album voice: who is in the picture, what
they are doing, where, and what the occasion appears to be. It asks for the
people by the names your catalog already has on them and for the place in the
title, and it is explicitly forbidden to invent what it was not given — no
relationships, no birthdays or weddings, no landmark the photo's own data does
not support.

The genre templates each bring a working vocabulary and a specific line they
will not cross:

- **Wildlife & Nature** — field-biologist naming: common name plus the
  scientific name where it is certain, genus or family where it is not, with
  behaviour, life stage, habitat and season. It will not anthropomorphise or
  guess a species.
- **Landscape & Travel** — landform and terrain in the words a map would use,
  plus light, weather and season. It will not invent a named peak, lake, trail
  or landmark.
- **Architecture & Urban** — building type, period and style, materials and the
  elements carrying the composition. It names a specific building or architect
  only when it is certain.
- **Events & Weddings** — the part of the day a frame belongs to, people by
  name or by role, and the details that make a frame findable months later. It
  will not guess relationships, traditions or the meaning of a ritual.
- **Sports & Action** — the sport, the discipline and the phase of play, filed
  the way a wire caption is. It will not invent a team, a competition or a
  score.
- **Street & Documentary** — observational restraint: the gesture, the light,
  the geometry, the setting, with dignity toward everyone in the frame and no
  inferred story.
- **Portrait & Studio** — the kind of portrait and the craft behind it,
  lighting pattern included. It will not speculate about the person.
- **Product & Stock** — commercial keywording a buyer would actually type: shot
  type, material, finish, negative space, concept. Brands only when legibly
  visible.
- **Food & Drink** — the dish, its components, the preparation and the styling,
  without inventing an ingredient or a dietary claim.
- **Night & Astro** — what is actually in the sky and how the frame was made,
  without inventing a catalogue designation or an event.

The people-facing templates pair with **People's names from face recognition**
and **Location** below; without those switches they have nothing to name.

Both are ordinary templates once they appear: edit them, add your own, delete
what you do not want. A template you delete stays deleted, and one you rewrite
stays rewritten — the plug-in only ever offers each built-in once. "Reset to
defaults" in the prompt dialog brings both back.

**Clearing a template's text** is allowed and is saved: an empty template means
"no persona of my own", and the run uses the backend's built-in one instead of
being sent an empty instruction. The template itself stays in the menu — it is
empty, not deleted.

**Default** is the one template the Delete button refuses, because it is the
fallback every other selection resolves to. If it is missing from an older
install it is put back the next time the plug-in loads; that is a repair, not a
re-offer, and it leaves the templates you deleted deleted.

---

## Context & Save

### AI Context

Extra information sent alongside the photo to improve accuracy:

- **Existing Keywords** — the photo's keywords go into the prompt.
- **People's names from face recognition** — the names Lightroom put on the
  faces are sent, separately from the keywords and labelled as the people in
  the picture. Two things follow: a person called *Ivo* is no longer read as
  scenery and turned into "Ivo Beach", and the model is asked to *use* the
  names, so a photo of two tagged people gets "Ivo and Myriam on the beach"
  rather than "a man and a woman on a beach". With several people named and no
  way to tell which face is which, it names them together rather than guessing
  who is who.

  Its own switch since it is its own decision: a name identifies a person, and
  a household happy to send "beach, sunset" to a cloud provider may well not
  want to send "Ivo, Myriam" with it. Untick it and no name from your catalog
  leaves the computer — for AI edits either, which never had a checkbox of
  their own and follow this one. On upgrade it inherits whatever **Existing
  Keywords** was set to, so nothing starts being sent that was not being sent
  before.
- **Folder Names**
- **Location** — on by default, and the place reaches the prompt from whichever
  of these knows it:
  1. Lightroom's own location fields (Sublocation, City, State/Province,
     Country) as the catalog holds them, which is also the only version that
     includes a place you corrected after import;
  2. failing that, what the file itself carries — its IPTC/EXIF, and the XMP
     sidecar beside a raw original;
  3. failing that, the photo's GPS coordinates, looked up against an offline
     list of ~145,000 places (GeoNames `cities1000`, bundled — nothing is asked
     of the network). This is what covers the common case where Lightroom's
     address lookup only ever *suggested* a city and you never confirmed it:
     the suggestions are not stored anywhere the plug-in can read, but the
     coordinates are. The prompt marks a looked-up place as looked up, and says
     "near" when the nearest known place is more than 5 km away, so the model
     names the area rather than inventing a landmark.

  Raw coordinates only leave your machine if no place name can be worked out at
  all. Untick the option and nothing about where the photo was taken is sent.

  Two configurations used to lose the location silently, and no longer do:
  **Submit originals instead of exports** (Plug-in Manager) and an **export size
  above 2048 px**. Both make the backend re-encode the image, and the JPEG it
  produces has no metadata left to read.
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
