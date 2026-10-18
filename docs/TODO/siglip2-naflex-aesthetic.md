# Plan: SigLIP2 So400m NaFlex + AestheticSigLIP, Genre, Katalog-Aufräumen, Ästhetik-Suche

## Context

- Heute läuft `siglip2-so400m-384/fp16` (fester 384²-Center-Crop, `lrg-ml/src/siglip.rs`, `image_pre.rs`). Der Crop schneidet bei Hoch- und Panoramaformaten Bildteile weg. NaFlex behält das Seitenverhältnis bei.
- Die Ästhetik beim Culling ist heute zusammengesetzt: eine Heuristik (`cull_aesthetic` aus Kontrast, Farbigkeit und Belichtung) wird mit CLIP-IQA-Prompt-Paaren gemischt (`lrg-ml/src/clip_iqa.rs`, `group_similar.rs:454`, `grouping.rs:390-399`). **somepago/AestheticSigLIP** ist ein auf Ästhetik feinjustiertes **SigLIP2 So400m NaFlex**: Feature-Tapping über mehrere Layer, SORD- und Ranking-Loss, Score 1–10, Gewichte als `best.pt`. Es ist also ein **eigener Backbone**, kein Kopf auf unserem gespeicherten Embedding.
- Im Katalog gibt es 42 Plug-in-Felder (`MetadataProvider.lua`). Davon sind 20 `cull*`-Felder, das Feld `keywords` schreibt niemand mehr, und `aiEditRecipe` (JSON) sowie die Legacy-Felder `globalPhotoId*` sind im Panel sichtbar.
- Die erweiterte Suche (`routes/search.rs`, `TaskSemanticSearch.lua`) braucht immer einen Suchbegriff und kennt keine Qualitätswerte.

**Entscheidungen (mit dem User geklärt):** Das Genre kommt per SigLIP2-Zero-Shot. Die Culling-Felder werden auf das Nötige reduziert, Internes wird ausgeblendet. AestheticSigLIP läuft beim Index **und** beim Cull-Ingest und wird einmal gespeichert.

---

## Phase 0 – Spike / Gates (vor jedem Code)

1. **Model Card und Code von AestheticSigLIP lesen.** Zu klären:
   - Lizenz: Dürfen wir ein ONNX auf unserem Release weiterverteilen?
   - Mit welchem `max_num_patches` wurde trainiert?
   - Welche Layer werden getappt?
   - Was liefert der Head (Verteilung über 10 Bins → Erwartungswert)?
   - Wie wird normalisiert?

   Verbietet die Lizenz die Weiterverteilung: stoppen und mit dem User klären.
2. **ONNX-Export für NaFlex prüfen.** `Siglip2VisionEmbeddings.resize_positional_embeddings` interpoliert bilinear mit Antialias auf die dynamischen `spatial_shapes`, das braucht Opset ≥ 18.
   - Fallback A: Die Positions-Embeddings in Rust vorberechnen und als zusätzlichen Input übergeben.
   - Fallback B: Feste Seitenverhältnis-Buckets.
3. **Latenz messen.** NaFlex@576 Patches gegen heute 384² (CPU und CoreML), dazu den Ästhetik-Tower. Erwartet werden etwa doppelt so viele So400m-Durchläufe pro Foto.

## Phase 1 – SigLIP2 So400m NaFlex als Embedding-Modell (PR 1)

- **Export:** `server-rs/scripts/export_siglip2_naflex_fp16.py`, angelehnt an `export_siglip2_fp16.py`.
  - Image-Tower-Inputs: `pixel_values [1,N,768]`, `pixel_attention_mask [1,N]`, `spatial_shapes [1,2]`.
  - Text-Tower: Gemma-Tokenizer, 64 Tokens. `text_pre.rs`/`CONTEXT_LENGTH` gegenprüfen.
  - Goldens neu erzeugen (`testdata/siglip_goldens.json`).
- **Neu: `lrg-ml/src/naflex_pre.rs`.** Ersetzt für SigLIP `image_pre::preprocess`, bleibt aber für BioCLIP erhalten (`MEAN`/`STD` sind geteilt).
  - Port von HF `get_image_size_for_max_num_patches`, dann bilinearer Resize (`lrg_imaging::pil_resample::resize_plane`).
  - Rescale 1/255, Normalisierung 0.5/0.5.
  - Patchify in der Reihenfolge (nh, nw, p, p, C), auf `MAX_NUM_PATCHES` auffüllen, Maske setzen.
  - Default `MAX_NUM_PATCHES = 576` (Kosten wie heute), außer Phase 0 ergibt für AestheticSigLIP einen anderen Wert. Dann diesen Wert für beide nehmen, damit die Vorverarbeitung **einmal pro Foto** läuft.
- **`siglip.rs`:** Neue Inputs. `MODEL_ID` wird zu `"siglip2-so400m-naflex-576/fp16"`.
  - Das stößt automatisch eine Neuindexierung an (`/v1/index/unprocessed`, `index.rs:105`; Stale-Check `index_upload.rs:1387`).
  - Die Prompt-Embeddings in `text_embeddings` hängen an der `model_id` und werden von selbst neu aufgebaut (`prompt_embeddings.rs`).
- **Assets:**
  - Neue Dateinamen in `model_paths.rs::resolve()`, damit alte 384er-Dateien nicht als „vorhanden“ zählen.
  - Neuer Release-Tag `model-assets-v2` in `routes/clip.rs` (`MODEL_ASSETS_RELEASE_TAG`, `clip_assets`). v1 bleibt für alte Builds unangetastet.
- **Weitere Konsumenten der Embeddings:**
  - `routes/training.rs:140`: Edit-Training speichert SigLIP-Vektoren. Model-ID mitspeichern und veraltete Beispiele neu einbetten, mit Warnung.
  - Schwellwerte neu prüfen, die auf das alte Modell kalibriert sind: `relevance_filter.rs` (Knee), Near-Duplicate-Cosinus in `group_similar.rs`, `keywords.rs`-Clustering, BioCLIP-Organism-Gate. Werkzeug dafür: `lrg-analysis/examples/cull_eval.rs` und die Goldens.
- **Plugin:** Die Coverage-Warnung (`TaskSemanticSearch.lua buildCoverageWarning`) bzw. der Analyze-Dialog soll klar sagen: „Neues KI-Modell – Analyze & Index erneut ausführen“.

## Phase 2 – AestheticSigLIP + Genre beim Index (PR 2)

- **Export:** `scripts/export_aesthetic_siglip_fp16.py` lädt Modellcode und `best.pt`. Das ONNX nimmt dieselben NaFlex-Inputs und gibt `score` (1–10) aus, optional zusätzlich die Bin-Verteilung. Dazu Goldens.
- **Neu: `lrg-ml/src/aesthetic.rs`.**
  - Gleiches Lazy-Load-, Idle-Unload- und Status-Muster wie `siglip.rs`. Dafür `Tower<T>` in ein gemeinsames `tower.rs` ziehen und wiederverwenden.
  - Eigene `MODEL_ID`. `model_paths::resolve_aesthetic()` mit `LRG_AESTHETIC_ONNX`.
  - Download über `model-assets-v2`. Readiness in `routes/assets.rs` und in `/health`.
- **Genre, neu in `lrg-ml/src/genre.rs`:**
  - Feste Liste von etwa 12–14 Genres (Landschaft, Porträt, Street, Wildlife, Sport, Event/Hochzeit, Architektur, Makro, Food, Produkt/Stillleben, Astro/Nacht, Konzert/Bühne, Luftaufnahme), jeweils 1–2 Prompts.
  - Text-Embeddings über `prompt_embeddings::prepare` (gecacht pro `model_id`). Klassifikation per Cosinus-Argmax mit Mindestabstand, sonst `other`.
  - Kostet nur Skalarprodukte auf dem gerade berechneten Embedding.
- **Index (`index_upload.rs`):**
  - `parse_options` (~278): Neuer Task `aesthetic`. `embeddings` und `cull` schließen ihn **beide** ein. NaFlex-Patches werden einmal berechnet und beiden Towers gegeben.
  - Gespeichert wird in den Metadata-JSON-Feldern: `aesthetic_score`, `aesthetic_model`, `genre`, `genre_confidence`.
  - Überspringen, wenn `aesthetic_model` aktuell ist (Muster wie beim `embedding_model`). `/v1/index/unprocessed` reiht Fotos ohne aktuellen Score erneut ein.
  - Die Index-Antwort liefert `aesthetic_score` und `genre`. Fehlt das Modell, kommt das in die `warnings` mit einem Hinweis, was zu tun ist („Ästhetik-Modell nicht geladen – „Download AI models“ im Plug-in Manager“). Gilt nach CLAUDE.md „Errors must surface“.
- **Plugin:**
  - `MetadataManager.lua` (beim `aiModel`-Write ~271) und `TaskRetrieveMetadata.lua` schreiben `aestheticScore` (Format `"7.4"`) und `genre`.
  - `APISearchIndex.lua` reicht beides durch, `warnings` werden bis zum Dialog gesammelt.

## Phase 3 – Katalogfelder aufräumen (gleicher PR wie Phase 2)

**Mechanik zum Ausblenden:** Ein Feld in `metadataFieldsForPhotos` **ohne `title`** (und mit `searchable`/`browsable = false`) speichert Daten, erscheint aber weder im Metadaten-Panel noch in „All Plug-in Metadata“, Filtern oder Smart Collections. `schemaVersion` geht von 33 auf 34. Das in Lightroom verifizieren.

| Gruppe | Sichtbar | Ausgeblendet (ohne title) | Entfernt |
|---|---|---|---|
| Allgemein | aiModel, aiLastRun, **genre**, **aestheticScore**, photoContext | – | `keywords` (wird nicht geschrieben) |
| Arten | speciesCommon/Scientific/Rank/Confidence/Taxonomy, iNat/Wikipedia-URL | – | – |
| Culling | cullDecision, cullScore, cullGroupRank, cullGroupId, cullExplanation | – | cullGroupType, cullGroupWinner (= Rang 1), cullReasonCodes (steckt in der Erklärung), alle Teilmetriken (Sharpness, Exposure, Noise, TechnicalScore, Face*, Occlusion, EyeOpenness, BlinkPenalty), cullAesthetic (ersetzt durch aestheticScore) |
| AI Edit | aiEditLastRun, aiEditModel, aiEditStatus, aiEditSummary, aiEditWarnings | aiEditRecipe | – |
| Legacy-ID | – | globalPhotoId, …FileSize, …ModificationDate, …Algorithm (Definition bleibt für Upgrades, `Util.lua:1000`) | – |

- Die Teilmetriken liest im Plugin niemand zurück (nur `setPropertyForPlugin` in `TaskCullPhotos.lua:661-681`). In der Backend-DB und in der Erklärung bleiben sie erhalten. Weniger Writes bedeuten auch weniger `lastEditTime`-Wechsel (`Util.lua:925`).
- Die Felder in logischen Gruppen anordnen. `MetadataTagset.lua`: genre und aestheticScore oben, dazu ein Culling-Block mit den 5 Kernfeldern.

## Phase 4 – Culling nutzt AestheticSigLIP (PR 3)

- `group_similar.rs:454`: Der `PromptSet::Quality`-Pass wird ersetzt. Der gespeicherte `aesthetic_score` wird als `(s-1)/9` in den Rank-Input injiziert. Den Key in `cull_aesthetic_model` umbenennen und `grouping.rs:390-399` anpassen.
- Fallback-Kette: AestheticSigLIP, sonst Heuristik. Das CLIP-IQA-Quality-Set und `DEFAULT_PROMPT_PAIRS` entfallen, falls `eval.rs`/`cull_eval` sie sonst nicht brauchen. Die Moment-, Emotion- und Kiss-Prompt-Sets bleiben, sie laufen auf dem SigLIP2-Embedding.
- `culling_config.rs`: `aesthetic_iqa_weight` heißt künftig `aesthetic_model_weight`, Default etwa 0.9. Mit `cull_eval` und den Fixtures aus `TaskExportCullFixture` neu tunen. `grouping_goldens.rs` aktualisieren.
- Fehlt Fotos der Score, sammelt die Warnung „N Fotos ohne Ästhetik-Score“ über die bestehende `UnavailableSignals`-Kette und zeigt sie im Dialog.

## Phase 5 – Erweiterte Suche mit Ästhetik (PR 4)

**Backend (`routes/search.rs`):**
- `term` wird optional. Neue Body-Felder:
  - `sort`: `relevance` | `aesthetic` | `combined`. Default ist `relevance` mit Suchbegriff, `aesthetic` ohne.
  - `min_aesthetic` (1–10)
  - `genres` (Liste)
  - `aesthetic_weight` (für `combined`)
- **Ohne Suchbegriff („Beste N“):** `scan_meta` (ohne Vektoren und ohne Text-Tower), dann `in_scope` + Filter anwenden, nach `aesthetic_score` absteigend sortieren und `max_results` nehmen.
- **Mit Suchbegriff:** Semantik wie bisher, danach die Filter. `combined` = (1−w)·Relevanz (normalisiert im Ergebnisset) + w·(s−1)/9. Metadaten-Treffer ohne Distanz werden nach Ästhetik einsortiert.
- Ergebniszeilen bekommen `aesthetic_score` und `genre`. Fotos im Scope ohne Score gehen als Anzahl in die `warnings`.
- Neu: `GET /v1/search/genres` liefert die Genre-Liste aus `genre.rs` als einzige Quelle für den Dialog.
- Contract-Tests in `lrg-api/tests/contract.rs` anpassen; der 400-Fall „kein Suchbegriff“ fällt weg.

**Plugin:**
- `APISearchIndex.lua`: `searchIndex` reicht sort, minAesthetic und genres durch. Neues Genres-Endpoint plus Eintrag in `ENDPOINTS`.
- `TaskSemanticSearch.lua`: neue Gruppe „Ranking“.
  - Popup „Sortieren nach“: Relevanz / Ästhetik / Kombiniert.
  - Slider „Mindest-Ästhetik“ (0 = aus, 1–10, Schritt 0.5).
  - Popup „Genre“ (Alle + Liste).
- Ein leerer Suchbegriff ist erlaubt und bedeutet „Beste N“. `requireClip` gilt nur mit Suchbegriff. Der Dialog „No Search Criteria“ entfällt.
- Collection-Name z. B. `Best 50 · ≥7.0 · Landscape @ …`. Der Hinweis auf „Custom Order“ existiert bereits (Zeile 543).
- Reine Hilfsfunktion zum Bauen der Search-Optionen bzw. Formatieren des Scores in `Util.lua`, dazu eine busted-Spec.

## Phase 6 – Doku und Regeln (jeweils im selben PR)

- Wiki-Seiten:
  - `docs/wiki/Dev-Backend-API.md` (search, genres, Index-Antwort, Assets)
  - `Help-Advanced-Search.md`, `Help-Cull-Photos.md`, `Help-Analyze-and-Index.md`
  - `Credits.md` (AestheticSigLIP, SigLIP2 NaFlex)
  - `server-rs/README.md` (Export, Env-Vars)
- Golden-Familie `aesthetic`:
  - in `CLAUDE.md` **und** `AGENTS.md` bei `LRG_REQUIRE_GOLDENS` ergänzen, bei Bedarf in `CONTRIBUTING.md`
  - in `golden-tests-full.yml` laden lassen
- `scripts/check-docs.py --for <datei>` für jede berührte Datei ausführen.

## Verifikation

- `cd server-rs && cargo fmt && cargo clippy --workspace --all-targets && cargo test --workspace`
- Goldens mit `LRG_REQUIRE_GOLDENS=siglip,aesthetic` (Assets lokal exportiert)
- Live: `cargo run -p lrg-server -- --db-path … --debug`, dann
  - Index-Upload mit `tasks=embeddings` → Metadaten enthalten `aesthetic_score`, `genre`, neues `embedding_model`
  - `tasks=cull` → Score vorhanden
  - `curl -X POST /search` ohne `term`, mit `sort=aesthetic&max_results=20` → absteigend sortiert; mit `min_aesthetic`/`genres` → gefiltert; ohne Modell → `warnings`
  - `group_similar` → Ranking nutzt `cull_aesthetic_model`
- `busted` sowie `python3 scripts/check-docs.py`
- In Lightroom:
  - Nach dem Schema-Bump zeigt das Metadaten-Panel nur noch die sichtbaren Felder, die ausgeblendeten Werte bleiben lesbar (Smoke-Test über `TaskAutomatedTests.lua`).
  - Erweiterte Suche ohne Begriff ergibt eine Collection „Beste N“ in Custom Order.
  - Cull-Lauf schreibt nur noch die 5 Kernfelder.

## PR-Schnitt (passt zum Token-Budget, je PR eine Session)

1. NaFlex-Umstieg (Phase 0 + 1)
2. AestheticSigLIP + Genre beim Index + Katalog-Aufräumen (Phase 2 + 3)
3. Culling auf AestheticSigLIP (Phase 4)
4. Ästhetik-Suche / „Beste N“ (Phase 5)

Später, nicht in diesem Plan: das Genre schlägt automatisch das Culling-Preset vor.
