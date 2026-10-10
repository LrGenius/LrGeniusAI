# Getting Started

One way to get going, start to finish: install, download the models, analyze
your first photos. Everything runs on your own computer — no account, no API
key, no cloud.

**You need:**

- Lightroom Classic 14 or newer
- A Mac with Apple silicon (M1 or newer) **or** a Windows PC
- 16 GB of memory
- About 10 GB of free disk space

---

## Before you start: back up your catalog

LrGeniusAI writes its results into your Lightroom catalog — keywords, titles,
captions, and its own fields such as species or culling results. That has two
side effects you should know about before the first run, because neither can be
undone afterwards:

- **The Edit Date of your photos changes.** Lightroom counts every change a
  plug-in makes to a photo as an edit, so each photo LrGeniusAI processes gets
  the time of the run as its **Edit Date** — even on an *Analyze & Index* run
  that writes no titles, captions or keywords. Smart collections that filter on
  *Edit Date* will pick those photos up. Lightroom gives a plug-in no way to set
  the old date back, and putting old metadata back would itself count as another
  edit.
- **Your image files may change too.** If **Automatically write changes into
  XMP** is turned on (*Catalog Settings → Metadata*), Lightroom writes the new
  keywords, titles and captions into XMP sidecar files, and directly into DNG,
  JPEG, TIFF and PSD files. A cloud or NAS sync tool then sees all of those files
  as changed and uploads them again.

So before you let LrGeniusAI loose on your catalog:

1. **Back up your catalog.** Open **Catalog Settings** (**Edit → Catalog
   Settings…** on Windows, **Lightroom Classic → Catalog Settings…** on a Mac),
   set **Back up catalog** on the **General** tab to **When Lightroom next
   exits**, quit Lightroom and click **Back Up** in the dialog that appears.
2. **Check your XMP setting** on the **Metadata** tab of the same window, and
   whether your photos sit in a folder that a cloud or NAS service syncs.
3. **Start small.** Try LrGeniusAI on a handful of photos (step 3 below) and look
   at what changed before you run it over the whole catalog.

Restoring a catalog backup is the only way to get the old Edit Dates back — and
it also undoes everything else you changed in the catalog since. A catalog
backup does not include your image files, so it cannot undo what Lightroom wrote
into them.

---

## 1. Install

1. Open the [latest release](https://github.com/LrGenius/LrGeniusAI/releases/latest)
   and download the installer for your computer:
   - **Mac:** `LrGeniusAI-macos-arm64-….pkg`
   - **Windows:** `LrGeniusAI-windows-x64-….exe`
2. Quit Lightroom Classic.
3. Run the installer and click through it.
4. Start Lightroom Classic.

That's it — the installer puts the plugin into Lightroom and starts the
LrGeniusAI server in the background.

### If your computer warns about the installer

- **Mac** ("cannot be opened because it is from an unidentified developer"):
  open **System Settings → Privacy & Security**, scroll down and click
  **Open Anyway**.
- **Windows** ("Windows protected your PC"): click **More info**, then
  **Run anyway**.

---

## 2. Download the models

1. In Lightroom, open **File → Plug-in Manager…** and select **LrGeniusAI**.
2. Under **Status**, click **Run Setup Wizard**.
3. On the **Backend Server** tab, read the **Before you start** box (the same
   advice as [above](#before-you-start-back-up-your-catalog)) and check that
   *Server Status* says **Running**.
4. Switch to the **AI Models** tab:
   1. Click **Download AI Models** (about 3.3 GB). These power search, species
      and face detection.
   2. Further down, in **Local AI Model — MLX** (Mac) or **Local AI Model —
      llama.cpp** (Windows), pick **Gemma 4 E4B (recommended)** and click
      **Download** (about 5–6 GB). This is the model that writes your
      keywords, titles and captions.
5. Click **OK**.

The downloads keep running in the background — watch the progress bar in the
top-left corner of Lightroom. Wait until both are finished before you go on.

---

## 3. Analyze your first photos

1. In the **Library**, select a few photos (start with 10–20). Back up your
   catalog first if you have not — see
   [Before you start](#before-you-start-back-up-your-catalog).
2. Open **Library → Plug-in Extras → Analyze & Index Photos…**
3. Under **AI Model**, choose:
   - **Mac:** `On this Mac · gemma-4-e4b-it-4bit`
   - **Windows:** `On this PC · gemma-4-E4B-it-Q4_K_M.gguf`
4. Leave everything else as it is and click **Start**.
5. For each photo, a **Review results** window shows what the AI wrote. Click
   **OK** to save it to the photo.

The first photo takes a little longer while the model loads.

---

## 4. Search your photos

1. Open **Library → Plug-in Extras → Advanced Search…**
2. Describe what you are looking for, e.g. `dog on the beach` or
   `red car in front of a house`.
3. Click **Search**.

The matches appear as a new collection under **Search Results** in the
Collections panel.

---

## Done

Now run **Analyze & Index Photos** over the rest of your catalog.

When you want to explore more, the [Plugin Guide](Plugin-Guide) lists every
feature. Stuck? See [Troubleshooting](Troubleshooting).
