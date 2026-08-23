# Roadmap

This is a practical, user-focused backlog. Items are worked in order unless a
real connection problem makes a later item more urgent.

## Completed

- [x] Saved profiles, secure password storage, and validation
- [x] Advanced FreeRDP settings while retaining a simple default form
- [x] Native Wayland SDL path with input preservation and safe X11 recovery
- [x] Physical-display sizing and optional smart sizing
- [x] Persistent logging, launch results, and detailed status view
- [x] Remmina, `.rdp`, and JSON profile import; password-free `.rdp` export
- [x] Profile clone and search/filter
- [x] DNS/mDNS and TCP RDP-port preflight checks

## Next

- [x] RDP gateway support, including validation and a separate saved gateway password
- [ ] Optional SSH tunnel support for RDP hosts not directly reachable
- [ ] Per-profile keyboard/layout and performance controls (wallpaper, animations, font smoothing)
- [ ] Redirected printers, serial devices, and selected local drives
- [ ] Safer backups: timestamped export and restore from the launcher

## Quality bar

Every change needs validation, logging that excludes secrets, automated tests,
README coverage, and a pushed commit. Experimental compositor-specific behavior
must stay opt-in and have a safe recovery path.
