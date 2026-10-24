# Help: Smart Collection Templates

**Library → Plug-in Extras → Create Smart Collection Templates...** creates
ready-made smart collections over LrGeniusAI's own metadata fields, so the
results of indexing and culling are one click away in the Collections panel
instead of a filter you have to build yourself.

They are ordinary Lightroom smart collections: they update by themselves as
LrGeniusAI fills in its fields, you can open them with *Edit Smart
Collection...* to see or change the rules, and deleting them deletes nothing
but the collection.

## What you get

A dialog lists every template with a checkbox (all ticked to start with). The
ones you keep are created in a collection set called **LrGeniusAI**, with one
set per group:

| Set | Collection | Shows photos whose... |
| --- | --- | --- |
| Quality | Best photos (aesthetic score 7+) | *Aesthetic score* is 7.0 or higher |
| Quality | Has quality flags | *Quality flags* field is not empty |
| Quality | Blurred or motion blur | *Quality flags* contain `blurred` or `motion blur` |
| Quality | Clipped highlights or shadows | *Quality flags* contain `highlights clipped` or `shadows clipped` |
| Culling | Culling picks | *Culling decision* is a pick |
| Culling | Culling picks without keywords | *Culling decision* is a pick and the photo has no keywords at all |
| Culling | Culling reject candidates | *Culling decision* is a reject candidate |
| Species | Species identified | a species (or a higher rank) was identified |
| Species | Birds, Mammals, Insects, Reptiles, Amphibians, Plants, Fungi | the identification belongs to that group (Aves, Mammalia, Insecta, Reptilia, Amphibia, Plantae, Fungi) |
| Genre | one per genre (Landscape, Portrait, Street, Wildlife & animals, …) | *Genre* is that genre |

A collection stays empty until the task that fills its field has run on your
photos:

- **Aesthetic score** and **Genre** — *Analyze & Index Photos* with *Enable
  smart photo search*.
- **Quality flags** — any *Analyze & Index Photos* run, or *Cull Similar
  Photos*. See [Quality flags](Help-Analyze-and-Index#quality-flags).
- **Culling decision** — *Cull Similar Photos*. A photo keeps the decision of
  the last cull it was part of.
- **Species** — *Analyze & Index Photos* with *Identify animal and plant
  species*.

The genre names come from the LrGeniusAI backend. If it is not running when
you open the dialog, the Genre group is missing and the dialog says so; start
the backend and run the command again to add them.

## Running it again

Running the command again is safe. A collection whose name is already taken in
its set is left exactly as it is — including any rules you changed — and the
completion message counts it. To get a template back in its original form,
delete the collection and run the command again. If an *ordinary* collection
already has one of the names, that template is not created and the message
names it.

## Limits

- Lightroom compares these fields as text, not as numbers, so "7 or higher" is
  written as "starts with 7, 8 or 9". That is exact for the score, which the
  plug-in writes with one decimal on a 1–9 scale, but it is why there is no
  template with an arbitrary threshold such as 6.5. Edit the collection to
  change the digits if you want a different cut.
- The collections read the fields in the catalog, so they only see what was
  written there. If indexing ran with *Save data to catalog* off, run
  **Retrieve Metadata from Backend** first.

## Related

- [Help: Analyze and Index](Help-Analyze-and-Index)
- [Help: Cull Photos](Help-Cull-Photos)
- [Help: Advanced Search](Help-Advanced-Search) — for a search by content
  rather than by a field
