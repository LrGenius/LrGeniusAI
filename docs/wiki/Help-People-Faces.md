# Help: People & Faces

LrGeniusAI provides two face-related workflows:

- **People** — browse face clusters (persons), assign names, and jump to Lightroom collections per person.
- **Find Similar Faces** — select a photo containing a face and find other photos featuring the same person.

---

## Prerequisites

Face data is generated during **Analyze & Index Photos** — tick *Enable face detection* on the *General* tab. Without indexed face data neither workflow finds anything, and the People dialog stays empty until you have also pressed **Cluster faces** at least once.

---

## People

`Library → Plug-in Extras → People...`

### What it shows

The dialog shows a grid of persons from your indexed photos. Each cell has:

- A thumbnail of the representative face.
- An editable name field, or *Unnamed* for faces not yet grouped into a person.
- The number of photos this person appears in.
- A **Library** checkbox.

Named persons come first, then unnamed ones; within each group the person with
the most photos is first. Every face that belongs to no person — never
clustered, or left over by the last run — shares a single *Unnamed* entry.

### Clustering

Detection during indexing finds faces; grouping them into persons is a separate
step. Press **Cluster faces** to (re-)group everything the backend has. The run
reports how many persons and faces it produced — reopen *People...* afterwards
to see the new list, the open dialog does not refresh itself.

### Assigning names

Type directly into a person's name field. **OK** writes all edited names to the
backend, **Reset** reverts your edits, **Cancel** closes without saving. Names
survive re-clustering and are used as context elsewhere.

### Jumping to a Lightroom collection

Tick **Library** on one or more people and press **Show in Library**. With
several people selected, the dropdown decides what you get:

- **Photos with any selected person** — the union
- **Photos with all selected people** — only photos where everyone appears
  together (the default)

The result opens as a Lightroom collection.

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
- Clustering works by visual similarity, so identical twins or people who look very similar can land in the same cluster. The People dialog lets you rename a cluster, not split or merge one — if a cluster is wrong, index more photos of that person and cluster again.
- For culling portrait sessions, face data also feeds into the **Cull Photos** scoring (eye openness, blink detection, face sharpness). See [Help: Cull Photos](Help-Cull-Photos).
