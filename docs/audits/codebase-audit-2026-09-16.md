# Memory, simplicity, and interaction review

Reviewed at `fcb009f` on macOS, 2026-09-16. Recommendation: simplify the shell and shared controls, then remove duplicate catalogue data and budget retained rows. Keep the existing Rust/egui architecture and optimistic playback contract.

This is the original audit record. Implementation, measurements and verification
are tracked in [the follow-up record](performance-simplification-work.md).

This is a source and visual review, not a measured performance audit. No release RSS, allocation profile, CPU baseline, or savings were measured. Memory candidates below are deliberately separate from graded findings. No application source was changed.

## Confirmed UI and structural findings

| ID | Severity | Area | Finding | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| C1 | Medium | Shell | Duplicate Settings entry and permanent specialist controls compete with search/navigation | `src/ui/topbar.rs:379`, `:399`, `:411`; captured Home | CONFIRMED |
| C2 | Medium | Interaction | Shared custom controls switch hover/press state abruptly; some badges have no visual feedback | `src/theme.rs:639`, `:703`; `src/ui/topbar.rs:97` | CONFIRMED |
| C3 | Medium | Accessibility | Common targets fall below the project's 44-point minimum | `src/theme.rs:639`; `src/ui/topbar.rs:134`; `src/ui/player_bar.rs:339` | CONFIRMED |
| C4 | Medium | Motion | Reduced-motion behavior is confined to fullscreen lyrics | `src/ui/lyrics.rs:179`, `:495`; `src/app.rs:334` | CONFIRMED |
| C5 | Medium | Settings | Setup instructions and nine large sections make routine settings hard to scan | `src/ui/settings.rs:306`, `:453`, `:802`, `:1524`; captured Settings | CONFIRMED |
| C6 | Low | Branding | Packaged/app logo geometry and website branding have separate sources | `src/theme.rs:679`; `src/util.rs:150`; `packaging/icons/spotidark.svg:1`; `docs/assets/images/logo.svg:1` | CONFIRMED |

Five medium findings and one low finding. Severity describes usability/convention gaps, not measured performance impact.

## System map

`entrypoint` owns window/tray lifecycle. `App` owns catalogue pages, library state, playback intentions and UI state. Views draw and emit `Action`s; the application applies those after drawing. Backend/runtime workers perform network and playback work. Artwork has its own loader and egui decoded-image/texture caches. Audio flows through the player/sink; visualizers consume the post-EQ, pre-volume tap. MilkDrop runs in a child process when used.

## Findings and recommended direction

### C1: Give the shell fewer permanent controls

Settings appears both in the account menu and as a header icon. MilkDrop and the Winamp mini-player occupy two more permanent header positions. The playback device is exposed in both the header and player bar. On a narrow window these compete directly with the search field.

Keep navigation and search in the header. Keep Settings in one predictable location, and put mini-player/visualizer commands in a View menu while retaining shortcuts. Keep one actionable device control near playback; remote-playback status can remain visible without duplicating a full control.

The sidebar adds three Library header controls, four wrapping filter pills, and a separate sort control before its contents (`src/ui/sidebar.rs:509`, `:620`). Use a compact library selector at narrow widths and place sort beside it. Keep the transport, volume, and queue directly accessible. Treat lyrics and specialist tools as secondary actions. Preserve keyboard access and visible focus when reducing idle visual emphasis.

### C2: Refine shared interaction states before adding page animation

The app already has hover tints, pressed icon scaling, focus rings, tooltips, slider thumbs and row highlights. The problem is inconsistency and abrupt transitions. Setting `style.animation_time = 0.12` does not interpolate custom paint branches such as `if response.hovered()`.

`icon_button` instantly changes scale to 0.92 on press; `circle_button` instantly grows to 1.05 on hover. Circle, pill and soft buttons do not share the same pressed treatment. Header badges paint the same fill regardless of hover. Home also conditionally reveals play controls (`src/ui/home.rs:114`), changing available text width.

Extend the existing shared controls with consistent hover, focus, pressed, selected and disabled states. Keep frequent hover feedback nearly imperceptible. Use the standards' 100–160 ms press budget and 125–200 ms small-popover budget; avoid bounce on routine buttons. Keep seek and volume direct, keyboard actions immediate, and transitions interruptible. Reserve space for revealed controls and fade them in without reflowing text.

Use egui's existing animation support, with stable widget IDs, rather than introducing an animation dependency. [egui 0.36 Context documentation](https://docs.rs/egui/0.36.0/egui/struct.Context.html#method.animate_bool_with_time_and_easing) documents timed/eased interpolation and repaint support. Native immediate-mode painting needs frame-cost verification; CSS compositor rules do not literally apply here.

### C3: Separate visible icon size from interaction area

`icon_button` allocates `icon_size + 12`, which puts common controls below 44 points. Navigation uses 32 points and the main transport play button uses 36. Increase layout-owned hit areas while keeping small glyphs. Do not overlap invisible targets to satisfy the number. Fewer permanent controls makes room for accessible targets.

### C4: Make motion preference app-wide

Fullscreen lyrics uses transient `lyrics_reduce_motion`; the side panel always animates its line highlighting. Introduce one persisted preference, using the OS preference where supported and an explicit override. Apply it to shared controls, scrolling, drag shifts and both lyrics presentations. Reduced motion should remove scale/travel/overshoot while retaining useful gentle color or opacity feedback. Do not turn every interaction into a spring.

### C5: Separate everyday settings from setup and specialist tools

The first Settings viewport is dominated by personal Spotify app instructions. Preserve the instructions and the existing direct setup entry point, but expand them when configuration is needed or explicitly requested. Show a compact connection/status summary otherwise.

Group ordinary playback and appearance controls first. Put proxy configuration, skins and visualizer tuning in expandable specialist sections. Start with progressive disclosure in the existing page; a new settings navigation system is not necessary to test the improvement.

### C6: Calibrate the logo optically across all assets

The app and packaged SVG use triangle points `(50,38)`, `(50,90)`, `(94,64)` in a 128-unit square. Its bounding-box center is x=72, but its area centroid is x=64.67, close to the square center of 64. Therefore shifting it left by eight units to center its bounds would not establish optical correctness.

The logo is drawn independently of the play-button glyph alignment. Review optical position and proportion at actual 16/32/64-point sizes, then apply the selected geometry to the UI painter, raster generator and packaged assets together. The website still uses a green circle variant. Geometry/source divergence is confirmed; the desired optical adjustment remains a visual judgment, not a proven coordinate bug.

## Unmeasured memory and CPU candidates

These are source-backed leads, not graded performance findings. Order reflects which experiments are most worthwhile, not measured savings.

| Candidate | Concrete scenario and source | Simplification | Measurement needed |
| --- | --- | --- | --- |
| Retained catalogue rows | `PagedList` saves displaced windows and merges adjacent windows (`src/model.rs:249`, `:323`, `:336`). Page eviction limits page counts, not rows within them (`src/app.rs:5482`). Browsing a large playlist can retain many rows in one page. | Add a retained-row/byte budget, evict cold windows before pages, protect active/playing/pending-edit data. Update the documented window-retention contract. | Release RSS and retained row counts through large-playlist jumps and back navigation. |
| Liked Songs duplication | `sync_view` clones the server list; checkpoint clones it again; writing materializes JSON bytes (`src/liked.rs:212`, `:243`, `:275`). | One canonical collection plus an optimistic-change overlay; stream persistence using existing playlist-cache patterns. Preserve immediate likes/unlikes. | 10,000-row library load, mutation and checkpoint: peak RSS and allocations. |
| Table row duplication | `TableRowsCache` owns full `PlayableItem` graphs while source pages remain (`src/model.rs:9`, `:17`; `src/ui/collection.rs:372`). Two caches are already capped (`src/app.rs:1726`). | Cache indices/display metadata, or share immutable source items where ownership warrants it. Avoid a broad Arc conversion. | Existing retained-byte accounting plus allocator/RSS comparison on large tables. |
| Artwork work in flight | Pending work is outside the 64 MiB retained-art estimate. Bodies are fully buffered before the 8 MiB check, disk files are read in full, and accent decoding lacks the explicit limits used for lyrics backdrops (`src/images.rs:234`, `:258`, `:392`, `:480`). | Bound concurrent fetch/decode work; enforce byte limits while reading; share bounded image decoding. | Peak RSS and outstanding tasks during rapid image-heavy scrolling, including large-image fixtures. |
| Rootlist/metadata ownership | Live rootlist is cloned into the last-good cache (`src/app.rs:1516`); accent and secondary metadata maps are not coupled to page eviction (`:320`, `:1412`, `:5482`). | One account-scoped rootlist owner; bounded secondary caches tied to catalogue lifetime. | Retained counts and allocations after prolonged browsing; account-switch regression checks. |
| Repeated transient work | Recents clones its merged list each draw (`src/ui/queue.rs:314`); visualizers allocate packet/frame buffers (`src/vis.rs:89`, `:120`; `src/milkdrop/shm.rs:100`). | Borrow Recents during rendering and collect actions afterwards; reuse visualizer scratch buffers. | Allocation profile while Recents/visualizers are visible. |
| Hidden-window wakeups | Tray mode calls `background_frame` every 150 ms (`src/entrypoint.rs:657`). | Consider deadline-aware/event-driven waiting while preserving media/tray responsiveness. | Idle CPU/wakeups with paused playback, active playback, and media commands. |

## Design tensions

- **Fast back navigation versus bounded memory:** preserve active data and optimistic edits, but make cold cache retention a budgeted choice. Page-count limits alone do not express memory use.
- **Feature availability versus permanent visual presence:** move occasional features into menus first. Making MilkDrop opt-in is a separate product decision; it already starts its renderer on demand, so removing its button or default build feature does not establish an idle-RAM saving.
- **Central orchestration versus ownership:** start structural simplification with catalogue ownership/budgets. Splitting the large `App` into many files without eliminating duplicate state will not reduce memory or conceptual complexity.
- **Polish versus rendering cost:** animate local interaction state only while transitioning. Avoid introducing continuous repaint loops or a framework rewrite to make the app feel native.

## Considered and rejected

- No demonstrated tray-mode event leak: `background_frame` calls `handle_events`, draining backend events while the window is closed (`src/app.rs:7986`, `:1428`).
- Do not simply slash the artwork budget: it already accounts for decoded images and textures, releases compressed bytes, and documents a prior visible-image reload regression (`src/images.rs:13`, `:150`).
- Do not remove optimistic playback/queue reconciliation. It protects the explicit no-flicker product contract.
- The audio sink already bounds its queue to 12 chunks (`src/sink.rs:30`, `:584`). No evidence justifies replacing it.
- Do not remove accessibility or legacy command aliases for hypothetical savings.
- No SwiftUI/AppKit rewrite is justified by this review. Shared controls and hierarchy can improve within egui.

## Evidence and limits

Inspected production cache/state paths, relevant view code, image and audio lifecycle code, settings and architecture references. This was targeted review across the codebase, not a line-by-line audit of every test, API/auth path, platform module or MilkDrop renderer branch.

The existing debug demo binary reported version 0.8.0 and rendered on Apple M1 Max/OpenGL. It was not rebuilt, so screenshots are evidence of that executable, with source findings independently traced at the reviewed commit. Demo mode does not exercise real Spotify playback. Screenshots show static layout, not verified animation timing or OS window chrome.

Captured Home in dark/light at 1280×800 and 900×650 logical points, plus Settings. Retina PNG dimensions are double. The final matched captures use temporary settings with `{"theme":"dark"}` and `--demo-show light` for light mode:

```sh
target/debug/spotidark --demo --demo-data /tmp/spotidark-audit-demo --demo-shot /tmp/spotidark-audit-home-dark.png --demo-size 1280x800 --demo-shot-delay 2000
target/debug/spotidark --demo --demo-data /tmp/spotidark-audit-demo --demo-show light --demo-shot /tmp/spotidark-audit-home-light.png --demo-size 1280x800 --demo-shot-delay 1200
target/debug/spotidark --demo --demo-data /tmp/spotidark-audit-demo --demo-shot /tmp/spotidark-audit-narrow-dark.png --demo-size 900x650 --demo-shot-delay 1200
target/debug/spotidark --demo --demo-data /tmp/spotidark-audit-demo --demo-show light --demo-shot /tmp/spotidark-audit-narrow-light.png --demo-size 900x650 --demo-shot-delay 1200
```

These temporary captures are review evidence, not a before/after comparison. A redesign implementation needs matching before/after evidence and visual approval under CONTRIBUTING.md. No Linux/Windows runtime testing, production memory benchmark, animation frame analysis, build, lint or test suite was run in this read-only source review. Only this report was added.

## Decisions still open

The recommended first visual scope is shared interaction polish, larger hit areas, optical logo calibration, and removing duplicate/specialist header controls. Sidebar and Settings regrouping can follow as separately reviewable changes. Whether MilkDrop/Winamp should remain prominent product features is a maintainer choice; nothing here authorizes removing them.
