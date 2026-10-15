# 03 — Engines: Office, HWP, E-books, E-mail

Reverse-engineering spec of Filee's document engines, written so a Rust developer can re-implement them
without reading the C#. All paths are relative to the Filee repository root; `E/` is short for
`src/Filee.Engines/`. Line numbers refer to the snapshot we read.

Area covered: `E/Office/**`, `E/Hwp/**`, `E/Ebooks/**`, `E/Email/**` (about 20 000 lines of C#).

---

## 1. The big picture

Filee has two kinds of document engines:

1. **Wrappers around external programs.** LibreOffice (`soffice`), Pandoc, calibre (`ebook-convert`) and
   rhwp. Each runs as a child process with a timeout.
2. **Built-in, in-process converters.** Most of the code is here. These are hand-written readers and
   writers for OOXML, ODS, CSV, HWPX, EPUB, MOBI, FB2, EML and others.

The key design choice is **one shared document model**, `HDocument` (`E/Hwp/Hwpx/HwpxModel.cs`). Nearly
every document reader produces an `HDocument`, and several writers consume one:

```
                        +-------------------+
 DOCX/DOCM/DOTX/DOTM -->|                   |--> HWPX   (HwpxWriter)      --> rhwp --> PDF / HWP
 TXT, MD             -->|                   |--> DOCX   (DocxWriter)
 XLSX/XLS/ODS/CSV/TSV ->|    HDocument      |--> XHTML  (XhtmlWriter)     --> EPUB / HTMLZ / HTML
 PPTX/PPTM/POTX/PPSX -->|  (HwpxModel.cs)   |--> Markdown (MarkdownWriter)--> MD / TXTZ
 PDF (PdfPig)        -->|                   |--> Plain text (PlainTextWriter)
 HWPX                -->|                   |--> FB2    (Fb2Writer)
 HTML, EPUB, MOBI,   -->|                   |
 AZW3, FB2, HTMLZ,TXTZ->|                   |
 ODT/RTF/RST/TeX (via Pandoc JSON AST) ---->|                   |
                        +-------------------+
```

Spreadsheets have a second, simpler model, `Workbook` (`E/Office/Sheets/Workbook.cs`). It is used for
sheet-to-sheet conversions and is turned into an `HDocument` table (`SheetDocument`) when a sheet must
become a document (HWPX, then PDF).

The route planner (outside this area) picks the cheapest chain of edges. Each engine publishes
`ConversionEdge(from, to, cost)`, where **cost defaults to 10** (`src/Filee.Core/Conversion/IConverter.cs:17`).
Engines tune costs so that built-in routes win and external programs act as fallbacks.

### Engine contract (what every converter here implements)

| Member | Meaning |
|---|---|
| `Id`, `DisplayName` | e.g. `"libreoffice"`, `"LibreOffice"` |
| `MaxParallelism` | 0 = unlimited; LibreOffice 2, rhwp 2, calibre 2 |
| `Edges` | list of `(from, to, cost)` |
| `GetStatus()` | available or not, plus a version or path string; re-locates the binaries |
| `ConvertAsync(step, progress, ct)` | returns the list of output files (usually one) |

A `ConversionStep` gives `InputPath`, `From`, `To`, `WorkDirectory` (a scratch folder for the job),
`Preset` (for example `Preset.Document.PdfA`) and `Output.Allocate(ext, suffix?)`. `Allocate` returns the
final output path, or null when the user cancelled an overwrite. In that case the engine returns `[]`.

Shared infrastructure that this area uses:

- `ProcessRunner.RunAsync(exe, args, timeout, ct, workingDirectory?, environment?, onOutput?)`
  (`E/Infrastructure/ProcessRunner.cs`). It starts the process with no shell (an argv list), captures
  stdout and stderr as UTF-8 and calls `onOutput` for each line. On timeout or cancel it **kills the whole
  process tree** and throws `TimeoutException` (or rethrows the cancel). The default working directory is
  the executable's own folder.
- `ProgressEstimate.Start(progress, typicalDuration)`. It moves a fake progress value forward while a
  silent external program runs.
- `EngineEnvironment` finds programs in this order: **the user's chosen copy**
  (`OwnProgram`), then **Filee's bundled or downloaded copy** (`BundledProgram`), then the **system copy**
  (`SystemProgram`). The system search uses `PATH`, then `/opt/homebrew/bin`, `/usr/local/bin`,
  `/usr/bin`, `/bin` and `/opt/local/bin`. For LibreOffice it also checks `/usr/lib/libreoffice/program`,
  `/usr/lib64/libreoffice/program` and the macOS `.app` paths. The `.exe` suffix is dropped on Unix.

---

## 2. Cost table for this area (how routing decides)

| Engine | Edge | Cost |
|---|---|---|
| ooxml | variant ↔ variant in the same family | 10 |
| spreadsheet | xlsx/xlsm/xltx/xls/ods/csv/tsv → xlsx/ods/csv/tsv | 10 |
| docx-writer | any built-in reader format → docx | **8** (pdf: 8+6) |
| hwpx-writer | any reader format → hwpx | 10 (pdf: 10+6) |
| pandoc | md↔docx/odt/html/rtf, md→txt, rst/tex in and out | 10 |
| pandoc | html↔docx/odt | 14 |
| unhwp | hwp/hwpx → txt, md | 10; → html 14 |
| rhwp | hwp/hwpx → pdf, hwp ↔ hwpx | 10 |
| ebook | book reader → book writer | 10 (→ md 12) |
| ebook | html → md | 14 (fallback when Pandoc is missing) |
| ebook | pdf → cbz | 15 |
| calibre | lit/lrf/chm/pdb/pml/rb/snb/tcr/oeb → epub; epub → mobi/azw3/lit/... | 20 |
| libreoffice | general fallback | **25** |
| libreoffice | txt/html → docx/odt/pdf | 28 |
| libreoffice | hwp/hwpx → docx/odt/rtf (needs H2Orestart) | 31 |
| libreoffice | Writer → pdf when "LibreOffice for documents" is on | 10 |
| libreoffice | Calc → pdf when "LibreOffice for sheets" is on | 10 |
| libreoffice | Impress → pdf when "LibreOffice for slides" is on (**default on**) | **0** |
| libreoffice | hwp/hwpx → pdf when "LibreOffice for HWP" is on | 0 (else 31) |
| email | eml → html, txt, zip | 10 |

`DocumentReaders.PdfPenalty = 6` (`E/Hwp/Hwpx/DocumentReaders.cs:77`). Reading a PDF rebuilds text from
positioned words, so the penalty makes routes that keep the source structure win. For example, HWP → HWPX →
DOCX beats HWP → PDF → DOCX.

Defaults of the settings toggles (`E/Infrastructure/EngineEnvironment.cs:58-67`): `LibreOfficeForSlides =
true`. `LibreOfficeForHwp`, `LibreOfficeForDocuments` and `LibreOfficeForSheets` are `false`.

---

## 3. LibreOffice engine — `E/Office/LibreOfficeConverter.cs`

**Role:** the fallback for everything the built-in engines cannot do (DOC, XLS output, PPT, OpenDocument
text and slides, DOCX → PDF by default, ...). It is also the only reader of rare formats.

### 3.1 Format families (lines 31-45)

| Group | Formats | LibreOffice app |
|---|---|---|
| `Writer` | docx, doc, odt, rtf | writer |
| `Calc` | xlsx, xls, ods, csv | calc |
| `Impress` | pptx, ppt, odp | impress |
| `WriterImport` (read only) | dot, wps, wpd, lwp, abw, pages, sdw | writer |
| `CalcImport` (read only) | et, numbers, sdc | calc |
| `ImpressImport` (read only) | key, dps, sda | impress (but a real `.sda` file is draw) |
| `ImpressGraphics` | cgm | impress, exported like a drawing |
| `Draw` | pub, cdr, vsd, odg, emf, wmf | draw |

`FamilyOf(from, ext)` (line 234) has one special case. The format id `sda` covers both `.sda` (StarDraw,
which opens in Draw) and `.sdd` (StarImpress). The file extension decides. Anything not listed is
`writer`, and that includes `txt` and `html`.

### 3.2 Edges (`BuildEdges`, lines 250-290)

```
Writer        → Writer ∪ {txt, html}            25
Writer        → pdf                             documents ? 10 : 25
Calc          → Calc ∪ {html}                   25
Calc          → pdf                             sheets ? 10 : 25
Impress       → Impress                         25
Impress       → pdf                             slides ? 0 : 25
{txt, html}   → {docx, odt, pdf}                28
WriterImport  → Writer ∪ {pdf, txt, html}       25
CalcImport    → Calc ∪ {pdf, html}              25
ImpressImport → Impress ∪ {pdf}                 25
cgm ∪ Draw    → {pdf, odg, svg, png}            25
if H2Orestart: {hwp, hwpx} → {docx, odt, rtf}   31
               {hwp, hwpx} → pdf                hwpToPdf ? 0 : 31
```
Self-edges (`s == t`) are skipped. The edges are rebuilt on every `GetStatus()` call.

### 3.3 Exact invocation (`RunAsync`, lines 181-199)

```
soffice --headless --norestore --nologo --nodefault --nolockcheck --nofirststartwizard \
        -env:UserInstallation=file:///<DataDir>/libreoffice-profiles/worker-<N> \
        [-env:UNO_JAVA_JFW_JREHOME=file:///<bundled jre>/]   # only when a JRE is bundled (for H2Orestart)
        --convert-to <filter> --outdir <outDir> <input>
```

- The `UserInstallation` value **must be a file URI** (`new Uri(dir).AbsoluteUri`).
- On macOS the JRE path takes `Contents/Home` if that folder exists.
- Timeout: **3 minutes** (line 26). A document that opens a modal dialog would otherwise hang forever.
- `outDir` = `<WorkDirectory>/lo-<8 hex chars>`, a fresh private folder per conversion. soffice always
  writes `<outDir>/<input stem>.<ext>`, so the engine takes **the first non-empty file** in `outDir`
  (line 95). It never builds the expected name. Then it moves that file to `Output.Allocate(to)`.
- If no output appears, the error is `"LibreOffice produced no output (exit N). <stderr>"`.

### 3.4 Filter map (`FilterFor`, lines 203-231)

| Target | `--convert-to` value |
|---|---|
| pdf + PDF/A | `pdf:{family}_pdf_Export:{"SelectPdfVersion":{"type":"long","value":"2"}}` (PDF/A-2b, JSON syntax needs LO 7.4+) |
| pdf | `pdf:{family}_pdf_Export` (writer_pdf_Export, calc_pdf_Export, impress_pdf_Export, draw_pdf_Export) |
| docx | `docx:MS Word 2007 XML` |
| doc | `doc:MS Word 97` |
| odt | `odt` |
| rtf | `rtf:Rich Text Format` |
| txt | `txt:Text (encoded):UTF8` |
| html (calc) | `html:HTML (StarCalc)` |
| html (other) | `html:HTML (StarWriter)` |
| xlsx | `xlsx:Calc MS Excel 2007 XML` |
| xls | `xls:MS Excel 97` |
| ods | `ods` |
| csv | `csv:Text - txt - csv (StarCalc):44,34,76` (separator `,`=44, quote `"`=34, charset 76=UTF-8) |
| pptx | `pptx:Impress MS PowerPoint 2007 XML` |
| ppt | `ppt:MS PowerPoint 97` |
| odp | `odp` |
| odg (impress) | `odg:impress8_draw` |
| odg (other) | `odg:draw8` |
| svg | `svg:{family}_svg_Export` |
| png | `png:{family}_png_Export` |
| anything else | the target id itself |

### 3.5 Concurrency: per-worker profiles (the most important gotcha)

- `Workers = 2` (line 25). A `SemaphoreSlim(2)` plus a `Stack<int>` of free profile numbers `{0, 1}`.
- **Two soffice processes must never share a profile.** If they do, the second fails silently with exit 1.
  Every conversion takes a slot, runs, and returns the slot in a `finally` block.
- Profiles are kept under `<DataDir>/libreoffice-profiles/worker-<N>`. A profile counts as "prepared" when its
  `user/` sub-folder exists.
- **Warm-up** (`WarmUpAsync`, lines 119-160). The first start with a new profile is 2-3 times slower, so
  at app start Filee converts a tiny `warmup.txt` (content `"Filee"`) to PDF once per unprepared profile.
  It works in a temp folder `filee-warmup-<8hex>` and deletes it afterwards. While the warm-up holds one
  slot it takes the next one first, so every round gets a different profile. A user conversion that starts
  meanwhile gets the free profile or waits. `--terminate_after_init` alone was **not** enough to build the
  caches.
- Because each conversion uses its own profile, a LibreOffice window the user already has open is not
  affected.

### 3.6 Locating and H2Orestart detection (lines 296-322)

- Lookup order: own copy → bundled → system (`soffice` / `soffice.exe`).
- HWP support needs the **H2Orestart** extension (GPL-3.0, Java). Filee follows symlinks to the real binary
  and takes the parent of its folder as the install root. It then searches recursively for any entry whose
  name contains `*H2Orestart*` under `share/uno_packages`, `share/extensions`, `Resources/uno_packages` and
  `Resources/extensions`.

---

## 4. Pandoc engine — `E/Office/PandocConverter.cs`

Optional. Filee bundles it on Windows. **Not installed on our dev machine.**

Reader ids (line 15): `md→markdown, html→html, docx→docx, odt→odt, rtf→rtf, rst→rst, tex→latex`.
Writer ids (line 26): `md→gfm, html→html, docx→docx, odt→odt, rtf→rtf, txt→plain, rst→rst, tex→latex,
epub→epub`. Markdown output is **GitHub-flavoured** (`gfm`).

Edges (lines 48-64):
- `{docx, odt, html, rtf} → md`, `md → {docx, odt, html, rtf, txt}`, cost 10
- `html → {docx, odt}` and `{docx, odt} → html`, cost 14 (these overlap with LibreOffice)
- `{rst, tex} → {md, html, docx, odt, rtf, epub, txt, rst, tex}` minus itself, cost 10
- `{md, html, docx, odt} → {rst, tex}`, cost 10
- **No PDF output**, because that would need a TeX engine.

Invocation (lines 80-89):
```
pandoc <input> -f <reader> -t <writer> -o <output>
       [--standalone]                                   # when to ∈ {html, tex}
       [--extract-media=<outdir>/<outstem>_files]       # when to ∈ {md, html, rst, tex}
```
Working directory = **the input file's folder**, so relative image paths resolve. Timeout 2 minutes.
Success test: the output file exists. Otherwise the error is `"Pandoc failed (exit N). <stderr>"`.

Pandoc is also used as a **reader** for the document model: `pandoc <in> -f <reader> -t json
--extract-media=<media> -o <ast.json>`, then `PandocAstReader` (section 8.3).

---

## 5. OOXML variant engine — `E/Office/OoxmlConverter.cs`

**Purpose:** convert between Office "variants" without Office, by rewriting the ZIP package. A variant is
the same package with a different content type for its main part.

Variant table (lines 23-36):

| id | family | main-part content type | macros |
|---|---|---|---|
| docx | word | `application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml` | no |
| docm | word | `application/vnd.ms-word.document.macroEnabled.main+xml` | yes |
| dotx | word | `application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml` | no |
| dotm | word | `application/vnd.ms-word.template.macroEnabledTemplate.main+xml` | yes |
| xlsx | excel | `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml` | no |
| xlsm | excel | `application/vnd.ms-excel.sheet.macroEnabled.main+xml` | yes |
| xltx | excel | `application/vnd.openxmlformats-officedocument.spreadsheetml.template.main+xml` | no |
| pptx | powerpoint | `application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml` | no |
| pptm | powerpoint | `application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml` | yes |
| potx | powerpoint | `application/vnd.openxmlformats-officedocument.presentationml.template.main+xml` | no |
| ppsx | powerpoint | `application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml` | no |

Edges: every pair inside a family, cost 10. (There is no xltm, potm or ppsm.)

Macro content types that must go when the target has no macros (lines 39-49):
`application/vnd.ms-office.vbaProject`, `...vbaProjectSignature`, `...vbaProjectSignatureAgile`,
`...vbaProjectSignatureV3`, `application/vnd.ms-word.vbaData+xml`,
`application/vnd.ms-word.keyMapCustomizations+xml`, `application/vnd.ms-excel.macrosheet+xml`,
`application/vnd.ms-excel.intlmacrosheet+xml`.

Algorithm (`Rewrite`, lines 88-161):
1. Open the source ZIP. Load `[Content_Types].xml`. If it is missing, raise "not an OOXML package".
2. Find the main part: in `_rels/.rels`, the first `Relationship` whose `Type` ends with
   `/officeDocument`. Strip the leading `/` from its `Target`.
3. Find or create the `<Override PartName="/main">` element and set its `ContentType` to the target's type.
4. If the target has no macros:
   - Each entry whose content type is a macro type is "removed". The content type comes from the
     `Override` whose `PartName` matches, case-insensitively, or else from the `Default` for its extension.
   - Each removed part's own rels part (`dir/_rels/name.rels`) is removed too.
   - Drop the `Override`s of the removed parts and the `Default`s whose type is a macro type.
5. Write a new ZIP. Skip removed entries and directory entries. Write the edited `[Content_Types].xml`. If
   something was removed, re-parse **every `.rels`** file and drop internal relationships whose resolved
   target was removed, remembering the dropped ids per owner part. Copy all other entries byte for byte.
6. **Excel only:** if relationships of the workbook part were dropped (macro sheets), open the new ZIP
   again and edit `workbook.xml`:
   - remove `<sheet>` elements whose `r:id` was dropped;
   - for each `<definedName localSheetId=N>`: if sheet N was removed, delete the name; otherwise renumber
     it to the sheet's new index;
   - remove the `activeTab` and `firstSheet` attributes from every `workbookView`.
7. Write to `output + ".tmp"`, then rename. Delete the temp file in `finally`.

Path resolution helper (used in several places): a target that starts with `/` is package-absolute.
Otherwise it is relative to the source part's folder. URL-decode it, then handle `..` and skip `.` and
empty segments.

---

## 6. Spreadsheets — `E/Office/SpreadsheetConverter.cs` + `E/Office/Sheets/*`

### 6.1 Engine

- Sources: `xlsx, xlsm, xltx, xls, ods, csv, tsv`. Targets: `xlsx, ods, csv, tsv`.
- Edges: every source to every target, except itself and except `xlsm/xltx → xlsx`. The OOXML engine
  handles that pair because it keeps the whole package (line 24).
- `Read(path, format)` (line 72): `xls→XlsReader`, `ods→OdsReader`, `csv→CsvFormat.Read`
  (delimiter auto-detected), `tsv→CsvFormat.ReadTsv`, anything else → `XlsxReader`.
- Output to xlsx or ods: one file. Output to **csv or tsv: one file per non-empty sheet**. A sheet is
  non-empty when `UsedRange()` is not null. If every sheet is empty, the first sheet is written anyway.
  With more than one sheet, the suffix is `_<SafeName(sheet)>` (invalid filename characters become `_`;
  an empty name becomes `Sheet`).

### 6.2 `Workbook` model (`Workbook.cs`)

```text
Workbook { sheets: Vec<Worksheet> }
Worksheet {
  name
  rows: BTreeMap<row0, BTreeMap<col0, SheetCell>>   // only cells with a value or visible formatting
  column_widths: Map<col, f64 chars>, default_column_width = 8.43
  row_heights: Map<row, f64 points>                 // custom heights only
  hidden_rows, hidden_columns: Set
  merges: Vec<CellRange>                            // inclusive, 0-based
  frozen_rows: i32                                  // repeated as header on printed pages
  row_breaks: SortedSet<row>                        // a row that starts a new printed page
}
SheetCell {
  text: String          // as displayed, already formatted ("1,234.50", "2026-10-01", "TRUE")
  style: CellStyle
  is_number: bool       // numbers and dates (align right under General)
  value: Option<f64>    // numbers, dates as Excel 1900 serials, booleans as 1/0
  number_format: Option<String>  // Excel code; None = General
  is_boolean: bool
}
CellStyle { bold, italic, underline, strike, font_size: Option<f64>, color, fill: Option<"#RRGGBB">,
            align: General|Left|Center|Right, valign: Bottom|Center|Top, wrap,
            left/right/top/bottom: CellBorder{style: Excel name ("thin", "medium", ...), color} }
has_content = text non-empty || fill set || any border visible
```

`UsedRange()` is the bounding box of the cells that have content, expanded by merges that start inside it.
`CellRange::parse("A1:C3")` strips `$`. Column letters use base 26 with A = 1.

**Invariant:** readers always store dates in the **1900 date system**. A 1904 workbook is shifted by +1462
days for date formats only.

### 6.3 Number formatting (`NumberFormatter.cs`)

- Uses the `ExcelNumberFormat` (MIT) library to render a value with an Excel format code.
- Built-in format ids follow ECMA-376 18.8.30, **except id 14, which is changed to `yyyy-mm-dd`** (ISO)
  instead of the locale's short date. The full table is at lines 13-43.
- General format: an integer below 1e11 prints with no decimals. Otherwise use `G10`. If that would give
  an exponent, use `0.#####E+00`.
- The 1900 leap-year bug: `ToSerial(date)` = the OLE Automation date, minus 1 when the OA value is in [1, 61).
  `FromSerial` is the reverse (+1 for serials in [0, 60)).
- `IsDate(fmt)`: the format shows a date or time, and is not a duration such as `[h]:mm`.

### 6.4 CSV / TSV (`CsvFormat.cs`)

- **Decoding** (`DocumentReaders.DecodeText`): a UTF-16 BOM selects UTF-16. Otherwise try strict UTF-8 and
  strip a BOM. If that fails, decode as **CP949** (Korean Excel). For Western users this fallback is wrong
  (they would want CP1252); see the porting notes.
- **Delimiter detection:** look at the first 4096 characters. Count `,`, `;` and TAB outside quotes (a `"`
  toggles the quoted state). The highest count wins; on a tie, comma wins. TSV always uses TAB.
- **Parser:** RFC 4180. A quote opens a quoted field only when the field is still empty. `""` inside quotes
  is a literal quote. CR, LF and CRLF all end a record. The last record is kept only if it is non-empty.
- **Number detection** (`IsNumber`): length 1-19. No leading zero unless followed by `.` (so `007` stays
  text), and the same rule for `-0`. The value must parse as an invariant float. `1,234` stays text.
- **Column widths:** CSV has none, so each column gets `clamp(max display width + 1, 4, 50)`. CJK and
  full-width characters count as 2.
- **Writing:** only the used range, starting at row 0 and column 0 (not at the range's top-left corner).
  Hidden rows and columns are skipped. A field is quoted when it contains the delimiter, `"`, CR or LF, or
  when it starts or ends with a space. Encoding is **UTF-8 with BOM**, line ends **CRLF**. Every row has the
  same field count. The cell's displayed text is written, not the raw value.

### 6.5 XLSX reader (`XlsxReader.cs`)

1. Find the main part through `_rels/.rels`, defaulting to `xl/workbook.xml`. Read
   `workbookPr/@date1904`.
2. Find the theme, sharedStrings and styles parts through the workbook's relationships, matching the type
   URI suffixes `/theme`, `/sharedStrings` and `/styles`.
3. **Shared strings:** `<si><t>` or the concatenation of `<r><t>` runs. Phonetic runs are skipped.
4. **Styles:**
   - custom `numFmts`;
   - fonts (b/i/u/strike/sz/color; an element with no `val` counts as on);
   - fills (`patternFill` with type none or gray125 means no fill; otherwise `fgColor`, else `bgColor`);
   - borders (`left` or `start`, `right` or `end`, `top`, `bottom`; style name plus colour);
   - `cellXfs`, giving a `(CellStyle, formatCode)` pair per index.
   Colours resolve through `OoxmlColors` (theme slots plus tint, the indexed palette of 64, and rgb ARGB).
5. **Sheets:**
   - Skip `state="hidden"` and `"veryHidden"` sheets, and parts whose root is not `worksheet` (chart and
     dialog sheets).
   - `sheetFormatPr/@defaultColWidth`.
   - Frozen rows come from `pane[@state="frozen"|"frozenSplit"]/@ySplit`.
   - `<col min max width hidden>`: `max` is capped at `min + 16384`. A width of 0 counts as hidden.
   - Rows: use `r` when present, otherwise the previous row + 1. `hidden`, and `ht` only when
     `customHeight` is set. Cells: use the `r` reference, otherwise the previous column + 1.
   - Cell types: `s` (shared string index), `inlineStr`, `str` and `e` (text as-is), `b` (TRUE/FALSE with
     value 1/0), `d` (ISO date to serial), and the default `n` (a number formatted with its format).
   - `mergeCells`, ignoring single-cell merges. `rowBreaks/brk/@id` gives the 0-based row that starts the
     next page.
   - Charts, pictures and conditional formatting are **not read**.

### 6.6 XLSX writer (`XlsxWriter.cs`)

- Builds a minimal package by hand: `[Content_Types].xml`, `_rels/.rels`, `xl/_rels/workbook.xml.rels`
  (sheets `rId1..n`, styles `rIdStyles`), `xl/workbook.xml`, `xl/styles.xml` and
  `xl/worksheets/sheetN.xml`.
- **Text is always written as `inlineStr`**; there is no sharedStrings part. Numbers use `<v>` with
  round-trip formatting (`"R"`). Booleans use `t="b"`. An empty text cell gives `<c r=".." s=".."/>`.
- Style table: xf 0 = default (Calibri 11). Fills 0 and 1 are the required `none` and `gray125`. One xf
  per distinct (CellStyle, numberFormat) pair. Fonts, fills and borders are de-duplicated by their XML
  string. Built-in format ids are used for `0, 0.00, #,##0, #,##0.00, 0%, 0.00%, 0.00E+00, @`. Custom
  formats are numbered from **164**. Colours are written as `FF` + RRGGBB.
- Sheet names (`UniqueNames`): remove `: \ / ? * [ ]`, trim `'` and spaces, use `Sheet<n>` when empty,
  cap at 31 characters, and make unique ignoring case with a ` (2)` style suffix that still fits in 31.
- Frozen pane: `<pane ySplit=N topLeftCell="A{N+1}" activePane="bottomLeft" state="frozen"/>`. The first
  sheet gets `tabSelected="1"`.
- `<cols>`: runs of neighbouring columns with the same width and hidden flag are merged into one element.
- Escaping: remove control characters other than TAB, LF and CR, then XML-escape.

### 6.7 XLS reader (`XlsReader.cs`, `XlsLayout.cs`)

- **Sniffing:** if the file starts with `PK`, it is really an XLSX with an `.xls` name, so `XlsxReader` is
  used. Web apps do this often.
- Cell values come from **ExcelDataReader**, which handles BIFF2-8 and code pages. Number formats: if the
  format index is a known built-in, Filee uses its own table (so dates come out ISO); otherwise it uses
  the format string from the file.
- ExcelDataReader gives **no fonts, fills or borders**, only alignment. XLS cells therefore keep the
  default look.
- `XlsLayout` parses the compound file (CFB/OLE2) itself. It reads the `Workbook` or `Book` stream
  (following the FAT, the DIFAT chain, and the mini stream through the mini FAT), then the BIFF records:
  - `0x002F FILEPASS` means encrypted, so return null and fall back to ExcelDataReader's values;
  - `0x0085 BOUNDSHEET` with `data[5] == 0` is a worksheet; the first 4 bytes are its stream offset;
  - `0x007D COLINFO`: first and last column, width `/256` in characters, hidden if `flags & 1` or width 0;
  - `0x0208 ROW`: row index, height `(u16 at 6 & 0x7FFF) / 20` points, custom if `flags & 0x40`, hidden
    if `flags & 0x20`;
  - a substream starts at BOF (`0x0809`) and ends at EOF (`0x000A`) or at the next BOF.
  Reason: ExcelDataReader reports only widths with the "user set" flag (LibreOffice never sets it) and
  cannot tell custom row heights from automatic ones.
- Error messages: password-protected; "a web page or XML saved with an .xls name" (the file starts with
  `<`, or a UTF-8 BOM followed by `<`); "not an Excel 97-2003 workbook".
- Error cells become `#NULL!`, `#DIV/0!`, `#VALUE!`, `#REF!`, `#NAME?`, `#NUM!` or `#N/A`.

### 6.8 ODS reader and writer (`OdsReader.cs`, `OdsWriter.cs`, `OdsNumberStyles.cs`)

Reader:
- An encrypted file is detected when `META-INF/manifest.xml` contains an `encryption-data` element.
- Sheets come from `content.xml` → `office:body/office:spreadsheet/table:table`. Sheets whose table style
  has `table:display="false"` are skipped (hidden).
- Styles: `styles.xml` plus the automatic styles. Each style is resolved with its parent chain and the
  default style applied. ODF lengths (`cm`, `in`, `pt`, ...) become points.
- Borders: `"0.75pt solid #000000"` maps to an Excel style by width. Below 0.3pt is `hair`, up to 1.1 is
  `thin`, up to 2.1 is `medium`, otherwise `thick`. Dashed, dot-dash and dot-dot-dash patterns get
  `medium*` variants when wider than 1.1pt.
- **Repeats:** `number-columns-repeated` and `number-rows-repeated` are capped at Excel's limits
  (1 048 576 rows and 16 384 columns). Empty-but-styled cells repeated more than **256** times are dropped.
  These are the formatted remainder of the sheet, and expanding them would create millions of cells.
- Covered cells (`table:covered-table-cell`) stay empty. `number-columns-spanned` and
  `number-rows-spanned` become merges.
- Values: `office:value-type` float, percentage or currency → `office:value`; date →
  `office:date-value`; time → `office:time-value` (ISO duration); boolean.
- The displayed text is the cached `text:p` (lines joined with `\n`, and `text:s` gives repeated spaces).
  If there is no cached text, the value is formatted.
- Frozen rows come from `settings.xml`.
- Width conversion: `PointsPerCharacter = 5.25` (7 px of Calibri 11 at 96 dpi).
- `OdsNumberStyles` converts both ways between ODF data styles and Excel format codes: digits, grouping,
  scaling, percent, scientific, currency, literals, colours, positive/negative/zero sections, dates, times
  and durations. Anything else becomes General.

Writer:
- `mimetype` (`application/vnd.oasis.opendocument.spreadsheet`) **first and stored uncompressed**, then
  `content.xml`, `styles.xml`, `settings.xml` (only when there are frozen rows) and
  `META-INF/manifest.xml`. ODF version **1.3**.
- The default cell font is set to **11pt** to match Excel (LibreOffice's default is 10pt).
- Columns are written up to **1024**, because LibreOffice before 7.4 warns beyond that. The columns past
  the used range repeat with the default width.
- Merges: the anchor cell spans, the other cells are covered. Overlapping merges: the first one wins.

### 6.9 Sheets as documents (`E/Hwp/Hwpx/SheetDocument.cs`)

Used for XLSX/CSV/... → HWPX, which then goes to PDF through rhwp.
- Every non-empty sheet becomes one section with one table. With several sheets, each gets a bold 13pt
  title paragraph.
- Caps: **5000 rows and 60 columns**, with a note when the sheet is cut.
- A4 (59528 × 84188 HWPUNIT) with 8 mm margins (2268). The page turns **landscape** when the summed column
  width exceeds the portrait text width.
- Column width in HWPUNIT = `(chars * 8 + 10) * 75`. Everything is scaled to fit the text width ("fit all
  columns"). The font scale is never below 0.5.
- Cells without borders get light grey grid lines (`#D9D9D9`, 0.1 mm).
- Frozen rows (1 to 5 only) repeat as header rows. Excel row breaks start a new table on a new page.

---

## 7. Shared colour handling — `E/Office/Ooxml/OoxmlColors.cs`

- Theme slots in `clrScheme` order: `dk1 lt1 dk2 lt2 accent1-6 hlink folHlink`. The fallback is the Office
  2013+ default theme (`dk2 #44546A`, `accent1 #4472C4`, and so on, lines 18-32).
- Excel's legacy indexed palette of 64 colours (lines 35-45).
- SpreadsheetML colours: `rgb` (ARGB), `theme` + `tint`, `indexed`.
- DrawingML colours: `srgbClr`, `schemeClr`, `sysClr` and others, with the modifiers `lumMod`, `lumOff`,
  `tint` and `shade`. PresentationML `clrMap` aliases (`tx1→dk1`, `bg1→lt1`, ...) are applied through
  `WithMap`.
- The output is always `"#RRGGBB"`.

---

## 8. The document hub — `E/Hwp/Hwpx/*`

### 8.1 `HDocument` model (`HwpxModel.cs`)

Units: **HWPUNIT = 1/7200 inch** for lengths and **1/100 pt** for font sizes. A `None` formatting field
means "inherit from the style".

```text
HDocument { title, sections: Vec<HSection> }
HSection  { page: Option<HPage>, columns: HColumns{count,gap,separator}, headers/footers: Vec<HHeaderFooter{pages: Both|Even|Odd, blocks}>,
            hide_first_header, hide_first_footer, start_page_number: Option<i32>, blocks }
HPage     { width, height, left, right, top, bottom, header, footer, gutter }  // text_width = max(2000, w-l-r-g)
HBlock    = HParagraph | HTable
HParagraph{ format: HParaFormat, mark_format: HCharFormat, heading_level 0..9, list: Option<HListRef{numbering, level, numbered}>,
            page_break_before, column_break_before, columns_change: Option<HColumns>, inlines }
HNumbering{ levels: Vec<{format: "DIGIT"|"LATIN_SMALL"|"HANGUL_SYLLABLE"..., text: "^1." | bullet char, start, bullet}> }
HTable    { rows: Vec<HRow{cells, height, header}>, column_widths | relative_widths, column_count, align, indent,
            cell_margin, borders, page_break_before, caption: Vec<HBlock> }
HCell     { column (-1 = next), row_span, col_span, valign, fill, borders, blocks }
HInline   = HText{text, fmt} | HLineBreak | HTab | HLink{target ("#bookmark" or URL), content} | HBookmark{name}
          | HNote{endnote, blocks} | HField{PageNumber|TotalPages} | HImage{path, width?, height?, anchor?}
          | HTextBox{w,h,anchor,fill,gradient,line,shape,corner_ratio,padding,valign,blocks}
          | HShape{Rectangle|Ellipse|Line, w,h,anchor,fill,gradient,line,corner_ratio,flip_h,flip_v}
HAnchor   { h_rel: PAPER|PAGE|COLUMN|PARA, h_align, h_offset, v_rel, v_align, v_offset, wrap: SQUARE|TOP_AND_BOTTOM|BEHIND_TEXT|IN_FRONT_OF_TEXT, allow_overlap }
HParaFormat { align, left, right, first_line (negative = hanging), before, after, line_spacing{Percent|Fixed|AtLeast, value}, keep_with_next, keep_lines, widow_orphan, tabs }
HCharFormat { bold, italic, underline, strike, superscript, subscript, size, font (Latin), east_asian_font, color, shade, spacing }
HBorder   { style: None|Solid|Dash|Dot|DashDot|Double, width_mm, color }   // Thin = Solid 0.12mm black
```

Unit helpers (`HwpxUnits.cs`): twips × 5, EMU / 127, points × 100, mm × 7200/25.4, px × 75.
`ParseLength` accepts in, cm, mm, pt, pc and emu. `%` counts as a share of a ~150 mm text width. A bare
number counts as px.

### 8.2 Reader registry (`DocumentReaders.cs`)

This is the one place that maps a format id to a reader. Both `HwpxConverter` and `DocxWriterConverter`
take their input formats from it, so adding a reader gives both writers a new input.

| Format | Reader |
|---|---|
| docx, docm, dotx, dotm | `DocxReader` (direct OOXML: sections, columns, headers and footers, text boxes, floating pictures, tables with merges, lists, notes, bookmarks, TOC links) |
| txt | `TextDocument`: **one paragraph per line, verbatim** |
| md | `MarkdownReader` (Markdig + advanced extensions; YAML `title` becomes the document title) |
| xlsx, xlsm, xltx, csv, tsv, xls, ods | `SheetDocument.Build(workbook)` |
| pptx, pptm, potx, ppsx | `PptxReader`: one page per slide, objects floating at their positions; inheritance chain paragraph → shape list style → layout → master → master text styles → presentation default |
| pdf | `PdfDocumentReader` (PdfPig, outside this area) |
| hwpx | `HwpxReader` (HWP arrives as HWPX through rhwp first) |
| html | `HtmlReader` (AngleSharp plus a small CSS subset in `HtmlCss`) |
| epub, mobi, azw3, azw, prc, fb2, htmlz, txtz | the e-book readers (section 10) |
| odt, rtf, rst, tex | **through Pandoc's JSON AST** (`PandocAstReader`), only when Pandoc is installed |

`ReadAsync` creates a fresh media folder `<WorkDirectory>/media-<guid>` for extracted pictures. Pandoc
image paths are resolved as absolute paths first, then relative to the input's folder. URLs are dropped.

### 8.3 Writers from `HDocument`

- **HwpxWriter** (`HwpxWriter*.cs`). It starts from an embedded **`blank.hwpx` template**, taken from
  pypandoc-hwpx (MIT), and copies every entry of the template.
  - `mimetype` (`application/hwp+zip`) is written **first and stored**.
  - `Contents/section0.xml` is replaced by `section0..N-1.xml`.
  - `Contents/header.xml` is regenerated: fonts, charPr, paraPr, numberings, borderFills and tab stops,
    created on demand and de-duplicated.
  - `Contents/content.hpf` gets the title plus manifest and spine items for the extra sections and images.
  - `META-INF/container.rdf` gets `hasPart` entries.
  - `Preview/PrvText.txt` holds the first 1000 characters of the text.
  - Images go to `BinData/<id>.<ext>`. Only png, jpg, gif and bmp are kept; anything else is converted to
    PNG with ImageMagick.
  - Output goes to a `.tmp` file, then is renamed.
  - **Quirk:** 한글 (Hangul) stores paragraph margins, line spacing and tab positions twice: in `hp:case`
    (with the HwpUnitChar namespace, true HWPUNIT) and in `hp:default`, where **lengths are doubled**
    (percentages are not). Readers must take this into account (`HwpxReader.Header.cs` header comment).
  - Page setup (`secPr`) and columns (`colPr`) live in the first run of a section's first paragraph.
  - `HwpxWriter.Layout.cs` roughly measures text to fill layout caches (row heights, tab widths) for
    simple readers.
- **DocxWriter** (`Docx/DocxWriter*.cs`). Hand-written WordprocessingML. Child elements must be in
  **schema order** (CT_PPr, CT_RPr, CT_SectPr, CT_TblPr, CT_TcPr), because Word refuses files where they
  are not. A section's `sectPr` goes in the last paragraph of that section, and for the last section as
  the last child of `w:body`. Mid-page column changes become continuous section breaks. Shapes use
  `wps` inside `mc:AlternateContent`. Every drawing gets a document-wide unique `docPr` id.
- **XhtmlWriter**. Clean XHTML plus one fixed CSS (`XhtmlWriter.cs:41-58`). It can split chapters at
  level-1 headings, page breaks, or once a chapter passes **150 000 characters** (cut at the next plain
  paragraph). Links to bookmarks in other chapters are rewritten to the right file.
- **MarkdownWriter**: GFM tables, `~sub~`, `^sup^`, footnotes.
- **PlainTextWriter**: paragraphs separated by a blank line, list markers written out, table rows
  tab-separated, footnotes at the end.
- **Fb2Writer**: section 10.5.

### 8.4 The built-in engines on top

- `HwpxConverter` (`hwpx-writer`): every reader format except hwpx → hwpx, cost 10 (pdf 16). Pandoc
  formats appear only when Pandoc is found.
- `DocxWriterConverter` (`docx-writer`): built-in reader formats except the Word ones → docx, **cost 8**,
  so it wins over Pandoc (10) and LibreOffice (25).

---

## 9. HWP / HWPX pipeline — `E/Hwp/*`

HWP is the Korean Hancom 한글 binary format. HWPX is its ZIP + XML successor (OWPML, KS X 6101). Filee has
four engines for them:

| Engine | Edges | How |
|---|---|---|
| **rhwp** (`RhwpConverter.cs`) | hwp→pdf, hwpx→pdf, hwp↔hwpx (cost 10) | external CLI, bundled under `engines/rhwp` (MIT, github.com/edwardkim/rhwp) |
| **unhwp** (`UnhwpConverter.cs`) | hwp/hwpx → txt, md (10), html (14) | native library through NuGet; x64 only (plus macOS arm64) |
| **hwpx-writer** | anything → hwpx | built in; then rhwp `convert` makes HWP |
| **libreoffice + H2Orestart** | hwp/hwpx → docx/odt/rtf (31), pdf (0 or 31) | Java extension |

Plus `HwpxReader`, which reads HWPX into `HDocument` (hwpx → docx/html/epub...). HWP reaches it as
hwp →(rhwp)→ hwpx →(reader).

### 9.1 rhwp invocations (lines 39-44 of RhwpConverter.cs)

```
rhwp export-pdf <input> -o <tmp.pdf> [--equation-font "Cambria Math"]
rhwp export-hwpx <in.hwp> <tmp.hwpx>
rhwp convert <in.hwpx> <tmp.hwp>
```
- Temp output: `<WorkDirectory>/rhwp-<guid>.<to>`. Timeout 2 minutes. Success means the file exists and
  is non-empty. Then it is moved to the allocated output.
- Lookup: **only Filee's bundled copy** (`engines/rhwp/rhwp[.exe]`), never the system.
- `MaxParallelism = 2`.

### 9.2 PDF pre-processing (`RhwpInput.cs`), clever and fragile

Before `export-pdf`, Filee prepares a patched **copy** (`PrepareAsync`, line 67). The user's file is never
touched:
1. If the input is HWP, first run `rhwp export-hwpx` to a temp HWPX. If that fails, render the original.
2. Load `Contents/header.xml` and all `Contents/section*.xml`. Whitespace is significant inside `hp:t`.
3. **Fonts** (`RhwpFonts.Plan`). Collect up to 200 000 characters of `t` text. For each
   `fontface[@lang]/font[@face]`:
   - Language detection: Hangul means Korean. Kana means Japanese. Otherwise, Simplified versus
     Traditional Chinese is decided by counting characters from two fixed 50-character lists.
   - Korean fonts in a Chinese or Japanese document are replaced with the first installed font of the
     same style (serif or sans) from a per-script preference list (lines 31-42; for example zh-Hans serif:
     SimSun, NSimSun, Noto Serif SC, ...).
   - Fonts rhwp cannot embed (**CFF2 variable fonts**, which made blank pages) are always replaced.
   - Text with no Hangul, Han or kana (Russian, Greek, Arabic, Thai): pick a font of the same style that
     covers every character the current font covers, and more.
   - Fonts are looked up through SkiaSharp / fontconfig. On Linux, fontconfig always returns *some* font,
     so only an exact name match counts.
4. **Shape sizes** (`FixShapeSizes`, line 133). For `rect, ellipse, arc, polygon, curve, line,
   connectLine, container, pic, ole, textart`: if `hp:sz` has `widthRelTo` or `heightRelTo` other than
   `ABSOLUTE` and `hp:curSz` has positive width and height, copy curSz into sz and set both RelTo
   attributes to `ABSOLUTE`. rhwp treated relative values as absolute, which made boxes tiny.
5. **Equation scripts** (`NormaliseEquation`, line 175). In each `equation/script`, outside `"..."`
   quotes:
   - split identifiers that have a known command glued on (`fCIRCf` → `f CIRC f`, `lek` → `le k`). There
     are upper-case and lower-case command lists, plus a `Whole` list that is never split (`left`,
     `leq`, `inf`, `INFTY`, ...);
   - replace operators: `!=` → ` NEQ `, `<=` → ` LEQ `, `>=` → ` GEQ `, `+-` → ` PLUSMINUS `,
     `-+` → ` MINUSPLUS `.
6. If nothing changed, return null and render the original. Otherwise copy the HWPX and replace the
   changed entries.
7. `--equation-font "Cambria Math"` is added only when none of Latin Modern Math, STIX Two Math or Arial
   Unicode MS is installed and Cambria Math is.

### 9.3 Unhwp

`UnhwpDocument.ParseFile(path)` gives `ToText()`, `ToMarkdown({IncludeFrontmatter:false})`, or HTML
(Markdown → Markdig → `MarkdownConverter.HtmlPage(title, html)`). Files are written as UTF-8; **TXT gets a
BOM**, MD and HTML do not.

### 9.4 HwpxReader

It matches elements **by local name** (any prefix, any namespace version) and resolves `hp:switch`
blocks. Every `idRef` is looked up in `header.xml`. Shapes have no inheritance, so each id maps to one
complete format. Tables sit inside runs in HWPX; the reader splits the paragraph around them. OLE objects,
charts, video, form controls and free-form shapes are skipped (and counted).

---

## 10. E-books — `E/Ebooks/*`

### 10.1 Engine edges (`EbookConverter.cs:35-47`)

- Book readers `epub, mobi, azw3, azw, prc, fb2, htmlz, txtz` → writers `epub, hwpx, txt, html, md,
  fb2, htmlz, txtz`, cost 10 (→ md 12).
- `html, md, txt, docx` → `epub, fb2, htmlz, txtz`, cost 10.
- `html → txt` (10) and `html → md` (14, the fallback when Pandoc is missing).
- Comics `cbz, cbr, cb7, cbt, cbc` → `pdf`, `epub` (fixed layout), and `cbz` (except from cbz).
- `pdf → cbz` (15, rendered with PDFium) and `azw4 → pdf` (the embedded PDF is extracted).

### 10.2 `Book` model (`Book.cs`)

`Book { document: HDocument, metadata: BookMetadata { title, authors, language (BCP-47), description,
publisher, cover_image path } }`

- `DisplayTitle`: metadata title, else the document title, else the first heading (max 200 characters),
  else the file name.
- `DisplayLanguage`: metadata, else a guess from the first 2000 text runs. Count Hangul, kana, Han and
  ASCII letters. Pick ko, then ja, then zh, whichever first has a nonzero count with `count*3 >= latin`.
  Otherwise en.
- `EbookFiles.Extract(zip, folder)` is a **zip-slip-safe** extraction: entries with `..`, absolute paths
  or `:` that would escape the folder are skipped.
- `EbookFiles.CommonImage`: JPEG and PNG (plus GIF and SVG when allowed) are kept. Others are converted
  with ImageMagick at quality 90: to JPEG for opaque webp, avif, jxl or tiff, otherwise to PNG.
- DRM message (exact text): *"This book is protected with DRM (copy protection), so it cannot be
  converted. Filee does not remove DRM; open it in the reading app it was bought for."*

### 10.3 EPUB reader (`EpubReader.cs`, `Opf.cs`, `ChapterReader.cs`)

1. **DRM check:** `META-INF/sinf.xml` (Apple FairPlay) means refuse. In `META-INF/encryption.xml`, any
   `EncryptionMethod/@Algorithm` other than font obfuscation (`http://www.idpf.org/2008/embedding` or
   `http://ns.adobe.com/pdf/enc#RC`) means refuse.
2. Extract safely. Read `META-INF/container.xml` and take the first `rootfile` with media type
   `application/oebps-package+xml`. If the container is broken, use the first `*.opf` anywhere.
3. OPF: matched by local name. XML is loaded with **DTDs ignored and no resolver** (no XXE). Manifest
   hrefs are URL-decoded and resolved safely. The spine includes non-linear items (as calibre does).
   Metadata: `title`, `language`, `description`, `publisher`, and `creator` with role `aut` or none.
   Cover: the EPUB 3 `properties="cover-image"`, else the EPUB 2 `<meta name="cover" content=id>`.
4. Chapters: spine items of type xhtml, html or xml. Image items get a generated one-image XHTML page.
5. `ChapterReader`: every chapter is read with `HtmlReader` into **one section**. Each chapter starts with
   a page break and a bookmark `c{n}`. Element ids become bookmarks `c{n}-{id}`, with characters outside
   `[A-Za-z0-9_-]` replaced by `_`. Links between chapters (`ch2.xhtml#x`) become `#c2-x`. URLs with a
   scheme are kept, `javascript:` links are dropped, and links to unknown files are dropped. Unused
   bookmarks are pruned.

### 10.4 EPUB writer (`EpubWriter.cs`)

Output is EPUB 3 plus an EPUB 2 NCX:
```
mimetype                (stored, "application/epub+zip", first)
META-INF/container.xml  → OEBPS/content.opf
OEBPS/content.opf       version 3.0, dc:identifier urn:uuid:<random>, title, language, creators, publisher,
                        description, dcterms:modified, <meta name="cover" content="imgN"/>,
                        fixed layout: rendition:layout=pre-paginated, rendition:spread=auto
OEBPS/nav.xhtml         <nav epub:type="toc">, title "목차"/"目录"/"目次"/"Contents" by language
OEBPS/toc.ncx
OEBPS/style.css         XhtmlWriter.Css
OEBPS/cover.xhtml       only if the cover picture is not already shown in the content
OEBPS/chNNN.xhtml       chapters (ch001.xhtml ...)
OEBPS/images/imgNNNN.ext  stored (no compression)
```
- TOC: the two highest heading levels used. If there are no headings, one entry per chapter labelled with
  its first words. There is always at least one entry.
- Fixed layout (comics): one `pNNNN.xhtml` per picture with `<meta name="viewport" content="width=W,
  height=H">`. The first picture is the cover.

### 10.5 Other book formats

- **MOBI / AZW / AZW3 / PRC** (`Mobi/*.cs`), written from the MobileRead wiki:
  - **Refused up front:** KFX (starts with `CONT` or `\xEADRMION`), Topaz (`TPZ`), and anything whose PDB
    type/creator is not `BOOKMOBI` or `TEXtREAd`.
  - PalmDB: a 78-byte header, record count (u16 BE) at offset 76, then 8-byte entries holding a u32 BE
    offset. Offsets must be increasing. The name is the first 32 bytes in Latin-1, and type/creator are
    the 8 bytes at offset 60.
  - `TEXtREAd` is plain PalmDOC text: compression is the u16 at record 0 offset 0, the record count is
    the u16 at offset 8, and the result goes through `PlainText.ToDocument`.
  - MOBI header (record 0, all big-endian): compression u16@0 (1 = none, 2 = PalmDOC, 17480 = HUFF/CDIC),
    text length u32@4, text records u16@8, **encryption u16@12 (nonzero means DRM)**, `"MOBI"` magic at 16,
    header length u32@20, code page u32@28 (65001 = UTF-8, else cp1252), version u32@36, full-name
    offset/length @84/@88, locale @92, first image record @108, HUFF record and count @112/@116, EXTH flag
    `u32@128 & 0x40`, extra flags u16@0xF2 (header length ≥ 0xE4), FDST @0xC0, fragment @0xF8,
    skeleton @0xFC (KF8 only; a MOBI 6 file reuses 0xC0).
  - EXTH records: 100 author (split on `&`), 101 publisher, 103 description, 121 KF8 boundary, 201 cover
    offset, 503 title, 524 language.
  - Trailing entries per text record (`TrailingSize`): for each set bit above bit 0 of the extra flags,
    read a backward varint from the end. Bit 0 adds `(last byte & 3) + 1` (multibyte overlap).
  - PalmDOC LZ77: byte 1-8 = that many literal bytes; 0 and 9-0x7F = literal; 0xC0+ = a space plus
    `byte ^ 0x80`; 0x80-0xBF = a 2-byte pair with 11-bit distance `(pair >> 3) & 0x7FF` and length
    `(pair & 7) + 3`. Damaged pairs are skipped, not fatal.
  - HUFF/CDIC: a canonical Huffman code over a phrase dictionary; phrases can be recursively compressed.
  - MOBI 6: one HTML document. `filepos=` links become `<a id="fileposN">` anchors (Filee works on a
    Latin-1 string so byte offsets index characters). `recindex` refers to image records.
  - KF8: SKEL and FRAG INDX tables reassemble the files; `kindle:pos`, `kindle:embed` and `kindle:flow`
    are resolved. A joint MOBI6+KF8 file uses the KF8 half (EXTH 121); images are numbered from the MOBI 6
    header.
  - **AZW4 (Print Replica):** decompress the text. If it starts with `%MOP`, read the table and take the
    first (offset, length). Otherwise take everything from the first `%PDF-`.
- **FB2** (`Fb2Reader.cs`, `Fb2Writer.cs`): XML. Metadata in `description/title-info`. Nested sections
  become heading levels, and each top-level section starts a page. `binary` elements hold base64 images.
  `type="note"` links become real footnotes. The writer turns headings into nested sections, puts notes in
  a `notes` body, writes list markers out, and starts a new paragraph at each line break (FB2 has no line
  breaks).
- **HTMLZ / TXTZ** (`ZippedText.cs`): calibre's zipped formats. HTMLZ = `index.html` + `style.css` +
  `images/` + `metadata.opf`. TXTZ = `index.txt` (read as **Markdown**) + `images/` + `metadata.opf`. The
  main file is the preferred name at the top level, else the shallowest file with a matching extension.
  The minimal OPF is version 2.0 with dc:title, language, identifier, creator and so on.
- **Plain text for books** (`PlainText.cs`), which differs from TXT → HWPX: paragraphs are separated by
  blank lines. If the text has no blank lines, each line is a paragraph. **Hard-wrap detection:** if more
  than 70% of the non-final lines of multi-line paragraphs are 50-100 characters long, join them with
  spaces; otherwise join with line breaks.
- **Comics** (`ComicBook.cs`): SharpCompress opens ZIP, RAR, 7z and TAR. CBC is a ZIP of CBZ files ordered
  by `comics.txt` (`file.cbz:Title` lines). macOS metadata is skipped. Pages sort **naturally** (`p2` before
  `p10`, ignoring case). PDF output: one page per picture, sized from its DPI (or 96), with JPEG embedded
  without re-encoding. CBZ output is stored (no compression) and numbered.
- **HTML output** from a book: one self-contained page with images as `data:` URIs. **MD output**: images
  go to `<name>_files/imgNNNN.ext` next to the output.

### 10.6 calibre (`CalibreConverter.cs`)

- Edges: `{lit, lrf, chm, pdb, pml, rb, snb, tcr, oeb} → epub` and `epub → {mobi, azw3, lit, lrf, pdb,
  pml, rb, snb, tcr}`, all **cost 20**. EPUB is the hub, so MOBI output is EPUB → calibre.
- Invocation: `ebook-convert <input.EXT> <output.EXT>`. ebook-convert picks formats **by file extension**,
  so the input is copied to `<work>/input.<from>` (`oeb` becomes `.zip`; an `.opf` is used in place).
- Isolated environment (line 62):
  ```
  CALIBRE_CONFIG_DIRECTORY=<work>/config
  CALIBRE_CACHE_DIRECTORY=<work>/cache
  CALIBRE_TEMP_DIR=<work>/temp
  CALIBRE_OVERRIDE_LANG=en
  PYTHONIOENCODING=utf-8
  ```
- Progress: parse output lines with `^\s*(\d{1,3})%\s` and report `percent/100 * 0.95`.
- Timeout **10 minutes**, `MaxParallelism = 2`. On failure the message is the last 3 lines of stderr
  (or of stdout when stderr is empty).

---

## 11. E-mail — `E/Email/EmlConverter.cs`

Edges: `eml → html`, `eml → txt`, `eml → zip` (attachments only).

- Parsing: MimeKitLite. The **charset fallback for raw 8-bit headers is CP949** (old Korean mail
  programs).
- Header list (only the fields present): `From`, `To`, `Cc` (as `Name <addr>` or `addr`), `Date`
  (`yyyy-MM-dd HH:mm:ss zzz`), `Subject`, `Attachments` (comma-joined names).
- **HTML output:**
  1. Take the HTML body. Replace `cid:` references (`cid:([^"'\s)>]+)`) with `data:<mime>;base64,...` of
     the part that has that Content-ID. Remove `<script>` blocks.
  2. Keep the `<style>` blocks. Use the content of `<body>`, or the HTML with `<head>` removed.
  3. If there is no HTML body, use `<pre class="filee-mail-text">` with the escaped text body.
  4. Wrap in a page: `<meta charset="utf-8">`, `<title>`=subject, small CSS for `.filee-mail-headers`, a
     header table, `<hr>`, then the body. Written as UTF-8 **without** a BOM.
- **TXT output:** `Name: value` lines with CRLF, a blank line, then the text body (or rough text from the
  HTML). Line ends normalised to CRLF, trailing whitespace trimmed. UTF-8 **with** BOM.
  - HTML to text: remove script, style and head; `<br>` becomes `\n`; closing `p, div, tr, li, h1-6, table,
    blockquote` becomes `\n`; strip tags; decode entities; NBSP becomes a space; collapse runs of spaces
    and tabs; collapse 3 or more newlines to 2.
- **ZIP output:** every `MessagePart` (as `<filename or subject or "message">.eml`) and every `MimePart`
  that is an attachment or has a file name and no Content-ID. A missing name becomes
  `attachment<ext from mime>` or `.bin`. Names are sanitised: only the last path segment, invalid and
  control characters become `_`, leading and trailing spaces and dots are trimmed, a maximum of 150
  characters, and the fallback is `attachment`. Duplicates become `name (2).ext`. Contents are decoded
  (base64 or QP) when written. **No attachments is an error:** "The e-mail has no attachments."
- Written to `.tmp`, then renamed.

---

## 12. Clever bits and fragile spots (summary)

Clever:
1. A **per-worker LibreOffice profile**, a semaphore, and a warm-up run. This fixes the silent exit-1
   failure and the slow first conversion.
2. A **private outdir** per soffice run, and taking the first non-empty file, so no output-name guessing.
3. **Cost-tuned edges**, so built-in engines win and external tools are a fallback. User toggles shift
   costs to 0 or 10.
4. **OOXML variant conversion as a ZIP rewrite**, including macro stripping and fixing the Excel sheet
   list.
5. **Hub model `HDocument`**: N readers plus M writers instead of N×M converters.
6. Spreadsheets keep both the **displayed text and the real value plus format**, so CSV gets what the user
   saw and XLSX keeps real numbers.
7. **Patching HWPX copies before rhwp** (fonts, relative sizes, equation tokens).
8. **Zip-slip-safe extraction** and **XXE-safe XML** for untrusted e-books.
9. **Isolated calibre config** through environment variables.

Fragile:
1. `DecodeText` falls back to **CP949**. Wrong for non-Korean legacy files; MorphRing should use
   `chardetng` or fall back to Windows-1252 outside Korean locales.
2. CSV writer emits rows from row 0 and column 0, so leading empty rows and columns are kept.
3. The LibreOffice JSON PDF/A filter syntax needs LO ≥ 7.4.
4. Many fixed heuristics (5000 × 60 sheet cap, 256 styled-repeat cap, 150k-character chapter limit, 70%
   hard-wrap rule, 8 px per character widths).
5. rhwp is found only as a bundled copy, and patched for its bugs (#61, #64, #65). Those patches may
   become wrong when rhwp is fixed.
6. The HWPX doubled-length quirk between `hp:default` and `hp:case`.
7. EML HTML body extraction uses **regex**, not an HTML parser (greedy `<body>(.*)</body>`).
8. ExcelDataReader gives no XLS styling; the custom BIFF and CFB parser fills only widths and heights.

---

## 13. Porting to Rust

### 13.1 Engine trait shape

Extend the scaffold's `Engine` trait (`crates/core/src/engine.rs`) toward Filee's contract:

```rust
pub struct Edge { pub from: &'static str, pub to: &'static str, pub cost: u32 } // default cost 10
pub trait Converter: Send + Sync {
    fn id(&self) -> &'static str;
    fn max_parallelism(&self) -> usize;              // 0 = unlimited
    fn edges(&self) -> Vec<Edge>;                     // recomputed after status()
    fn status(&self) -> EngineStatus;                 // re-locate binaries
    fn convert(&self, step: &Step, progress: &dyn Fn(f64), cancel: &CancelToken) -> Result<Vec<PathBuf>>;
}
```
Keep conversions synchronous in core and run them on a thread pool (rayon, or `std::thread` plus a
per-engine semaphore). Async is optional; Tauri can call into a worker thread.

### 13.2 Process runner (shared by LibreOffice, pandoc, calibre, rhwp)

- `std::process::Command` with an argv list (never a shell). Pipe stdout and stderr and read both on
  threads, line by line, for progress parsing.
- Timeout plus **kill the process tree**: on Unix, spawn in a new process group
  (`CommandExt::process_group(0)`) and `killpg` on timeout (crate `nix`, or `libc::kill(-pgid, SIGKILL)`).
  On Windows, use a Job Object (crates `win32job` or `windows`). soffice forks `soffice.bin`, so killing
  only the parent leaves orphans.
- `wait-timeout` or `std::sync::mpsc` with a timer works for the wait. `shared_child` helps when killing
  from another thread.

### 13.3 LibreOffice

- Binary names on Linux: `soffice`, `libreoffice`, `/usr/lib/libreoffice/program/soffice` (Arch puts
  `soffice` in `/usr/bin`). `which` crate plus fallbacks.
- Profile dir: `dirs::data_dir()/morphring/libreoffice-profiles/worker-N`. Convert it to a URI with
  `url::Url::from_directory_path(p)` (it handles percent-encoding and Windows drive letters). **Do not**
  build `file://` strings by hand.
- Pool: `crossbeam_channel::bounded(2)` pre-filled with `0` and `1`. A receive blocks until a profile is
  free. Return the slot in a `Drop` guard.
- Warm-up: on app start, spawn a background thread that converts `"MorphRing"` txt → pdf per unprepared
  profile (check for a `user/` folder).
- outdir: `tempfile::TempDir` inside the job dir. After the run, take the first file with `len > 0` and
  `fs::rename` it to the target (fall back to copy plus remove across filesystems).
- Port `FilterFor` and `BuildEdges` as plain tables or `match`. Unit-test them against section 3.4.
- Timeout 180 s.

### 13.4 Pandoc and calibre

- Pandoc: `Command::new("pandoc").args([in, "-f", r, "-t", w, "-o", out])`, plus `--standalone` and
  `--extract-media` as in section 4, with `current_dir(input.parent())`. Status = found on PATH. Not
  installed on our dev box, so the edges must disappear when it is missing.
- calibre: copy the input to `input.<ext>`, set the 5 env vars, parse `^\s*(\d{1,3})%\s` with the `regex`
  crate. Binary: `ebook-convert`.

### 13.5 OOXML variant rewrite (pure Rust, small)

- Crates: `zip` (read plus write; keep entry order) and `quick-xml` (or `roxmltree` to read plus string
  edits). The rewrite edits attributes only, so a streaming `quick-xml` reader/writer pass over
  `[Content_Types].xml`, the `.rels` files and `workbook.xml` is enough.
- Copy unchanged entries with `ZipWriter::raw_copy_file`, which keeps the bytes compressed and is fast.
- Write to `out.tmp`, then rename.

### 13.6 Spreadsheets

- Reading: **`calamine`** reads xlsx, xlsm, xlsb, xls and ods (values, sheet visibility, merged regions,
  some formats). It does **not** give fonts, fills, borders, widths or heights the way Filee's readers do.
  For v0.1.0, values-only is acceptable. Full style fidelity later needs our own quick-xml reader
  (section 6.5 is small enough to port).
- Writing XLSX: **`rust_xlsxwriter`** (numbers, formats, merges, widths, freeze panes, hidden
  rows/columns). Or port the hand-written writer in section 6.6; it is about 250 lines.
- ODS writing: no mature crate (`spreadsheet-ods` exists and is usable). Otherwise port `OdsWriter`
  (mimetype stored first!).
- Excel number-format rendering: no full Rust equivalent of ExcelNumberFormat. Options: `ssfmt-rs` or
  `numfmt`, or a reduced formatter (General, `0`, `0.00`, `#,##0`, `%`, ISO dates) for v0.1.0. calamine's
  `Data::DateTime` plus `as_datetime()` covers dates. Remember the **1900 leap-year bug** and the
  **1904 offset of 1462**.
- CSV: the `csv` crate (`ReaderBuilder::delimiter`, `flexible(true)`) handles RFC 4180. Port the delimiter
  sniffing (sample of 4096, tie goes to comma) and `IsNumber` exactly. Write the BOM manually
  (`\u{FEFF}`), use `Terminator::CRLF` and `QuoteStyle::Necessary`; also quote leading or trailing spaces
  (custom check, then `QuoteStyle::Always` per field, or hand-roll the writer).
- Text decoding: `encoding_rs` (`EUC_KR` is WHATWG's windows-949, which covers CP949; `UTF_16LE/BE`;
  `WINDOWS_1252`). Consider `chardetng` to guess instead of hard-coding CP949.

### 13.7 HDocument and writers

- Model: plain Rust enums (`enum Block { Paragraph(..), Table(..) }`,
  `enum Inline { Text{..}, LineBreak, Tab, Link{..}, ... }`) with `Option<T>` fields for "inherit".
  Derive `Hash` and `Eq` on format structs for de-duplication (this replaces `HParaFormat.Equals`).
- Readers: Markdown via **`pulldown-cmark`** (tables, footnotes, strikethrough, tasklists) or `comrak`
  (closer to Markdig's extensions, with GFM and front matter). HTML via **`html5ever`/`scraper`** or `kuchikiki`.
  DOCX: none mature enough for layout; `docx-rs` reads basic text. Port `DocxReader` later.
- Writers: XHTML and Markdown are easy string builders. **DOCX writer:** `docx-rs` writes paragraphs,
  runs, tables and images in schema order, which is a good fit for v0.2. HWPX writer: port later, keeping
  the template approach (embed `blank.hwpx` with `include_bytes!`).
- EPUB writer: `epub-builder` crate (EPUB 2/3, nav, NCX, mimetype handled). Or port section 10.4 by hand,
  which is about 150 lines with `zip`. **The mimetype must be the first entry and stored**:
  `SimpleFileOptions::default().compression_method(Stored)`.

### 13.8 HWP

- **rhwp is itself a Rust project** (github.com/edwardkim/rhwp, MIT). Check whether it can be used as a
  **library crate** instead of a CLI. That would remove the bundled binary and the process round-trip.
  Otherwise shell out exactly as in section 9.1.
- `unhwp` also has a Rust origin (the NuGet package wraps a native library); check crates.io for `unhwp`.
- Font lookup for the rhwp patches: `fontdb` (pure Rust, scans system fonts, reports coverage through
  `ttf-parser`) instead of SkiaSharp. CFF2 detection: check for a `CFF2` table with `ttf-parser`.
- HWP is a niche format for our (non-Korean) first users. Defer it.

### 13.9 E-books and e-mail

- EPUB reading: `epub` crate or `rbook`, or port section 10.3 with `zip` plus `roxmltree` (roxmltree
  never resolves external entities, so it is safe by default). Implement **zip-slip protection**: use
  `ZipFile::enclosed_name()`, which returns `None` for escaping paths.
- MOBI: the **`mobi`** crate reads MOBI 6 (PalmDOC, HUFF/CDIC, EXTH). KF8/AZW3 support is partial, so port
  `MobiReader` later. The PalmDOC LZ77 in section 10.5 is about 30 lines.
- Comics: `zip`, `unrar` (C library binding, licence caveats), `sevenz-rust`, `tar`. Natural sort:
  `natord` or `alphanumeric-sort`. PDF from images: `printpdf` or `lopdf`.
- EML: **`mail-parser`** (Stalwart; handles RFC 2047/2231, QP/base64, many charsets including EUC-KR,
  `body_html(0)`, `attachments()`, Content-ID). Port the HTML wrapper, cid inlining (`base64` crate) and
  the attachment-name sanitiser exactly. For HTML to text, `html2text` is better than the regex approach.

### 13.10 General pitfalls

- Always write to `<target>.tmp`, then `rename`. Clean up in a `Drop` guard.
- Every path from an archive is untrusted. Every XML parser must refuse DTD or external entity
  resolution.
- On Windows, `rename` over an existing file needs `fs::rename` (it replaces atomically on NTFS through
  `MoveFileEx`, as Rust's std does) or a remove first.
- Treat soffice exit codes as unreliable: **success means a non-empty output file exists**. The same rule
  applies to rhwp and pandoc.

---

## 14. v0.1.0 candidates

What is small, valuable and doable today on the dev machine (LibreOffice installed; pandoc and calibre
not installed):

1. **LibreOffice engine**: per-worker profiles (2), a private outdir, a 180 s timeout with tree kill, the
   full `FilterFor` table and the Writer/Calc/Impress/Draw families. This gives
   DOCX/DOC/ODT/RTF ↔ each other and → PDF, XLSX/XLS/ODS/CSV ↔ each other and → PDF, PPTX/PPT/ODP → PDF,
   and the read-only legacy formats → PDF. One engine covers most office use. (The warm-up can be a
   follow-up, but it is about 30 lines.)
2. **CSV/TSV ↔ XLSX built-in**: `calamine` to read, `rust_xlsxwriter` to write, plus the ported CSV rules
   (delimiter sniffing, BOM, CRLF, number detection, one CSV per sheet with a `_<sheet>` suffix). It is
   fast, needs no external tool, and gives a cost-10 route that beats LibreOffice.
3. **OOXML variant rewrite** (docm/dotx/dotm ↔ docx, xlsm/xltx ↔ xlsx, pptm/potx/ppsx ↔ pptx, with macro
   stripping). Pure `zip` plus `quick-xml`, about 200 lines, easy to test.
4. **EML → HTML / TXT / ZIP** with `mail-parser`. Fully self-contained, about 250 lines.
5. **Pandoc engine as optional**: detect on PATH and publish the edges only when found, with the exact
   reader and writer tables and flags from section 4. Cheap to add, but it cannot be tested on the dev box
   until `pacman -S pandoc-cli`.
6. **calibre engine as optional**: the same detect-or-hide pattern, an isolated env, and progress parsing.
   Low effort, but untestable today.
7. **The route-cost scheme** from section 2 (fallback 25, built-in 10, toggles), even with few engines, so
   later built-in engines slot in without re-plumbing.

Defer to later releases: the HDocument hub with the HWPX and DOCX writers, PPTX/DOCX readers, rhwp/HWP, the
MOBI/KF8 reader, FB2, comics, the ODS writer with number styles, styled XLSX reading, and the BIFF layout
parser.
