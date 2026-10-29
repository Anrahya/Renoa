# Third-party material in the Android app

| What | Where in the app | Source | Licence |
|---|---|---|---|
| Instrument Sans (variable) | `app/src/main/res/font/instrument_sans.ttf` | google/fonts `ofl/instrumentsans/InstrumentSans[wdth,wght].ttf` at commit `ddf386ba1b9fbc6b2c818f1ddcd9148f54ced94b` | SIL OFL 1.1, `OFL-InstrumentSans.txt` |
| IBM Plex Mono Regular, Medium | `app/src/main/res/font/ibm_plex_mono_*.ttf` | google/fonts `ofl/ibmplexmono/` at commit `ddf386ba1b9fbc6b2c818f1ddcd9148f54ced94b` | SIL OFL 1.1, `OFL-IBMPlexMono.txt` |
| Phosphor icon paths | `app/src/main/kotlin/live/renoa/app/ui/Primitives.kt` (`Glyphs`) | `@phosphor-icons/react` 2.1.10, `dist/defs/*.es.js` | MIT, `MIT-Phosphor.txt` |
| Discord and Telegram marks | `app/src/main/kotlin/live/renoa/app/ui/BrandMarks.kt` | npm `simple-icons` 16.33.0, `icons/discord.svg` and `icons/telegram.svg`; arc flags spaced for Android's parser | CC0-1.0, no notice required |
| Pele 1.12.0, which draws Mermaid diagrams in replies | `app/src/main/assets/diagram/pele.global.js`, unmodified | npm `pele@1.12.0` (tarball sha1 `fa061823c67e222f3671641ab3ef0442b9030068`), its `dist/pele.global.js` (sha256 `787ed66bb4bd56c15e224109ff5b89ed395ebfacf71fae2c0ccad1f86d6e24ef`); obsidianmd/pele tag `1.12.0`, commit `8aff183be57021b533f319a29341a529be934b08`, by kepano | MIT, with Lucide's ISC notice for one icon, `MIT-Pele.txt` |
