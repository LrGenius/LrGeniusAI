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

- **Existing Keywords**
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
  photo.
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
