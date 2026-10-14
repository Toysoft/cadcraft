# Editable vector PDF import

`IMPORTPDF` imports one page of a vector PDF into the current drawing. The result is ordinary
editable CAD geometry that SAVEAS/EXPORT can write as DXF or DWG. It is not a PDF underlay or
an image.

## Desktop workflow

The command is `IMPORTPDF`. `PDFIMPORT` remains a typed alias for existing workflows.

1. File → Import Vector PDF... (`IMPORTPDF`) opens a PDF file picker immediately.
   Open/Import/drop a `.pdf` file also enters the same workflow. Cancelling the picker
   leaves the current drawing unchanged.
2. All pages are analyzed automatically. The overview shows the total page count and a
   thumbnail for each page. Click the page you want; its cached preview is immediately
   available. No separate Analyze action is required. Desktop analysis runs on one
   background worker. Unsupported pages retain their numbers and conversion errors.
3. Select drawing units and the paper scale factor. PDF describes the printed sheet, not the
   original model coordinates: a 1:100 plot imported in metres needs **Metres + factor 100**.
   Millimetres + factor 1 keeps the sheet's physical dimensions. Insertion defaults to (0,0)
   at the rotated CropBox lower-left; rotation defaults to zero degrees.
4. Inspect the geometry preview and any conversion warnings, then Import Vectors. One Undo
   removes the complete import. The new entities are selected and the view fits the drawing.
   Embedded TrueType text becomes vector outlines by default, preserving glyph shapes and
   their PDF spacing. Uncheck Convert embedded text to vector outlines for editable TEXT.
5. Check a known dimension, then use MOVE/ROTATE/SCALE/ALIGN as needed to match survey points.
   Save As `.dxf` or `.dwg`.

Changing pages, units or placement reuses the overview. Changing the text conversion mode
reanalyzes all pages automatically; Reload PDF refreshes a changed source file. A damaged,
raster-only or unsupported page keeps Import disabled; other supported pages remain selectable.

## Programmatic API

```text
importpdf {"path":"plan.pdf","page":2,"units":"m","scale":100,"at":[5,7],"rotation":30}
importpdf {"path":"plan.pdf","page":1,"preview":true}
importpdf {"path":"plan.pdf","preview":true,"pages":true}
```

The browser and scripts without a file system pass base64 `data` and optional `name` instead
of `path`. `preview:true` validates without inserting; optional `thumbnail:true` returns a
base64 PNG geometry preview. Results include page/page count, paper width/height in mm, object
count and warnings; an applied import also returns handles. JSON calls never open dialogs.
`preview:true,pages:true` returns `{pages,previews:[...]}` for the complete PDF without
modifying the drawing. Each preview contains page metadata and a thumbnail or an error.
The overview is limited to 512 pages, 5 million operations and 10 million geometry vertices.

`cadcraft-cli convert plan.pdf plan.dxf` converts page 1 at physical sheet size (mm).
For another page or paper scale, use `cadcraft-cli run --cmd 'importpdf {…}' --save plan.dxf`.
OPEN/IMPORT's programmatic forms also read page 1 by file extension, with default sheet units;
use IMPORTPDF when page choice, scale or conversion warnings matter.

## Supported content and limits

- Line/path/rectangle operators and cubic Bézier operators become LWPOLYLINE, LINE or SPLINE
  entities. Unclipped cubic Béziers preserve all four control points exactly. PDF does not
  retain the original CAD circle, dimension or block semantics; a plotted circle is often
  several Bézier curves, and a plotted dimension is separate paths/text.
- Graphics-state transforms, inherited page resources/boxes/rotation, UserUnit, concatenated
  content streams, Form XObjects and their matrices/BBoxes are respected. CropBox clipping
  and polygon clipping (nonzero or even-odd winding) apply to geometry. Cubics crossing a
  clip are adaptively flattened to chords with 0.005 mm flatness (maximum depth 12).
- Common single-byte fonts and ToUnicode maps become editable TEXT. TEXT uses the drawing's
  Standard font, so its appearance/metrics may differ. Embedded CID TrueType glyphs become
  flattened outline polylines by default (shapes at 1000 units per em, from `cadcraft-fonts`).
  `text:"editable"` chooses TEXT instead. Embedded font data stays in memory during conversion;
  DXF/DWG contains only the resulting geometry, with no embedded PDF font to install.
  Type3 glyph programs, vertical text,
  mirrored text and text clipping are not reproduced. The preview reports these limits.
- RGB/gray/CMYK and basic stroke widths are read; black/white use CAD's contrasting colour 7.
  Filled areas become boundary outlines. Dash patterns become continuous paths. PDF layer
  groups are flattened into `PDF_PAGE_<page>`. Raster images, gradients, soft masks and special
  graphics-state effects are skipped or simplified with explicit warnings. No OCR or tracing.
- A password-protected file needs an unencrypted copy. No PDF scripts, links or embedded
  applications are executed, and no file content is sent to an online converter.
- Input ≤64 MiB; any decoded stream ≤16 MiB; total content decoding ≤64 MiB; ≤500000 PDF
  operations, ≤200000 imported entities, ≤2000000 geometry vertices, ≤10000 pages, nesting ≤64, clipping ≤2048 edges and
  a bounded intersection budget. Coordinates and final placement must be finite within ±1e12.
  Expanded Unicode maps are limited to 8 MiB per font and 32 MiB for the font mapping cache.
  Unsupported filters, overly complex pages and malformed inputs return errors atomically.

Parser: unmodified MIT-licensed `lopdf`, with bounded stream loading/decompression and no
default date/time or parallel features. Dependencies and their versions are in Cargo.lock.
The interpretation code is original Rust based on the public ISO 32000-1 / PDF 1.7 specification.
Tests generate synthetic PDFs in memory, including compressed/object-stream files, transformed
Forms, crop/rotation, clipped paths, text spacing, hostile input and DXF/DWG round trips.
