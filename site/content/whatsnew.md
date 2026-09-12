<!--
  What's new on getmetermaid.com — written for musicians, not developers.

  This is the plain-English release summary the site shows on /updates/ and in
  the home-page "Updates" teaser. Keep the technical, developer-facing record in
  the repo's CHANGELOG.md; this file is only the friendly version.

  Format: one section per release, newest first, headed by

      ## <version> <YYYY-MM-DD>

  followed by a short paragraph and/or a few plain bullets. No jargon, no
  dependency or build detail, and no em-dashes. If a release is purely
  under-the-hood maintenance, leave it out entirely.
-->

# What's new

## 0.6.0 2026-09-12

Hands-free control while you play. You can now start and stop metering with the S key, and toggle Max hold with M, on top of the existing Space to reset between patches. The shortcuts keep working right after you choose an input device, so you can set up once and then keep your hands on your instrument.

Also fixed: in rare cases the Start and Stop buttons could stop responding until you restarted the app.

## 0.5.3 2026-09-10

More under-the-hood housekeeping. This release refreshes the components MeterMaid is built on, including a handful of security fixes, so the app stays current and dependable on macOS, Windows, and Linux. Nothing changes in how MeterMaid looks or works.

## 0.5.2 2026-07-20

You can now buy me a coffee. MeterMaid is free and open source, and it stays that way, but if it has earned a place in your studio there is a tip link in the About dialog and on this site. Entirely optional, and nothing in the app is gated behind it.

## 0.5.1 2026-07-19

Under-the-hood housekeeping. This release updates the components MeterMaid is built on to keep it current, secure, and reliable across macOS, Windows, and Linux. Nothing changes in how the app looks or works.

## 0.5.0 2026-07-03

New spectrum tools for shaping your tone, built around a common guitarist workflow: compare what you are playing against a reference, right on the meter.

- **Max hold** keeps a line at the highest level each frequency has hit until you clear it. Turn it on in the new Spectrum row, play a riff or a burst of noise, and the full picture of your tone stays on screen. It survives stopping and starting, and the Reset button leaves it alone.
- **Freeze ref** saves that held curve (or the live spectrum) as a background reference. Capture one sound, freeze it, then shape another sound against it.
- **Noise guides.** Pick Pink noise or Brown noise from the Guide menu and MeterMaid draws the curve that noise would make at your Target loudness, without you having to play any noise at all. Pink and brown noise are popular EQ references for studio and live guitar tones. The guide moves with your Target setting, so it always shows where your tone should sit.
- The spectrum now marks your **clip ceiling** with a dashed line, and any frequency that crosses it lights up red.
- **Point at the spectrum** to read the exact frequency and level under your cursor, along with the held maximum and reference values when they are showing.

## 0.4.5 2026-07-02

Fixes for Linux and Windows.

- Fixed a Linux bug where a brief audio hiccup could stop metering with a "buffer underrun or overrun" error. These hiccups are harmless and the audio keeps flowing, so MeterMaid now rides through them and keeps metering. It only stops when something is really wrong, like the interface being unplugged.
- Fixed the spacebar Reset shortcut on Linux and Windows. After clicking a button, such as the Target nudge arrows, pressing space could trigger that button again instead of resetting the measurement. Space now always resets while metering.

## 0.4.4 2026-07-01

Fixes for Linux, especially with USB audio interfaces like the Line 6 Helix Stadium.

- MeterMaid previously could not read the 32 bit audio many USB interfaces send on Linux and refused to start. It now handles every common audio format an interface can deliver.
- The device picker no longer lists the same interface several times over.

## 0.4.3 2026-07-01

Reliability fixes, especially for multichannel interfaces.

- Fixed a bug where metering an interface with certain channel counts (6, 10, 12, or 18 inputs) could quietly drift onto the wrong channels while capturing. If you meter a multichannel interface, this update is worth taking. Interfaces with 1, 2, 4, or 8 inputs were not affected.
- The app no longer freezes for a moment while it looks up your devices or starts and stops metering. This was most noticeable with ASIO interfaces on Windows.
- The spectrum's peak line now starts fresh each time you begin metering instead of carrying over from the last session, and the clip light no longer flashes back on right after you reset.

## 0.4.2 2026-06-28

Fixed MeterMaid on older Macs running macOS Catalina (10.15). It now asks for microphone permission on first launch and correctly lists your input devices, and all the controls work as expected.

## 0.4.0 2026-06-27

Multichannel metering on Windows. If you run a multichannel interface like a Line 6 Helix over ASIO, MeterMaid can now read each channel on its own (Ch 1 through 8) instead of just a single mixed signal.

## 0.3.3 2026-06-25

Tap the spacebar to reset the measurement while you are metering, so you can jump straight from one patch to the next without reaching for the mouse.

## 0.3.2 2026-06-25

Fixed a glitch where the spectrum could start out blank when you began metering, until you resized the window.

## 0.3.1 2026-06-25

Added an **About** window and a menu, including a one-click way to check for updates whenever you like.

## 0.3.0 2026-06-25

MeterMaid now keeps itself up to date. When a new version is ready it shows a banner, and you can install it with a single click and pick up right where you left off.

## 0.2.0 2026-06-23

- Plug in or unplug an audio device and it appears in (or drops out of) the list on its own, with no need to reopen the menu.
- Lighter on your computer when you are not metering, and the sample-rate list now only offers rates that actually work with the device you picked.

## 0.1.1 2026-06-22

- Clearer, friendlier messages when something goes wrong, each with a hint on how to fix it.
- **Reset** is now the main button while metering (the one you press between patches), and MeterMaid ships with its own fonts so it looks the same on every computer.

## 0.1.0 2026-06-18

The first release of MeterMaid: loudness metering (Integrated, Short-term, Momentary, and Loudness Range), true-peak, a live frequency spectrum, and a target helper that tells you exactly how much gain to add to hit your level. It also remembers your device and settings between sessions.
