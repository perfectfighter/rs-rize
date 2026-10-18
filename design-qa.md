# Desktop interface visual QA

final result: blocked

## Source visual truth

Official source inventory: [UI-REFERENCE-SOURCES.md](docs/UI-REFERENCE-SOURCES.md).
Primary dark Calendar: https://rize-io.s3.us-west-1.amazonaws.com/assets/blog/01-calendar-timeline-click-drag.webp
Source pixels: 1280 × 720. Target desktop viewport: 1280 × 720 CSS pixels at density 1.
Reference state: Day calendar with a right Summary rail and expanded navigation.
The local app uses the user's actual date and data, so screenshot example records are not copied.

## Rendered implementation evidence

Implementation screenshot: unavailable.
The preview launcher failed because its /tmp/sites-previewd mailbox is unavailable.
A Vite preview was started, but the cloud browser could not connect to terminal.local:4173.
The resulting browser error page could not be observed under the browser's URL policy.
No alternate browser-control mechanism or fabricated screenshot was used.
Implementation pixels/density: not measured. Density normalization: not performed.
Full-view comparison: blocked. Focused-region comparison: blocked.
Browser primary interactions and console checks: not completed.

## Implementation and nonvisual checks

- Desktop frame changed to the measured header, sidebar, and footer proportions.
- Navigation grouped by Home, Track, Insights, Work, and Admin.
- Calendar uses Day by default, a right Summary/review rail, and coordinated 90 px/hour geometry.
- Timer uses the reference's large ring and adjacent Current Session/Timeline panels.
- Agent uses the reference's greeting, composer, and prompt structure; existing consent gates remain.
- Work, finance, and settings use compact toolbars, neutral borders, and denser data layouts.
- Entity editors use centered dialogs, with original payloads, retained error drafts, Escape, and focus handling.
- Existing Music state stays mounted across navigation.
- Behavior checks and production build are recorded in the GitHub Windows workflow.

These checks establish functional contracts, not pixel fidelity.

## Findings

- [P1] Rendered comparison unavailable: source and implementation cannot yet be placed together at the same viewport/state. All five fidelity surfaces remain visually unverified.
- [P2] The Agent orb reference has no independently supplied image asset. The reserved image slot remains empty.
- [P2] Official controls for capabilities absent from this app are deliberately omitted. Appearance can be aligned without adding those capabilities, but the entire official application is not reproduced.

## Required fidelity surfaces

- Fonts/typography: Lato and official token measurements applied; wrapping, optical weights, and rasterization unverified.
- Spacing/layout rhythm: source dimensions used; rendered overflow, alignment, and responsive proportions unverified.
- Colors/tokens: public classic dark measurements used; rendered contrast and state balance unverified.
- Image quality/assets: source screenshots kept outside the app; native/library icons preserved; Agent orb unresolved.
- Copy/content: real workspace data retained; unsupported capabilities are not presented as working.

## Comparison history

1. Official screenshots were opened and inspected; the prior marketing-style Home and oversized framing differed from the reference. Source-based structural edits were applied.
2. Same-state implementation capture was attempted and blocked before comparison. No visual iteration is represented as passing.

## Implementation checklist

- Open the Windows build at 1280 × 720, capture Calendar, Timer, Agent, an entity editor, and a finance screen.
- Compare each capture with the corresponding official source at equal density.
- Check the five fidelity surfaces, overflow, focus, errors, and console output; fix actionable P0/P1/P2 differences.
- Supply/approve the actual Agent orb asset or explicitly accept that deviation.
- Change the final result only after rendered comparison passes.
