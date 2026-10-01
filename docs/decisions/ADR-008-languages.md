# ADR-008: Spanish and English, With the System Language as Default

## Status

Accepted

## Date

2026-09-30

## Context

ChronoRift was Spanish only. A user whose Windows is not in Spanish should get the app in English, and the choice has to reach three places: the UI, the text the backend produces (command errors, replay notes), and the installer.

## Decision

- Two languages, `es` and `en`. The setting `language` in `settings.json` is `"es"`, `"en"` or `null` (never chosen). The language in use is `language`, or `systemLanguage` when it is `null`.
- The system language has one source, `i18n::system_language()`: on Windows, `GetUserDefaultUILanguage()` with Spanish as the primary language (any variant: es-MX, es-ES…) gives `es`, anything else gives `en`. Other platforms give `es`. The UI receives it as `systemLanguage` in every settings response (`get_settings` and each `set_*`). It is computed on every read and never written to the file.
- The UI translates its own text. The backend translates what it generates: a global language in `i18n`, because errors become text at the edge of each command (`to_string()`), far from the settings. Only the Tauri `setup` (saved language, or the system's) and `set_language` change it; nothing that tests run does, so tests always see Spanish. A user-visible backend string is written once per language with `tr!(spanish, english)`, which evaluates only the branch in use.
- Text that is stored and shown later is translated when it is shown, not when it is read. `ReplayInspection.notes` are typed (`ReplayNote`) and become text when serialized, so a replay already in the in-memory cache follows a language change. Command errors and the `failures[].message` of a scanned library are produced at call time and keep the language they were produced in until the UI asks again (a rescan after `set_language`).
- Messages that come from the operating system (an `io::Error` inside an error) stay in the language of Windows.
- The installer (NSIS custom template, `src-tauri/windows`) offers Spanish and English. A language dialog opens first, every time, with the system language preselected by the same rule as the app. It is bilingual because it is shown before a language is chosen. `windows/spanish.nsh` and `windows/english.nsh` hold Tauri's own strings and the wizard texts, and both must define every key. Passive mode (`/P`) skips the dialog and silent mode never shows it; both use the language saved in `HKCU\Software\ChronoRift\ChronoRift\Installer Language`, else the system's. The uninstaller reuses that saved language. The installer language is independent of the app language.

## Alternatives Considered

### Error codes from the backend, translated in the UI

Rejected for now. Every error enum would need a code and arguments in the IPC, and the UI a table for each one, while the messages that wrap an OS error still could not be coded. Two languages do not justify that contract.

### A localization crate

Rejected. Two languages, a few dozen strings and `AGENTS.md` asks to prefer existing dependencies.

### A language argument on every command

Rejected. It would thread the language through every error type to the point where it is displayed, to avoid one process-wide value that changes only on an explicit user action.

## Consequences

- A new user-visible backend string must have both branches: `tr!` takes two arguments, so one cannot be forgotten.
- Switching language does not rewrite text the UI already holds. After `set_language`, the UI should rescan the library to refresh failures.
- The installer's Spanish and English files can drift. A missing key makes NSIS warn `LangString "x" is not set in language table`, and the build log is the place to check.
