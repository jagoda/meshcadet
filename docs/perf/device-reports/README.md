# Device report archive

**Currency: 2026-08-02.** This directory is the archive location for
`docs/perf/collection-kit.md` §9's report-back blocks — the mechanical
counterpart of `meshcadet-perf-device-report-ingest`. It is empty as of
this writing: **no device report has been submitted yet.** This README
documents the schema so an ingest run has somewhere defined to land, and so
a human skimming this directory later understands what's here without
reading the ingest crate's source.

## What lands here, and how

A human operator runs the on-device collection kit (`docs/perf/
collection-kit.md`) and pastes the resulting `meshcadet-perf-report`
block(s) back — into a tracking note, this directory, wherever is
convenient. `perf_device_report` (root-workspace crate, `cargo run -p
perf_device_report --bin ingest_device_report -- <path-to-pasted-text>`)
parses that text and writes one file here per report block:

```
<build_ref>--<section>--<payload_bytes>--<ui_load>--<capture_date>.md
```

e.g. `a1b2c3d--baseline--na--na--2026-08-02.md`, or
`a1b2c3d--two-device-delivery--10B--navigating--2026-08-02.md` for one row
of Part G's payload/UI-load sweep. Each archived file's body is the exact
report-back block (header + raw serial log), preceded by one provenance
comment line naming it a MEASURED (device) reading — see `perf_device_
report::archive::render_archive_entry`'s doc for the exact format. **Do
not hand-edit an archived file** — a corrected paste is a re-ingest run
(overwrites the file, appends a fresh `INDEX.md` row), not a manual patch.

`INDEX.md` (generated/appended by the same ingest run, not hand-maintained)
is one row per ingest event: build ref, section, payload/UI-load axis,
capture date, which `docs/perf/ui-perf-baseline.md` §8 predicates that
section can close, and the archived filename.

## Ingesting a real report

```sh
cargo run -p perf_device_report --bin ingest_device_report -- <path-to-file-containing-the-pasted-block(s)>
```

Run from the repo root (the default archive directory, `docs/perf/
device-reports`, is relative to the current directory). This is exactly
what a follow-on `meshcadet-perf-device-report-ingest-2`-shaped effort
does with the numbers once a maintainer reports them — re-run the same
command above against the newly pasted block.

For a `section: calibration` block, the same run also prints which of
`perf_loop_model`'s four calibratable fields (`ui_step`, `cad_spi_
overhead`, `gps_poll`, `battery_poll` — see `perf_loop_model::calibration`'s
module doc for why only these four) it could derive from that capture.
Feeding the derived `MeasuredConstants` into `perf_loop_model::calibrate`
and re-rendering `perf_loop_model::report::render_text_report_with_params`
against the result is what turns a calibration-section report into an
updated `docs/perf/perf-loop-model-baseline.md` baseline — that write-the-
baseline-doc step is intentionally NOT automated by the ingest binary
itself (see plan §6 criterion 6: distinguish a MEASURED input constant from
the still-SIMULATED output the model produces from it, which is a
judgement call about what to write in prose, not a mechanical step).

## Provenance discipline

Every archived file is a real device reading — tag it **MEASURED (device,
`<build_ref>`, `<capture_date>`)** wherever its numbers are quoted outside
this directory (a doc, a tracking note, a commit message). A `perf_loop_model`
sweep re-run against a calibrated point stays **SIMULATED** even when one
or more of its INPUT constants came from here — see `perf_loop_model::
report::render_text_report_with_params`'s own doc for why this crate
refuses to blur that distinction on a caller's behalf.

## Related

- `docs/perf/collection-kit.md` — the procedure that produces the pasted
  block this directory archives.
- `docs/perf/ui-perf-baseline.md` §8 — the deferred-predicate register this
  archive's `INDEX.md` cross-references.
- `docs/perf/perf-loop-model-baseline.md` — the SIMULATED baseline a
  `section: calibration` report re-calibrates.
- `perf_device_report/src/lib.rs` — the crate implementing this schema.
