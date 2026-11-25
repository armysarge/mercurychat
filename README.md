# <img align="absmiddle" src="apps/desktop/public/mercurychat.svg" width="60" height="60" alt="MercuryChat logo" /> MercuryChat

Open-source HF radio messaging for Windows and Linux, powered by the Mercury HF modem.

Founded by ZS6SHZ. Updates: [releases](https://github.com/armysarge/mercurychat/releases).

[![Status: Early alpha](https://img.shields.io/badge/status-early%20alpha-B45309)](ROADMAP.md)
[![Platform: Windows and Linux](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-24657D)](src-tauri/tauri.conf.json)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-31595F)](Cargo.toml)

[![Rust](https://img.shields.io/badge/Rust-backend-A6442E?logo=rust)](https://www.rust-lang.org/)
[![Tauri 2](https://img.shields.io/badge/Tauri%202-desktop-24657D?logo=tauri)](https://v2.tauri.app/)
[![Vue 3](https://img.shields.io/badge/Vue%203-UI-237A57?logo=vuedotjs)](https://vuejs.org/)
[![TypeScript](https://img.shields.io/badge/TypeScript-frontend-3178C6?logo=typescript&logoColor=white)](https://www.typescriptlang.org/)
[![SQLite](https://img.shields.io/badge/SQLite-storage-285771?logo=sqlite)](https://sqlite.org/)

[Roadmap](#roadmap) · [Changelog](CHANGELOG.md) · [Modem lab](lab/README.md) · [CI workflow](.github/workflows/ci.yml) · [Report an issue](https://github.com/armysarge/mercurychat/issues)

> **Early alpha: under active development.** MercuryChat is not ready for on-air use. Nothing is released, interfaces and stored data may change without notice, and it has not been tested on air.

**Status:** the beta features are built and tested against a built-in simulator, real Mercury modems over virtual audio and Hamlib dummy rigs. No radio transmissions have been made, and rig control has not been tried on a real radio. Raspberry Pi testing is still to be done.

## Features

- **Chat** with calls and auto-answer, a saved outbox and delivery receipts from the other station. After a failure, unconfirmed messages are marked uncertain and are not resent by themselves.
- **Discovery:** CQ, opt-in beacons, short broadcast messages and a heard list. No broadcast is ever shown as confirmed received.
- **Files:** the receiver accepts or declines first. Files are compressed, photos are shrunk with their metadata removed, and a file that fails its checksum is not saved.
- **Rig control:** the bundled or your own Hamlib `rigctld`, FLRig, or built-in CAT control with importable command files (Icom IC-7300, Yaesu FT-991A and Kenwood TS-590S/SG included).
- **Station tools:** QSO logging with ADIF and CSV export, QSY proposals, SNR and station information requests, macros, history search and notifications.
- **Setup:** a built-in Mercury modem or your own, a first-run wizard and a diagnostics file.
- **Languages:** English, Spanish, French and Brazilian Portuguese.

## At a glance

![The Operate view in the Midnight theme: heard stations, a connected chat with SIM1 and the QSO record](design/screenshots/operate-dark.png)

![The idle station home: the frequency dial, band buttons, CQ and beacon cards, and SIM1 in the heard list](design/screenshots/station-home.png)

![A connected chat in the Daylight theme, with the chat list and the filled-in QSO record](design/screenshots/operate-light.png)

| QSO record | Logbook |
| --- | --- |
| ![The QSO record for a session with SIM1: frequency, band, RST, name, QTH and notes](design/screenshots/qso-card.png) | ![The Logbook listing a draft record for SIM1, with ADIF export](design/screenshots/logbook.png) |

![Settings, Station tab: station information and what is shared with other stations](design/screenshots/settings-station.png)

![Settings, Rig tab: the external rigctld address and a link test with every step passing](design/screenshots/settings-rig.png)

## Design aim

MercuryChat should feel like the chat apps people already use every day: a conversation list, message bubbles, a composer that is always ready, and clear delivery status. It should not feel like a wall of modem controls. Radio detail is there when you want it and out of the way when you don't. Screens stay calm and uncluttered, readable at a glance, and friendly to newcomers, while experienced operators keep the controls they rely on.

## Roadmap

| When | What |
| --- | --- |
| **Public beta**, mid-December 2026 | Windows and Linux: chat, discovery, files, rig control and logging. First a security and optimization audit and a clean install test on Windows. |
| **1.0**, February 2027 | Encryption and signed messages, on-air field testing, a benchmark against VARA HF and Raspberry Pi support. |
| **After 1.0** | An animated tour, remote station control and a mobile app, mail and relay, a GPS link with a world map and PSK Reporter, other integrations, frequency slots and a user manual. |
| **Later** | Games over the air, such as tic-tac-toe, battleships and chess. |

These dates are targets, not promises. See the [full roadmap](ROADMAP.md).
