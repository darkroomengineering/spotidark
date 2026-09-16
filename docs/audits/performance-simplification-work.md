## Functional DAG

```text
Inputs                    Implementation                 Integration       Verification
Audit + current source ──> Data ownership and budgets ─┐
Motion standards ────────> Controls, motion, settings ─┼─> Docs + evidence ─> Full checks + UI comparison
Logo assets ─────────────> Consistent optical geometry ┘
Baseline demo ───────────> Before captures + baseline ──┘
```

# Performance and simplification work

Baseline: `fcb009f`. The maintainer approved the audit recommendations on 2026-09-16. Work stays on main. No features were removed and no dependencies were added. The existing serde dependency enables shared-pointer serialization without changing cache JSON.

## Completion criteria

- [x] Simplified header, sidebar and Settings preserve feature access and setup focus.
- [x] Shared hover/press/focus feedback, accessible hit areas, stable card layout.
- [x] Persisted global motion preference applies to controls, lyrics and drag movement.
- [x] Optical logo geometry is consistent across app, packaging and website.
- [x] Catalogue retention has row/byte bounds and protects optimistic changes.
- [x] Liked Songs and table data avoid duplicate full metadata ownership.
- [x] Liked persistence streams; rootlist has one canonical owner; secondary caches are bounded.
- [x] Artwork fetch/decode has concurrency and input-size bounds.
- [x] Recents and visualizers avoid recurring full-list/buffer allocations.
- [x] Tray waiting uses correct deadlines/waking, or evidence documents why a proposed replacement is unsafe.
- [x] Focused regressions and independent review pass.
- [x] README/reference documentation describes changed UI and retention behavior.
- [x] Before/after captures: normal/narrow, dark/light, device popup and Settings states; HTML comparison. View/account menu capture limitation is recorded below.
- [x] Full CONTRIBUTING checks and application build pass; limitations are explicit.
- [x] Performance claims use actual baseline/candidate measurements; no invented savings.

## Review coverage

Source changes received focused checks and separate integration review. Artwork
input limits received security review. Final measurements and the full checks
were run separately from implementation.

## Current state

Implementation, integration checks and release captures are complete. Baseline source is archived at `/tmp/spotidark-before-fcb009f`. Release binaries: `/tmp/spotidark-review/spotidark-before` and `spotidark-after`. Playback/queue optimism and settings compatibility are preserved.

The comparison page is `dist/performance-review/index.html`. Each side has 36 matching captures: Home, Settings, Playlist, device popup, empty/loading/error queue, invalid personal setup, and fullscreen lyrics, in both themes and at 1280×800 and 900×650 logical points. The PNGs were inspected at representative states and their dimensions and completion status validated for every capture. Binary hashes and raw measurements accompany the artifact.

All CONTRIBUTING checks passed: launchers, formatting, default/all-feature Clippy with warnings denied, default/all-feature all-target tests, doctests, Rust documentation with warnings denied, and Jekyll. The library suites passed 776 and 786 tests respectively; other binary/integration targets also passed. The optimized demo build and signed macOS app bundle validation passed.

The initial default suite exhausted macOS's file-descriptor limit. Final runs used `ulimit -n 4096`, four test threads and loopback access. Launcher checks use installed GNU install/sed because macOS BSD tools do not support the Linux scripts. Jekyll uses locked gems under `/tmp/spotidark-review/gems`. No lockfile or Nix definition changed.

The existing `credentials::tests::native_store_round_trip` remains ignored. No new tests were skipped and no assertions were weakened to pass. Obsolete full-table-cache implementation tests were replaced by shared/borrowed ownership and cache-generation coverage. UI fixtures now open the approved menus/disclosures, locate controls by accessible bounds, and advance animation time before checking terminal colors; keyboard, focus, action and geometry assertions remain.

Runtime and screenshot coverage is macOS demo only. Real Spotify playback, Windows/Linux runtime, and frame-pacing profiling were not exercised. Native UI automation stalled, so View/account menu appearance and interactive motion were not independently captured; automated interaction and accessibility checks passed. The comparison HTML's file references were validated, but it was not browser-driven end to end.

## Measurement record

Baseline release, demo feature, macOS Apple M1 Max. Three fresh-process Home captures with warm artwork disk cache, 1280×800 logical points, eight-second capture delay: peak RSS median 282.45 MiB, range 281.83–282.77 MiB. This includes screenshot capture and is not a playback or large-library workload. Commands and raw values are in `dist/performance-review/before/measurements.json`.

Final candidate samples were 282.83, 279.53 and 279.97 MiB, median 279.97 MiB. The median is 2.48 MiB (0.9%) lower for this small demo workload. The ranges overlap; this is a limited three-run observation, not a general RAM or latency guarantee.

A scratch counting allocator measures a synthetic 10,000-song Liked Songs workload using real `absorb`, `sync_view`, `checkpoint` and `write` functions. It counts requested live allocations, excluding allocator overhead, GPU memory and unrelated app state. All three baseline runs agreed:

| Stage | Baseline allocated bytes | Candidate |
| --- | ---: | ---: |
| Stored rows | 13,639,304 | 13,879,304 |
| Stored rows and displayed view | 27,278,194 | 13,959,304 |
| View and held checkpoint | 40,917,097 | 14,039,317 |
| Peak while writing checkpoint | 57,695,028 | 14,128,233 |

The exact fixture is retained as `dist/performance-review/benchmark.rs`; raw results are `before/liked-memory.json` and `after/liked-memory.json`. Candidate values are medians of three runs; their range was nine bytes. The fixture is unchanged between builds. Peak requested allocations while writing fell 75.5%, from 55.02 MiB to 13.47 MiB. Canonical storage alone grew slightly due to shared-owner bookkeeping; the savings come from removing duplicate graphs and the serialized JSON buffer. These are workload allocations, not whole-process RAM.

## Resolved design decisions

- Retain the 150 ms tray loop: the detached UI waker is inactive, and this interval currently bounds application command handling while native platform event loops differ. A timing increase would sacrifice responsiveness; a new cross-platform headless wake subsystem is not justified by a measured bottleneck.
- Keep MilkDrop and Winamp installed and available. Simplify their placement, not their availability.
- The default motion preference follows native macOS/Windows settings. Linux has an explicit override; no invented environment-variable convention is treated as a system API.
