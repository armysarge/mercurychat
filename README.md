# MercuryChat

Open-source radio messaging for Windows and Linux, powered by the Mercury HF modem.

**Status:** M0 (modem-integration proof) closed 15 November 2026, with the Raspberry Pi run carried as an exception ([go/no-go note](docs/M0-go-no-go.md)); M1 (desktop foundation): the foundation is built (station profiles, migrations with backups, data-root and endpoint locks, logs and crash reports, the typed command/event contract, and an in-process simulator modem the window runs on). M1 exits after the Raspberry Pi smoke test and the first-run UI ([M1 progress](docs/M1-progress.md)). A headless core and test client pass the M0 scenario against two real Mercury modems over crossed virtual audio; this is not yet a usable operator app. No radio transmissions have been made. Progress, evidence and open items: [docs/M0-progress.md](docs/M0-progress.md); how to reproduce: [lab/README.md](lab/README.md).

## Decisions confirmed with the owner

- Windows and Ubuntu/Debian-family Linux desktop app with a modern, polished interface.
- Raspberry Pi/Linux ARM64 support is required for Core 1.0; Pi 4/5 with a 64-bit OS are the proposed qualification targets.
- Initial modem tests use two crossed virtual audio routes, with physical PTT disabled.
- VarAC-inspired operating workflows, using Mercury as the modem.
- Open-source distribution.
- Core features first: connected chat, CQ/beacons, heard stations, file transfers and QSO logging.
- Support both managed local Mercury and an existing local or remote Mercury installation.

## Read in this order

1. [Product specification](docs/01-product-specification.md)
2. [Architecture and technology decisions](docs/02-architecture.md)
3. [Mercury integration and proposed application protocol](docs/03-modem-and-protocol.md)
4. [Interface and experience specification](docs/04-ui-specification.md)
5. [Implementation roadmap and acceptance gates](docs/05-implementation-plan.md)
6. [Evidence, risks and open decisions](docs/06-evidence-and-decisions.md)
7. [Virtual audio and Raspberry Pi qualification](docs/07-virtual-audio-and-raspberry-pi.md)
8. [Visual direction](design/mercurychat-concept.html) — open in a browser; illustrative data only, no modem connection.

The technology selection, protocol draft, support matrix and estimates are proposals. The product choices above are confirmed; exact Pi model, memory floor and OS versions remain qualification targets. Implementation milestones are subsequent work; this phase delivers their specification.

## Proposed technical direction

Tauri 2 desktop shell, Vue 3 + TypeScript interface, Rust application services and SQLite persistence. Mercury runs as a separate process with a tested, pinned version. TCP provides modem control, connected data and broadcasts; optional WebSocket telemetry supplies audio health and spectrum information.

MercuryChat-to-MercuryChat interoperability is the initial target. A VARA-like host interface does not make Mercury's waveform compatible with VARA, and it does not establish compatibility with VarAC's application protocol. No VarAC code, assets or proprietary formats are incorporated in this plan.

## Immediate next milestone

M1, the desktop foundation ([plan](docs/M1-plan.md)). It starts with the parts that do not depend on the UI stack. The Raspberry Pi smoke test carried from M0 must pass before M1's UI-heavy work and before M1 exits ([go/no-go note](docs/M0-go-no-go.md)).
