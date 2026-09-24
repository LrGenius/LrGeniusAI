# Third-Party Notices

LrGeniusAI as a whole is distributed under the GNU Affero General Public License
v3.0 (see [LICENSE](LICENSE)). Some parts of it originate elsewhere and carry
their own terms. Those terms are reproduced below and continue to apply to those
parts; they are compatible with, and do not restrict, the AGPL-3.0 licensing of
the combined work.

This file covers source that lives **in this repository**. Runtime and build
dependencies resolved by Cargo, LuaRocks or the model downloader are not listed
here — see the [Credits wiki page](https://github.com/LrGenius/LrGeniusAI/wiki/Credits)
for what the project depends on and under which licences.

---

## JSON.lua — Creative Commons Attribution 3.0

- File: `plugin/LrGeniusAI.lrdevplugin/JSON.lua`
- Upstream: <http://regex.info/blog/lua/json>
- Copyright 2010–2017 Jeffrey Friedl

Vendored verbatim. Released under a Creative Commons CC-BY "Attribution"
License, <http://creativecommons.org/licenses/by/3.0/deed.en_US>, whose
conditions are that the copyright notice at the top of the file and the
`AUTHOR_NOTE` string inside it are maintained. Both are intact; do not strip
them when editing the file, and prefer replacing the file wholesale from
upstream over patching it.

---

## Automaat/lightroom-mcp — MIT

- Upstream: <https://github.com/Automaat/lightroom-mcp>
- Copyright (c) 2026 Marcin Skalski

**Scope of this notice.** No file in this repository is a copy of a file from
lightroom-mcp, and no data table, list or configuration was taken from it. What
this project took is smaller and, in the ordinary sense, not copyrightable
expression: CI/CD practice (scoping workflow triggers, concurrency groups,
pinning every action to a commit SHA rather than a tag, pinning the test runner,
and the discipline of verifying a generated version stamp instead of assuming
the write landed), plus a handful of documented Lightroom SDK behaviours. The
entry is here because the ideas were worth having and the source deserves
saying so — not because MIT compels it at this scope.

The MIT permission notice is reproduced in full so that the entry stands on its
own if any of that ever changes:

```
MIT License

Copyright (c) 2026 Marcin Skalski

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## Adding to this file

See the "Adopting third-party code" section in [CONTRIBUTING.md](CONTRIBUTING.md)
for when an entry is required and what it has to contain.
