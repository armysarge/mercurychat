# MercuryChat roadmap

MercuryChat is an early alpha. Nothing is released yet and it has not been tested on air. The dates below are targets, not promises.

| Release | Target |
| --- | --- |
| Public beta (Windows and Linux) | Mid-December 2026 |
| 1.0 | February 2027 |

## Built so far

- **Chat:** calls and auto-answer, a saved outbox, delivery receipts from the other station, and recovery that marks unconfirmed messages as uncertain instead of resending them.
- **Discovery:** CQ, beacons, short broadcast messages and the heard list.
- **Station tools:** QSO logging with ADIF and CSV export, macros, history search and desktop notifications.
- **Rig control:** the bundled or your own Hamlib `rigctld`, FLRig, or MercuryChat's own CAT control with importable command files. PTT is cross-checked against the modem.
- **Operator extras:** QSY proposals, SNR and station information requests, and extended beacons.
- **Files:** consent before receiving, compression, photo shrinking and a checksum on every received file.
- **Packaging:** a built-in Mercury modem, a first-run wizard, a diagnostics file, a Windows installer, and `.deb` and Arch packages.

## Before the public beta

- A security and optimization audit.
- A clean install test of the Windows installer.

## Before 1.0

- **Encryption and signed messages**, built right after the beta so that field testing covers them. Encryption is opt-in per contact or group and never on by default. Encrypting amateur radio traffic is illegal in most countries: MercuryChat will say so and require you to acknowledge it, and lawful use is your responsibility.
- **Field testing on air** between two stations with real radios, including longer sessions on links of varying quality.
- **Modem benchmark:** Mercury against VARA HF over the same simulated channels.
- **Raspberry Pi (ARM64)** testing and packages.
- Signed or automatic updates are not built yet and are not yet scheduled.

## After 1.0

In planned order. Each item needs its own protocol proposal and tests before it ships.

1. **Animated tour** of the app for new users.
2. **Network station service:** your station (the desktop app, or a Raspberry Pi with no screen) can be controlled from another device over an encrypted connection. Devices are paired with a QR code and can be revoked. The modem's own ports are never exposed.
3. **MercuryChat Mobile:** an Android app first, then iOS, working as a remote screen for your station. The radio, modem and audio stay with the station.
4. **MercuryMail:** addressed mail that is delivered later, with mailbox limits, expiry and delivery states, kept separate from chat.
5. **Relay:** opt-in passing of messages through other stations, with a hop limit, expiry and receipts.
6. **GPS, world map and spotting:**
   - **GPS device:** connect a GPS receiver for your own station's position.
   - **World map:** the stations you have contacted and heard, with a date picker that starts on today.
   - **Positions:** each callsign is placed from a GPS position when one is known, otherwise from its grid locator. Email and AI gateway stations appear as pins on the map.
   - **PSK Reporter:** report the stations you hear to PSK Reporter. This sends data over the internet, so it will be off until you turn it on.
7. **Other integrations:** logging programs, forms and an email gateway, each as a separate option with its own internet permission.
8. **Frequency slots:** first dial-offset slots set through rig control, later audio slots inside the passband, which need a change to Mercury.
9. **User manual:** a PDF with screenshots generated from the app.

## After everything above

- **Games over the air:** turn-based games between two stations, such as tic-tac-toe, battleships and chess.
