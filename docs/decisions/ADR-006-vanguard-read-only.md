# ADR-006: Read Riot Vanguard's State, Never Control It

## Status

Accepted

## Date

2026-09-29

## Context

Riot Vanguard's kernel driver `vgk` starts with Windows (`SYSTEM_START`). While it is loaded, starting `League of Legends.exe` from a separate installation fails with `CreateProcess` access denied (os error 5).

Measured on 2026-09-29 with the downloaded `16.18.817.5716` client in `E:\ChronoRift`:

- With Vanguard running, the launch failed both elevated and not elevated. The binary was signed by Riot, the ACLs were correct, and the file had no Mark of the Web. Neither the antivirus nor Code Integrity logged anything. The Vanguard kernel log was written in the same second as each failure.
- After the user chose "Exit Vanguard" from the tray icon, `vgk` and `vgc` were `STOPPED`. The same launch, not elevated, played `LA2-1625390199.rofl`.

ReplayBook documents the same error and disables Vanguard before launching, with the NoMoreVanguard script as a fallback. The usual manual method changes the start type of `vgc` and `vgk` and stops them with `sc` and `net`, which needs admin rights. In June 2026, users reported that a Vanguard update broke it.

## Decision

- Read the `vgk` service state with query-only rights. When the replay has an executable and the driver is running, show "Vanguard activo" next to Reproducir.
- Read it again at startup, when the window gains focus (the user comes back after exiting Vanguard from the tray), and after a failed launch.
- Keep Reproducir enabled: the state is information, not a lock. A launch error while Vanguard is running becomes the detail of that state. When Vanguard stops, the error is cleared and the client shows as ready again.
- Never start, stop, or reconfigure a Windows service, and never ask for elevation.

## Alternatives Considered

### Disable Vanguard from ChronoRift

Rejected. It needs admin rights, changes how a security driver starts, and depends on Vanguard details that Riot changes (the common method was reported broken in June 2026). A failure could leave Vanguard misconfigured for online play. ChronoRift cannot reuse ReplayBook's GPL code either.

### Disable Reproducir while Vanguard is running

Rejected. A false positive, for example a future Vanguard that allows replays, would leave no way to launch.

### Parse `sc query` output

Rejected. It spawns a process, and its labels are localized (`ESTADO` on Spanish Windows).

## Consequences

- The user exits Vanguard from its tray icon before watching a replay. Online play needs Vanguard again, which a Windows restart brings back.
- `windows-sys` 0.61, already in the dependency tree through Tauri, becomes a direct Windows-only dependency with `Win32_System_Services`. No new crate is downloaded.
- Only `vgk` is checked: it is the kernel driver, and the failed launches coincided with its log. `vgc` is the user-mode service.
- A test reads real services with the same rights as the app: `Tcpip`, a kernel driver like `vgk`, is running; an unknown name is missing; and `vgk` can be read without elevation.

## Sources

- [ReplayBook: Issues Playing Replays](https://www.fraxiinus.dev/ReplayBook/troubleshooting/issues-playing-replays/)
- [Riot Support: Uninstalling and Disabling Riot Vanguard](https://support.riotgames.com/en-us/valorant/performance/uninstalling-and-disabling-riot-vanguard)
- [Commands to disable Riot Vanguard, with comments from June 2026](https://gist.github.com/AndrewMast/742ac7e07c37096017e907b0fd8ec7bb)
