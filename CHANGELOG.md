# Changelog

All notable changes to this project will be documented in this file.

## [v2.13.0-pre] - 2026-04-11

- docs: add privacy policy document outlining local-first data handling practices
- Add GNU AGPL v3 license
- docs: update documentation with specific bypass instructions for Windows SmartScreen and macOS Gatekeeper warnings
- refactor: update shutdown logic to unload resources from both local and remote backends
- feat: add /unload API endpoint to free model and collection memory without stopping the server fix: JSON decode error popups in a loop in plug-in manager while backend not ready.
- fix: update Windows installer architecture compatibility and improve build artifact output handling
- chore: add SourceDir directive to Windows installer configuration
- feat: add application icon to plugin and server directories
- fix: disable MSYS path conversion for Inno Setup compiler to prevent path resolution errors
- refactor: make DB_PATH handling robust against uninitialized states across services
- feat: implement dynamic database initialization, add server restart/init API endpoints, and introduce macOS/Windows installer build workflows.
- feat: disable onboarding wizard display during plugin initialization
- refactor: improve system health diagnostics, update onboarding strings, and enforce provider configuration checks in AI tasks.
- feat: implement onboarding wizard and system health diagnostics for backend and model configuration
- refactor: implement granular file-by-file model downloading to improve progress tracking accuracy
- fix: add progress tracking and improve error handling for log file export process
- feat: import share from LrView in DevelopEditManager
- refactor: standardize EXIF extraction, update UI layout constraints, and modernize style engine test suite
- fix: pass nil headers to LrHttp.get to correctly apply timeout parameter
- refactor: replace pcall with LrTasks.pcall and improve error handling in _request function
- refactor: improve HTTP request robustness with pcall and add AI engine/base profile metadata to the UI
- feat: add trace logging for API request results and status headers
- feat: add camera distribution tracking to training service and display learned cameras in plugin UI, plus add logging safety checks
- fix: add error handling for server log retrieval in copyLogfilesToDesktop task
- feat: reduce CLIP status polling frequency and throttle log output for status changes
- fix: run log file copy operations in asynchronous tasks to prevent UI blocking
- refactor: unify server log collection to exclusively use API-fetched logs with dynamic hostname prefixing
- feat: update tech stack documentation, add Credits wiki page, and improve remote log file naming conventions
- feat: implement remote log collection and error reporting for backend services
- feat: implement style engine tracking and UI dashboard for training profile statistics
- test: update search mock return values, fix edit endpoint payload keys, and mock send_file for database backups
- feat: implement full localization support for UI strings and dialog messages across the plugin
- refactor: update API test mocks to reflect route refactoring, add general project rules, and remove deprecated pcall rule
- feat: concatenate error list into a single string for improved error reporting
- fix: wrap Ollama provider initialization in try-block to prevent startup crashes
- feat: add warning reporting for indexing tasks and improve server health check logic
- feat: add scope selection to training dialog to allow processing of selected photos, current view, or entire catalog
- feat: add training photo validation and display helpful UI hints for edit style learning
- chore: add docker-compose.yml to .gitignore renamer docker-compose.yml to docker-compose-prod.yml
- feat: add user-facing warnings for unindexed or invalid reference photos in similarity search
- feat: add support for saving and applying custom user edit styles via training examples chore: missing translations

## [v2.12.7] - 2026-04-08

- refactor: centralize test configuration, improve log path safety, and update plugin API method names
- refactor: reorganize Info.lua menu structure by moving LrExportMenuItems and adding LrHelpMenuItems
- feat: implement automated testing task and localize UI strings for plugin settings and edit presets chore: add missing translations
- feat: implement unit testing suite with pytest and integrate into CI build workflows
- docs: overhaul wiki documentation and add comprehensive troubleshooting guide
- feat: add warning message in UI when OpenCLIP model is missing
- feat: add is_model_cached helper to check for local model files and update status route to use it
- feat: include detailed error messages in batch processing responses for better debugging
- feat: aggregate and display detailed error reports in task completion dialogs

## [v2.12.6] - 2026-04-06

- Enhance build workflow to support Windows in dependency installation by adding --break-system-packages flag for both macOS and Windows environments.
- [Bug] Server can not be started on Windows Fixes #91

## [v2.12.5] - 2026-04-06

- Update docker-compose.yml to allow overriding the image tag via .env and enhance build workflow to publish a multi-arch Docker manifest for the latest image.
- Update docker-compose.yml to use 'latest' image tag for geniusai-server

## [v2.12.4] - 2026-04-06

- add version tag to docker backends. create two docker-compose files for building and pulling
- [Bug]  Bad Argument #2 Fixes #88
- [Bug] Unknown key "title" when selecting append Meta data Fixes #89
- Issue templates

## [v2.12.3] - 2026-04-04

- Enhance artifact handling in build workflows by adding steps to pack and upload tarred artifacts for both main and plugin-only builds, ensuring better organization and retrieval of generated files.
- Normalize macOS runtime by flattening symlinks in build workflows to ensure portability of uploaded artifacts
- Add torchvision NMS operation validation and update environment variables in build workflows
- Add logic to update Python executable links in workflows for bundled libpython

## [v2.12.2] - 2026-04-04

- Refactor Python environment setup in workflows to improve macOS handling and validate bundled libpython linkage

## [v2.12.1] - 2026-04-04

- don't zip local artifacts
- Disable GitHub cache in local runner build workflow to prevent issues with absolute cache paths
- Refactor macOS libpython bundling logic in build workflows to always vendor dylib and update executable links
- store local artifact for testing with local runner
- update to actions
- re-add action for local runner for testing
- fix artifact
- fix error in build

## [v2.12.0] - 2026-04-04

No changes.

## [v2.11.1] - 2026-04-04

- fix error in build
- fix depreaction warning in action
- further tackling with shrink action in build pipeline
- fix action and Dockerfile: vendored open_clip is history
- fix clip model loading
- - cleanup local open_clip dep - cleanup old model conversion scripts - add script for local uv env - add open_clip_torch to requirements.txt
- fix build pipeline
- add instruction for antigravity
- enable manual run on the dev branch for testing
- Update backend setup and dependencies
- Update README
- update build pipeline add smoke test

## [v2.11.0] - 2026-04-03

- Enhance crop settings handling in DevelopEditManager
- fix auto crop
- Add composition mode functionality for AI editing
- support for ai cropping
- Add edit intent presets and style strength configuration
- make adjustments optional
- point curve support. enhanced prompt
- support for tone curves (parametric and point)
- fix local masks
- fix in editing workflow
- First attempt to implement AI editing
- fix build pipeline  open_clip dep not found
- fix pipeline on windows
- Update dependencies in environment and requirements files for PyTorch, torchvision, and timm; enhance build workflow with additional import checks and validation for model configurations.
- update existing keywords in lightroom with synonyms für bilingual keywords
- fix error in new vertex ai api
- fix remove deprecation warning from vertex ai code. move to new api.
- - fix bad dep in open_clip - fix startup of downloaded backend in macos - try to avoid gatekeeper issues on macos in a dirty way.

## [v2.10.1] - 2026-03-29

- Refactor Windows command execution in build workflow to create a dedicated launch script for improved path handling and command execution.
- Refactor Windows command execution in build workflow to use correct path formatting and command syntax for improved compatibility.
- Update build workflow to improve backend process handling by redirecting output to a log file and ensuring proper process monitoring on all OS environments.
- Refactor build workflow by removing unnecessary pruning of non-runtime files and adding a sanity check for the backend import path. Updated environment.yml to include pytorch and torchvision dependencies.
- Improve build workflow by disabling fail-fast strategy, adding stdout logging for smoke tests, and enhancing backend process monitoring.
- Enhance build workflow by increasing wait time for backend readiness and adding logging for smoke test failures on Windows.
- fix for build pipeline
- fix smoke test on windows  in build
- trying to reduce size of build artifacts
- Refactor build process to create runtime environment directly at target prefix, eliminating conda-pack conflicts. Removed unnecessary unpacking steps in script generation.
- enhance build pipeline to also do test builds on dev pushes.
- fix: error in build pipeline

## [v2.10.0] - 2026-03-29

- update docs to reflect that smartscreen / gatekeeper errors should be gone
- Improve startup performance of backend Fixes #69
- Add bilingual keyword support and secondary language options

## [v2.9.0] - 2026-03-22

- add todo implementation plan for faces-people fix dialog width in people dialog
- make intersect mode default in TaskPeople
- further refactoring of people dialog multi select persons to display simplify dialog intersect of combine mode if multiple persons are selected.
- improve speed of person dialog by lazy loading face thumbnails
- fix dialog size in taskpeople.lua
- change layout of people dialog to grid
- some fixes for People dialog
- Fix progressScope in TaskAnalyzeAndIndex when setting scope to "delta" mode

## [v2.8.2] - 2026-03-21

- replace all pcall calls with LrTasks.pcall which can yield Server does not Start -2.8.1 ERROR	startServer: unexpected error: Yielding is not allowed within a C or metamethod call Fixes #63
- Remove outdated GitHub Actions workflows for building and releasing the LrGeniusAI plugin and server. Update build workflows to use newer action versions for improved functionality and security.

## [v2.8.1] - 2026-03-20

- restart local backend on version mismatch
- 404 Not Found Fixes #58
- fix for Error in generating metadata (2.8.0) Fixes #60
- fix 2.8 photo claiming error Fixes #59
- Button partially obscured Fixes #57
- safety measures in plugin logging function.-

## [v2.8.0] - 2026-03-15

- add clip based similarity algorithm
- add logging to find similar image
- fix bug in find similar images
- Add task for finding similar images based on phash
- deduplicate photo ids in claim photo. (virtual copies in lightroom)
- speedup claim photos by batching db updates
- add loggin for claim photos server-side
- add progressscope to photo claim process
- Fix claim photos task: replace pcall with LrTasks.pcall
- Document breaking change for cross-catalog backend behavior: no deletion of photo data when removed from a catalog. Update README files for plugin and server to reflect new catalog_id management and sync operations.
- fix log error in plugin
- Thread-safety: Different top-level keywords in parallel jobs Fixes #7
- Feature Request append metadata rather than overwriting Fixes #2
- implement general db migration logic in the plugin for breaking changes which require a migration.
- Implement [feature] Cleanup job for geniusai-server database / Deleted photos #6.. trying to make it cross-catalog safe for remote backends.
- Reviewing results – behavior of the “Discard” button  #54
- Reviewing results – behavior of the “Discard” button #54
- Suggestion: Version information in the server log #52

## [v2.7.2] - 2026-03-14

- fix db backup size. backups were backing up backups which lead to exponential backup size growth

## [v2.7.1] - 2026-03-14

- Incomplete keywords in the control dialog Fixes #45

## [v2.7.0] - 2026-03-13

- update docs
- fix setActiveSources in TaskCullPhotos.lua
- fix missing import
- fix in culling: eye_openness
- use uuid instead of localIdentifier in PhotoSelector
- refactor: update LMStudioProvider to use scoped client for chat functionality
- refactor: prepare image using scoped client in LMStudioProvider
- refactor: update LMStudioProvider to use scoped client for model listing
- fix indent error
- fix to change lm studio on the fly in backend
- enable offloading lm studio via base url
- fix for metadata not being applied if "Skip validation from here" is checked.
- fix datetime mismatch cocoa - epoc
- refactor: streamline capture time submission from Lightroom catalog
- refactor: take capture time from catalog via parameter not via exifs EXIF Data - Clarification Question Fixes #44
- add db_dump test script
- fix for EXIF Data - Clarification Question Fixes #44
- update compose and docs
- automated backup
- periodically cluster faces
- fix slow search with vertex ai
- Add functionality to generate hash-based global photo IDs for catalog
- Fix for #43: regenerate_metadata leads to error when there is no pre-existing metadata.
- fix progresscope
- enhance thumbnail workflow
- set timeout waiting for preview to 3 seconds. otherwise it will get slow.
- update to progresscope in indexing
- disable spammy cache hit logging
- Make use of Lightrooms preview in indexing as alternative to doing an export of the original.
- fix db backup finally
- keep db-backups on server as well
- fix in db backup
- keep db-backups on server as well
- fix in db backup
- fix typo
- fix db backup download
- fix typo
- fix db backup download
- add logging to db backup
- add logging to db backup
- fix for error in db backup plugin-side
- fix for error in db backup plugin-side
- enhance culling summary output with near-duplicate group count and preset details
- make use of new /cull endpoint
- update .gitignore
- introduce /cull endpoint
- update .gitignore
- move docker-compose.yml to root
- more cull preset
- move docker-compose.yml to root
- portrait preset for culling
- cull_aesthetics
- versioncheck
- version check between plugin - server
- migrate endpoint refactoring
- delta mode phash
- versioncheck
- version check between plugin - server
- Culling Presets
- make culliing configurable
- migrate endpoint refactoring
- eye aware culling
- face aware culling
- Add culling metadata fields and update schema version
- Add culling functionality for similar photos
- Remove old quality scoring code
- TODO culling
- todo image culling in docs
- docs update
- Add statistics formatting and user feedback for database backup
- Implement database backup functionality and update API endpoints

## [v2.6.2] - 2026-03-07

- another fix for build action
- add download counter
- rename plugin-only artifact to reflect docker dependency
- add plugin only artifact for docker users
- fix github docker repo name must be lowercase
- update build action
- add docker build and publish github action
- update to the latest available cloud ai models
- update to the latest available cloud ai models
- possible fix for Photos not found in database Fixes #19

## [v2.6.1] - 2026-03-07

- add vertexai login support to docker
- logging id migration request
- doc update for id glitch
- fix warning in plugin stable metadata id failed, falling back to partial hash for xxx err=Insufficient metadata for stable photo ID
- doc update partial fix for hashes with dng files fix for ValueError during migration
- offer backend id migration
- update docs
- fix yield error
- add some logging
- fix writing metadata according to datatype
- fix number -> string
- add custom metadata extensions for hashes
- Remove references to quality scoring from documentation and README files for LrGeniusAI plugin.
- Monster commit for docs and wiki
- update docs for google vertexai
- remove build status
- update plugin readme.md
- Enhance README with project details and architecture
- add migration to plugin
- switch server code to be md5 based instead of catalog uuids
- Initialize README with project details and instructions
- switch from catalog photo uuid to md5 hash in plugin
- change build action be tag triggered
- reintegrate vertexai changes to plugin code. got lost during monorepo migration
- Remove matplotlib from excludes

## [v2.5.0] - 2026-03-04

- set plugin's internal version information via github action
- update github actions for monorepo
- Possible fix for Selecting LM Studio results in the error 'str' object has no attribute 'get'  #13
- Cleanup: Prepare /server for vertexai subtree
- Squashed 'server/' content from commit 6ba69b5
- Squashed 'server/' content from commit 6074c41
- Refactor: Move plugin files to /plugin

## [v2.4.0-vertexai] - 2026-02-28

No changes.

## [v2.4.0] - 2026-02-28

- build: set releases to draft
- Put branch name in release notes and name if not main
- build action update
- Add conditional shutdown for backend server based on localhost check. Introduce isBackendOnLocalhost function to determine if the server is running locally before attempting to shut it down, enhancing control over server management.
- Refactor photo indexing to use multipart requests. Introduce _requestMultipart function for handling multipart form data, improving the API interaction for photo analysis and indexing. Update logging for better traceability of tasks.
- Add face detection and similarity search features. Introduce new API endpoints for face detection and querying similar faces. Implement a dialog for selecting faces and creating collections in the library. Update UI and translations for new functionality.
- Implement dynamic backend server URL configuration and enhance Ollama settings in the plugin. Update UI to allow user input for backend server and Ollama base URLs, with appropriate defaults and descriptions in both English and German translations.
- basic support face detection and recognition. fixes for delta runs.
- further cleanup
- major cleanup of unused default values and prefs
- fix zip file creation in action.

## [v2.2.0] - 2026-01-24

- fix installation of pillow in action
- fixhance github actions
- remove fix that was no fix
- improved error handling in CLIP model download
- bump plugin version to 2.2.0
- fix python env in github action to build geniusai_server correctly
- fix: minor bugs
- better error handling. minor fix for clip status check
- Disable generateEmbeddings when CLIP is disabled or not present.
- Make CLIP optional and trigger model download from Lightroom

## [v2.1.3] - 2026-01-20

- add self-hosted running action
- Force reinstallation of pillow in build.
- bump plugin version to 2.1.3
- fix: Update check in background
- fix for github action
- fix: github action create-release
- fix: hopefully final one.

## [v2.1.2] - 2026-01-20

- fix: action
- fix: github action
- Fix github action.
- Move zip file creation. since zip command is not present on windows build machine.
- fix: wrong paths in github action
- Unified build GitHub action
- temp. workaround: Avoid hitting github release file size limits
- Bump plugin version
- Refactor build process for Windows and enhance update check functionality
- Fix macOS build process by adjusting directory structure and adding executable permissions
- Bump version to 2.1.1
- Add build status badge to README

## [2.1.0] - 2026-01-18

- Add permissions for contents write in Create Release job
- Remove build and clean scripts for Windows and macOS
- Remove git tag creation step from build workflow
- Refactor UpdateCheck.checkForNewVersionInBackground to call checkForNewVersion directly
- Remove redundant require statement in UpdateCheck.lua
- Refactor update check logic in UpdateCheck.lua to use GitHub API for version comparison
- Add GitHub Actions workflow for building LrGeniusAI plugin on Windows and macOS
- added Build scripts
- Add project description for LrGeniusAI
- Comment out update check logic in UpdateCheck.lua
- Initial commit