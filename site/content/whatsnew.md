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
