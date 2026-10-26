# Help: Advanced Search

Advanced Search finds photos by description rather than by filename or keyword —
"red sports car in front of a garage" instead of remembering which shoot it was
in. Results land in a new Lightroom collection.

## Open advanced search

In Lightroom Classic:

- `Library → Plug-in Extras → Advanced Search...` — the dialog described on most
  of this page
- `Library → Plug-in Extras → Search (browser)...` — the same search in your web
  browser, with a preview of every result while you adjust it (see
  [Search in your browser](#search-in-your-browser) below)

## What the search can see

A text search only finds photos that carry a SigLIP embedding, so a
half-indexed catalog answers "no results" for a subject that is sitting right
there. The dialog says so before you type: if the search model is not
downloaded, or some of the catalog has no search index yet, an orange note at
the top gives the count and what to run to fix it. After an update that
switches to a new AI search model, photos indexed with the old one count as
not searchable too, and the note names them separately: run **Analyze &
Index Photos** with **Enable smart photo search** again and they are brought
up to date.

The check is two cheap lookups (`/v1/db/stats` plus the catalog's photo count) and
runs every time the dialog opens. It is a note, not a gate — the search still
runs across whatever *is* indexed.

## Search term

Free text, or **empty** to list your best photos: with no term the search ranks
everything in scope by its **Aesthetic score** and gives you the best ones (up
to **Max results**), optionally restricted by the controls under *Ranking*.
Photos that have no score yet are not part of that list; the dialog says how
many, and **Analyze & Index Photos** fills them in.

With a term, describe what is in the photo, not how it was shot:

- `red sports car in front of a garage`
- `sunset over mountains with orange sky`
- `two people laughing at a wooden table`

The field is also a drop-down: its menu lists your last 15 searches, most recent
first, so a search you run again after indexing more photos is one click rather
than retyped. Pick one and edit it, or type over it as usual.

## Search scope

- **All photos** — everything indexed in the backend
- **Current view** — the folder or collection currently open
- **Selected photos** — the current selection only

## Ranking

- **Sort by** — *Relevance* (the default for a search with a term),
  *Aesthetic score*, or *Relevance and aesthetic score*, which mixes the two
  evenly. Without a term the order is always the aesthetic score.
- **Minimum aesthetic score** — drop photos scored below this, from 1.0 to 9.0
  in half-points; *Off* at the left end. Typical photos sit between about 4.5
  and 7; treat it as a way to sort and thin out your own photos, not as a
  verdict.
- **Genre** — only photos of one genre (landscape, portrait, street, wildlife &
  animals, sport, event & wedding, architecture, macro, food, product & still
  life, astro & night sky, concert & stage, aerial, or *Other*). The genre is a
  guess from the picture, made when the photo is indexed.

Photos with no score (or no genre) cannot pass a minimum (or a genre filter), and
the search says how many it left out for that reason. The collection's name says
what shaped it, for example `Best 50 · ≥7.0 · Landscape @ …`.

## Tuning

### Relevance strictness (0–100)

How aggressively weak matches are dropped. `0` turns filtering off and returns
the full ranked list; `50` is moderate; `100` keeps only strong matches. The
filter looks for the point where result scores fall off a cliff and cuts there,
so a strict setting on a query with many good matches still returns many
results.

### Max results (50–1000)

Upper bound on how many photos end up in the collection, across both halves of
the search. Default 100.

## Search in

At least one of these has to be on:

- **Semantic (AI search)** — matches the meaning of the query against
  the image embedding. Requires the photos to have been indexed with
  *Create search embeddings*, and the SigLIP model to be downloaded.
- **Metadata** — plain substring matching against the AI-generated fields.
  Each field can be switched on separately: **Keywords**, **Caption**,
  **Title**, **Alt text**. Photos need AI metadata for this to find anything.

Semantic matches are ranked by similarity and come first; metadata matches fill
whatever is left of **Max results**.

## Results

The plugin creates a collection under the **Search Results** collection set,
named after the query and scope.

**Set the collection sort order to `Custom Order`** — that is the only order in
which Lightroom shows the best matches at the top. Any other sort order throws
the ranking away.

If the search matched photos that are no longer in the catalog, a **Search
Results Are Incomplete** message says how many. Those are usually photos you
deleted from Lightroom that are still in the index: when the automatic index
cleanup is on, the message also says that a cleanup has started in the
background, and the next search no longer finds them (see the
[FAQ](FAQ.md#what-happens-to-the-index-when-i-delete-photos-in-lightroom)).

## Search in your browser

`Library → Plug-in Extras → Search (browser)...` opens the search as a page in
your default web browser, served by the LrGeniusAI backend at
`http://127.0.0.1:19819/v1/ui/search`. It is the same search as the dialog —
same index, same ranking, same options — but you see the results while you
work on it: every change searches again, and each result is shown as a small
preview with its rank, aesthetic score and genre.

### Where it opens

Lightroom shows a progress bar named *Search (open in your browser)* while the
page is live. It is what tells the page what Lightroom is showing and what turns
results into a collection, so leave it running and cancel it when you are done.
The page shows *Lightroom: connected* at the top right while it is.

If the browser never comes up — a blocked pop-up, no default browser — the
plugin tells you after about half a minute and gives you the address to open by
hand. Use that whole address: the part after `#` is the backend's access token,
and the page cannot load anything without it. A bookmark of the page does not
keep it, so open the search from Lightroom each time.

### What you can change

- **Search term** — as in the dialog, or empty for your best photos by aesthetic
  score. The search runs a moment after you stop typing, or at once with
  **Enter**. The field suggests your earlier searches, the dialog's included.
- **Scope** — *All indexed photos in this catalog*, *Current view* (the folder or
  collections open in Lightroom, named in the menu with their photo count) or
  *Selected photos*. The page learns the view and the selection when it opens;
  after you change either in Lightroom, click **Update from Lightroom**.
- **Sort**, **Min. aesthetic**, **Genre**, **Strictness**, **Max results** and
  **Keywords and captions** (the metadata half of the search, all four fields) —
  the dialog's controls of the same name. They start where you left them in the
  dialog.

Sliders and menus search again as you move them. Notes from the backend — no
search model, photos without a score left out by the minimum, and the like —
appear above the results with what to do about them, and so do the dialog's
notes about photos that cannot be found because they were never indexed.

### Previews

The previews are small copies (256 pixels on the long side) that **Analyze &
Index Photos** stores for each photo it processes, whatever you ask it to do;
they take about 13 KB a photo on the backend's disk, roughly 130 MB per 10,000
photos, beside the search index. Photos indexed before this version have none
yet: they show *No preview yet*, and the page says how many. To add them, click
**Show in Lightroom**, select the photos of the new collection and run **Analyze
& Index Photos** with the scope *Selected photos only* and *Skip photos with
existing data* — the previews are added and nothing that is already there is made again.
The scope *New or unprocessed photos* does not pick them up, because nothing
they need for searching is missing.

A preview is deleted with its photo's index data (the index cleanup, or
removing the photo from the index), and is not part of a database backup —
indexing makes it again.

### Show in Lightroom

**Show in Lightroom** turns the results on the page into a collection under
**Search Results**, named like the dialog's, with the photos in the order the
page shows them, and switches the Library to it. As with the dialog, set the
collection's sort order to `Custom Order` to keep that order. If some results
are not in the open catalog, a message says how many: usually photos you
deleted in Lightroom after they were indexed, which the index cleanup then
takes out of the index in the background.

If the page says *Lightroom is not listening*, the progress bar in Lightroom
was cancelled: open **Search (browser)...** again.
