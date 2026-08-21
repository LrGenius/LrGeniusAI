# Help: People & Faces

LrGeniusAI provides two face-related workflows:

- **People** — browse face clusters (persons), assign names, and jump to Lightroom collections per person. Its interface opens in your web browser.
- **Find Similar Faces** — select a photo containing a face and find other photos featuring the same person.

---

## Prerequisites

Face data is generated during **Analyze & Index Photos** — tick *Enable face detection* on the *General* tab. It needs the face models on disk; they arrive with the **Download AI models** button in the plugin settings, alongside the search and species ones. Without indexed face data neither workflow finds anything, and the People page stays empty until you have also pressed **Cluster faces** at least once.

---

## People

`Library → Plug-in Extras → People...`

### Where it opens

People opens as a page in your default web browser, served by the LrGeniusAI
backend at `http://127.0.0.1:19819/ui/people`. Lightroom shows a progress bar
named *People (open in your browser)* while the page is live; it is what
carries your selections back into the catalog, so leave it running and cancel
it when you are done.

If the browser never comes up — a blocked pop-up, no default browser — the
plugin tells you after about half a minute and gives you the address to open
by hand.

### What it shows

A grid of persons from your indexed photos. Each card has:

- A thumbnail of the representative face.
- A name field, or *Unassigned faces* for the faces not yet grouped into a
  person.
- The number of photos this person appears in and how many faces are in the
  cluster.

Named persons come first, then unnamed ones; within each group the person with
the most photos is first. Every face that belongs to no person — never
clustered, or left over by the last run — shares a single *Unassigned faces*
entry. The **Filter by name** box and **Unnamed only** checkbox narrow a long
list; **Refresh** re-reads it from the backend.

### Clustering

Detection during indexing finds faces; grouping them into persons is a separate
step. Press **Cluster faces** to (re-)group everything the backend has. The run
reports how many persons and faces it produced, and the grid updates itself.

The **threshold** next to the button is the cosine distance clustering works
at. Leave it at 0.5 unless the result is wrong in a specific way: lower it
towards 0.45 when different people were merged into one cluster, raise it
towards 0.55–0.65 when one person was split across several.

### Assigning names

Type into a card's name field and press Enter or click away — each name is
saved on its own, and the card says *Saved* when it lands. Names survive
re-clustering and are used as context elsewhere.

### Jumping to a Lightroom collection

Click the thumbnails of the people you want; selected cards get a blue border.
The bar at the bottom of the page then offers **Show in Lightroom**. With
several people selected, the dropdown decides what you get:

- **photos with all selected people** — only photos where everyone appears
  together (the default)
- **photos with any selected person** — the union

Lightroom builds the collection and switches the Library to it. This is the one
thing the page cannot do on its own, so it needs the *People* progress bar
still running in Lightroom — if it is not, the page says so instead of leaving
you waiting.

---

## Find Similar Faces

`Library → Plug-in Extras → Find Similar Faces...`

### How to use it

1. Select a **single photo** in the Library grid that contains the face you want to search for.
2. Open *Find Similar Faces* from the menu.
3. The dialog lists every face detected in that photo, with a thumbnail and the
   person's name where one is known. Pick the one to search for.
4. The plugin queries the backend for photos with matching face embeddings
   across all indexed photos.
5. Results are placed into a new Lightroom collection, sorted by similarity.

There are no scope or result-limit options here — the search always runs over
the whole indexed catalog.

---

## Tips

- **Better names = better workflow.** Naming persons early makes it easy to find all photos of a specific subject across your entire catalog.
- Run indexing with face detection on all portrait-heavy shoots before using the People workflow.
- Clustering works by visual similarity, so identical twins or people who look very similar can land in the same cluster. The People page lets you rename a cluster and re-cluster at a different threshold, not split or merge one by hand — if a cluster is wrong, try the threshold first, then index more photos of that person and cluster again.
- For culling portrait sessions, face data also feeds into the **Cull Photos** scoring (eye openness, blink detection, face sharpness). See [Help: Cull Photos](Help-Cull-Photos).

## Upgrading from a version before the face-model change

The face pipeline used to run InsightFace's `buffalo_l` models, which could not
be shipped with the plugin — you had to install them yourself. It now runs YuNet
and FaceNet, which arrive with the ordinary **Download AI models** button.

The two produce face embeddings that cannot be compared with each other, so
faces detected by the old models are not silently reused:

- They are left **unassigned** by **Cluster faces** rather than folded into
  clusters, which would produce confident nonsense.
- Photos holding them are reported as needing processing again, so a normal
  **Analyze & Index Photos** run with *Enable face detection* on re-detects them.
- Nothing is deleted, and names you have given people are kept. As each photo is
  re-detected, its faces reclaim the person they were assigned to.

So: download the models, then re-index the photos with people in them. Until you
do, the People page will show those faces as unassigned.
