---
trigger: always_on
---

Errors and warnings in the backend should be reported to the user via error messages. Checking the log files should be a last ressort.
Always update Dockerfile, both docker-compose files if necessary.
Keep smoke tests aligned with the API of the backend.
All GUI strings should be translated to German and French, via the LOC function of the Lightroom SDK. Keep the TranslatedStrings_*.txt files up to date.