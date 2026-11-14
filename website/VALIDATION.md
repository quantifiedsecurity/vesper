# Website validation — 14 November 2026

**GATE: PASS. No open failures.** Three findings were waived after visual review; their reasons are recorded below.

The site presents the end-state language and ecosystem from the supplied Vesper Language Design & Formal Specification. The public design action links to the specification in the repository.

## Checks

- `npm run build` and `npm run check`: pass locally and in the GitHub Pages workflow. Checks cover local assets, links, unique IDs, tab references, included source tokens and the custom domain.
- Copy lint: **0 failures, 0 warnings**. Voice: the engineer; direct, specific and technical where useful.
- Layout checker: **no findings** at 1440×900, 1024×768, 820×1180, 768×1024 and 390×844, including headline stress checks. Screenshots were visually reviewed. Later visual adjustments preserved the layout; the finish gate also walked 320px for overflow and collisions.
- Full initial consistency review covered all five widths, reduced motion and script-failure cases. Subsequent checks and the final gate covered the corrected desktop and phone layouts, keyboard focus, controls, motion and narrow-screen overflow.
- Final performance review: **0 failures**, two font-family warnings. Transfer was approximately 196 KiB on desktop and 194 KiB on phone; no broken requests. Measured layout shift was 0.005 on desktop and 0.000 on phone. These are local headless-browser measurements, not production benchmarks.
- The three font families are the skill's dealt pairing: Funnel Display for headings, Funnel Sans for text and DM Mono for source. Three font files transferred approximately 50 KiB. This intentional division explains both performance warnings.
- The optional grain layer is disabled through the shader's supported parameter because it requested a broken SVG fragment. Kit source files were not edited.

## Reviewed waivers

| Finding | Reason retained |
| --- | --- |
| `card-layout`, ecosystem group | The four cards form one coherent 2×2 group with four different code fragments and real destinations. The skill permits this feature recipe and a consistent card family; the checker interprets the four sibling cards as four repeated layouts. |
| `copy-headings`, language heading group | Ownership, effect evidence, native execution and source are concrete subjects from the language design. The general-business noun heuristic does not recognize the programming-language vocabulary. |
| `off-grid`, footer logo | The V mark starts on the wrap's left edge. The reported offset is the word “vesper” following that mark and its gap inside the compound logo. The complete logo aligns with the page and header. |

## Non-blocking checker warnings

The gate reported 20 consolidated warnings. They were reviewed in these groups:

- **Navigation:** font bearings around the mark, short navigation nouns paired with descriptive section headlines, and no changing active-section marker. Anchor destinations are correct. The button's chevron accounts for the label-centering observation. The nav and hero intentionally show the same primary action twice.
- **Compiler moment:** pinning, border transitions, crossfades and the inset step list are the dealt `pinned-chapters` behavior. Its static phone, reduced-motion and script-failure arrangements remain readable. This explanatory section intentionally has no extra call to action.
- **Copy and type:** ecosystem headings name application domains; 13px monospaced diagram labels are source and pipeline annotations rather than body paragraphs.
- **Spacing:** full-screen sections create different apparent gaps around the pinned stage, the setup console and the inverse footer. The independent five-width layout check passed. The terminal deliberately retains some unused console space.

## Design and material

Software; cool-grey-cobalt; Funnel Display/Funnel Sans/DM Mono; sharp component radii and pill actions; triptych hero; sky ground with the painterly-landscape shader; pinned-chapters compiler section; dark wordmark close. The cool neutral surface supports source reading and cobalt identifies actions. The navigation uses the centre recipe because no client photographs were supplied for the dealt mega-menu image card.

No invented customers, benchmark claims, releases or assumed facts. No stock photographs, stand-ins, forms, client asset slots or coined name. No missing contact endpoint or canonical domain. Real GPU performance and physical-device rendering were not measured.

## Deployment

GitHub Pages publishes the checked static artifact from `main` via `.github/workflows/website.yml`. Cloudflare provides the four GitHub A records, four AAAA records and the `www` CNAME with proxying disabled. The repository's custom domain is `vesper-lang.org`. Domain, TLS and live HTTP status are verified separately from local checks.
