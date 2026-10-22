# Calendar design QA

**Comparison target**

- Source visual truth: `/Users/hxsf/.codex/generated_images/01a06b06-a1b0-7ca2-86e6-986f029fd430/exec-4fb269e1-0232-401c-b148-e8b93710b610.png`
- Rendered implementation: `/tmp/note4c-calendar.png`
- Full-view comparison: `/tmp/note4c-calendar-comparison.png`
- Focused header comparison: `/tmp/note4c-calendar-header-comparison.png`
- Viewport/state: 400×300 px, 2026-09-05 (Saturday), September 2026 month view.
- Density normalization: source 1448×1086 px was resized to 400×300 px; implementation is a native 400×300 framebuffer capture at 1:1 density.

**Findings**

- No actionable P0/P1/P2 differences remain. The implementation preserves the selected information hierarchy, seven aligned columns, white default cells, yellow today cell, red weekend text, one-line header, one-line lunar labels, and rounded-corner safe inset.
- [P3] Bitmap typography is more mechanical than the mock.
  Location: header, weekday labels, and lunar labels.
  Evidence: the mock uses a smooth condensed bold face; the implementation uses the actual firmware 5×7 numeric font and a 16×16 GNU Unifont Chinese subset.
  Impact: the real display looks slightly lighter and more pixel-like, but remains readable at 1:1 size.
  Fix: optional future polish could add a second bold bitmap weight, at the cost of more font data and Canvas commands.

**Required fidelity surfaces**

- Fonts and typography: hierarchy, single-line wrapping and truncation pass; the P3 bitmap-style difference is expected for the embedded renderer.
- Spacing and layout rhythm: 400×300 frame, 10–11 px outer inset, equal 54 px columns, aligned weekday/date tracks and five-row September layout pass.
- Colors and visual tokens: output uses only black, white, yellow and red; semantic today/weekend usage matches the selected visual.
- Image quality and asset fidelity: the design contains no photographic, logo, icon or decorative raster assets. The capture is produced from the actual 2 bpp framebuffer renderer without placeholders.
- Copy and content: `2026-09-05`, `星期六`, `丙午年`, `农历七月廿四`, weekday labels, Gregorian dates and lunar labels match the selected state.

**Comparison history**

- Initial code-rendered preview showed the left header date one scale smaller than the selected visual.
- The date was increased from scale 2 to scale 3 and CJK advance was tightened from 17 px to its native 16 px so the complete right-side lunar header remains on one line.
- The post-fix full-view and focused-header comparisons show no remaining P0/P1/P2 mismatch.

**Implementation checklist**

- Host conversion, Rune package compilation and four-color framebuffer render: passed.
- Physical full-refresh and legibility check: required after flashing because host rendering cannot prove panel appearance.

**Follow-up polish**

- Consider a bold 16×16 Chinese weight only if the physical panel makes the one-pixel strokes look too light.

final result: passed
