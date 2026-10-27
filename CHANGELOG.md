# Changelog

## Unreleased

- Show the session's token cost in the header, at the model config's `pricing`
  rates: cached prompt tokens at the cache-read rate, the rest at prompt and
  completion rates. A `≥` marks the figure as a floor when history is incomplete.

## 0.3.0 — 2026-10-18

- Fetch all retained request history, default ages to whole minutes, and refresh
  them every 30 seconds or on new entries. Add `a` and `--age-seconds` for seconds.
- Add a Policy view with memory graphs and resize outcomes. Keep policy status
  visible in smaller layouts.
- Read effective settings from a private worker socket. Add growth pause,
  reconciled recovery, and a preview before reloading VRAM settings.
- Add read-only attachment, named profiles, a general setup command, full setup
  checks, and shell completions.
- Account for active parallel slots separately and mark incomplete live totals.
- Bound event queues and UI event processing. Add controller, socket, profile,
  age-refresh, layout, and attachment tests.
- Require release tags to pass the full quality checks. Include docs/examples
  and checksums with binary releases.
- Add per-user JSON settings for GPU selection, VRAM thresholds, growth timing,
  step size, retry cooldown, and themes. CLI options override saved settings.
- Size automatic thresholds from the selected NVIDIA card's measured VRAM.
  Select a card by index or UUID and use it for both Strata and monitoring.
- Add capacity growth mode for busy sessions. The old idle rule can wait
  indefinitely during back-to-back requests; capacity mode counts sustained
  spare VRAM and asks Strata to resize between requests.
- Retry no-op and known engine-busy responses after a cooldown. Keep uncertain
  resize failures paused and show why growth is waiting in the dashboard.
- Add read-only `--diagnose`, config initialization and validation commands,
  example card profiles, and an agent setup guide.
- Move development and release instructions out of the README.

## 0.2.1

- Add Ayu Dark, Rosé Pine, and Everforest themes.
- Fix crate verification in the release workflow.

## 0.2.0

- Publish the Rust dashboard and elastic VRAM supervisor on crates.io.
- Add selectable themes and previews that can run alongside a live session.
- Display live prefill progress and account for driver-reserved VRAM.
- Add release binaries and a Trusted Publishing workflow.
