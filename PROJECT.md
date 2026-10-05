# Lumora OS

A from-scratch desktop operating system inspired by macOS and vib-os. Kernel, drivers, desktop shell, and apps are all written from scratch in this repository, entirely with GitHub Copilot in VS Code.

Read this file before generating any code. Keep every suggestion consistent with the decisions below.

---

## 1. Goals

- A bootable, usable desktop OS with a macOS-style experience: menu bar, dock, windows, wallpaper, launcher.
- Everything first-party is written from scratch: kernel, drivers, shell, and apps.
- A real app ecosystem through the Lumora Store.
- Ported browsers: Lumora Chromium (Build 1), Lumora Firefox (later build).
- Lumora Studio, an IDE based on code-oss, running on the Chromium port (Build 2).

## 2. Non-goals

- Linux or Windows binary compatibility (may be revisited later).
- Server, embedded, or mobile use.
- Copying macOS code, assets, or branding. Take inspiration only.

## 3. Technical decisions

Change these here first if they change; Copilot must follow this section.

| Area | Decision |
|---|---|
| Language | Rust (kernel, drivers, shell, apps). Assembly only where unavoidable (boot, context switch). C only for ported third-party software. |
| Architecture | x86_64 first |
| Boot | UEFI |
| Dev and test | QEMU with OVMF, plus VS Code tasks for build and run |
| Kernel style | Monolithic kernel with modular drivers |
| Build system | Cargo workspace with a top-level `xtask` for image building |
| Graphics | Framebuffer first, then a compositor with a GPU-accelerated path later |
| Userspace ABI | POSIX-style syscalls and a libc, required for the browser ports |

## 4. Repository layout

```
lumora/
  PROJECT.md
  boot/          UEFI bootloader
  kernel/        memory, scheduler, syscalls, VFS, networking
  drivers/       framebuffer, keyboard, mouse, storage, network, GPU
  libc/          userspace C library (POSIX layer)
  lumora-ui/     shared UI toolkit (widgets, fonts, theming)
  compositor/    window server, input routing
  shell/         menu bar, dock, desktop, wallpaper
  apps/          first-party apps (one crate per app)
  ports/         third-party ports (chromium, firefox, doom, code-oss)
  xtask/         build, image, and QEMU run tooling
  docs/
```

## 5. Branding and naming

- OS: Lumora OS. Kernel: Lumora Kernel. Desktop shell: Lumora Shell. Dock: Lumora Dock.
- App store: Lumora Store. IDE: Lumora Studio. Browsers: Lumora Chromium, Lumora Firefox.
- Names must be original. Do not reuse Apple or Microsoft app names.

## 6. Design language

- macOS-inspired: translucent top menu bar, centered magnifying dock, rounded windows with traffic-light controls, sidebar-based apps.
- Soft, light-themed identity (lumen/aura). Light and dark mode both supported.
- One shared UI toolkit (`lumora-ui`). Apps never draw their own chrome.
- Wallpapers and the dock are themeable.

## 7. Pre-installed apps

| App | Purpose |
|---|---|
| Atlas | File explorer |
| Lune | Terminal |
| Horizon | App launcher (start menu) with system-wide search that indexes files, apps, settings, and recents |
| Jot | Notepad / text editor |
| Pulse | Task manager |
| Image Viewer | View images |
| Wallpapers | Edits the desktop wallpaper only |
| Settings | System preferences |
| Calculator | Basic and scientific |
| Clock and Calendar | Time, alarms, dates |
| Screenshot | Capture screen or region |
| Media Player | Audio and video playback |
| PDF/Text Viewer | Read-only document viewer |
| Network Manager | Wi-Fi and Ethernet |
| Disk Utility | Partitions and disk info |
| Lumora Store | Install and update apps |

## 8. Lumora Store catalog

- Doom (open-source engine port, with Freedoom assets)
- Lumora Office: Write (documents), Grid (spreadsheets), Deck (presentations)
- Lumora Chromium (Build 1)
- Lumora Firefox (later build)
- Lumora Studio (Build 2)
- More apps over time

## 9. Lumora Studio (IDE)

- Based on code-oss, running on Lumora Chromium.
- Left sidebar: Explorer. On first launch it shows "No folder opened" with an Open folder button. After opening, it shows the folder name and a tree of files and folders.
- Center: code viewer and editor for the selected file, with tabs.
- Right: AI agent panel using bring-your-own-key (BYOK), for building projects and reviewing code.
- Bottom: status bar with language, encoding, and cursor position.

## 10. Build plan

### Build 1: Desktop and Chromium
1. Bootloader and kernel entry; serial logging.
2. Memory management: physical frames, paging, heap.
3. Interrupts, timer, scheduler, threads.
4. Syscalls, user mode, process model, ELF loader.
5. VFS and a disk filesystem; storage driver.
6. Input drivers: keyboard and mouse.
7. Framebuffer driver, font rendering, 2D drawing.
8. Compositor and window manager.
9. Lumora Shell: menu bar, dock, desktop, wallpaper.
10. `lumora-ui` toolkit and the first apps (Atlas, Lune, Jot, Pulse).
11. Networking: NIC driver, TCP/IP, DNS, TLS support.
12. libc and POSIX layer: threads, mutexes, signals, mmap, sockets, poll.
13. GPU or accelerated graphics path.
14. Chromium port: toolchain, platform layer, rendering, input, networking.
15. Horizon, Wallpapers, Settings, Lumora Store, remaining pre-installed apps.

Chromium cannot start until steps 3 to 13 are working.

### Build 2: Lumora Studio
- Port code-oss on top of Lumora Chromium.
- Add the explorer, editor, and BYOK agent panel as described in section 9.

### Later build: Firefox
- Port Firefox and publish it in Lumora Store.

## 11. Coding rules for Copilot

- Write small, focused modules. One responsibility per file.
- No `unsafe` without a `// SAFETY:` comment explaining why it is sound.
- No external crates in the kernel unless listed in `docs/dependencies.md`.
- Every driver and kernel subsystem gets a short doc comment explaining its role.
- Prefer explicit types and clear names over cleverness.
- Never invent hardware registers or addresses. If unsure, leave a `TODO` and ask.
- Log through the kernel logger, never `println!` outside userspace.
- Keep each change small enough to boot and test in QEMU.

## 12. Definition of done (per feature)

- Builds with `cargo xtask build` without warnings.
- Boots and runs in QEMU with `cargo xtask run`.
- Has a short note in `docs/` describing how it works.
- Does not break an earlier milestone.