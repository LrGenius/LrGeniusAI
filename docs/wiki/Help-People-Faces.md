# Help: People & Faces

LrGeniusAI provides two face-related workflows:

- **People** — browse face clusters (persons), assign names, and jump to Lightroom collections per person.
- **Find Similar Faces** — select a photo containing a face and find other photos featuring the same person.

---

## Prerequisites

Face data is generated during **Analyze & Index Photos**. Make sure face detection is enabled when you run indexing. Without indexed face data neither workflow will find anything.

---

## People

`Library → Plug-in Extras → People...`

### What it shows

The People dialog lists all detected persons from your indexed photos. Each entry shows:

- A thumbnail of the representative face cluster.
- The person's name (if assigned) or *Unknown* if not yet named.
- The number of photos in which this person appears.

Named persons are shown first, sorted by photo count. Unnamed clusters follow in the same order.

### Assigning names

Click a person entry to assign or edit the name. Names are stored on the backend and used for future face clustering and search context.

### Jumping to a Lightroom collection

Each person can be opened as a Lightroom collection — the collection contains all photos in which that person was detected. This lets you browse a person's photos directly in the Library grid without creating a manual filter.

---

## Find Similar Faces

`Library → Plug-in Extras → Find Similar Faces...`

### How to use it

1. Select a **single photo** in the Library grid that contains a face you want to search for.
2. Open *Find Similar Faces* from the menu.
3. Adjust search options if needed (scope, result limit).
4. The plugin queries the backend for photos with matching face embeddings.
5. Results are placed into a new Lightroom collection, sorted by similarity.

### Search scope

- **All indexed photos** — searches the entire backend.
- **Current view** — restricts search to the currently visible folder or collection.

---

## Tips

- **Better names = better workflow.** Naming persons early makes it easy to find all photos of a specific subject across your entire catalog.
- Run indexing with face detection on all portrait-heavy shoots before using the People workflow.
- Face clustering works by visual similarity — identical twins or people who look very similar may end up in the same cluster. Review and correct clusters manually via the People dialog.
- For culling portrait sessions, face data also feeds into the **Cull Photos** scoring (eye openness, blink detection, face sharpness). See [Help: Cull Photos](Help-Cull-Photos).
