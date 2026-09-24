# Changelog

Each release on GitHub uses its section from this file as its notes, so every
version needs one before it's tagged: the release workflow refuses to build a
tag with no matching entry here.

## 3.3.1 — 2026-09-24

### Fixed

- Scanning a battery from a match back into storage now returns the rest of
  its set too, as the battery page and batch location already did.
- Clear record now also removes the cell from any match, so the next cell
  to carry that label can't close an old match and pull its set back to
  storage.
- Batch add reading checks every row before saving any of them. Before, an
  error in one row left the rows above it already saved, and fixing the
  error and resubmitting recorded those readings twice.
- A cell keeps a single "bought" entry however it's added. The Scan and
  Batch add reading pages used to add a second one instead of replacing it.
- Error messages that were never shown now appear in a dialog: a failed
  save, a PDF that couldn't be written, a label that couldn't be rendered.
- The Link and Image buttons in the procedure description editor work
  again. They now insert the link with its address selected, ready to type.
- Adding a battery says so when its label fails to print, and a match that
  fails to save is reported instead of announced as done.
- Changes that touch several records (clearing a record, deleting one
  permanently, saving or returning a match, adding several batteries) now
  either complete fully or not at all.

## 3.3.0 — 2026-09-24

### Added

- **Battery matching.** Ask for a number of cells of one type for a device,
  say whether it draws little or a lot of current, and get a suggestion of
  which cells to take from storage: one brand and capacity where possible,
  the weakest cells for low-draw devices and the strongest for high-draw
  ones. Confirming moves them and records the match.
- A **match log** listing past matches, with a **Return** button that sends
  a whole set back to storage. Returning any one cell of a set to storage,
  from anywhere in the app, returns the rest with it.
- Location history entries and matches can be deleted. Neither affects
  where the batteries are.
- Settings for the storage location matching draws from, and for the
  resistance assumed for a cell that hasn't had one measured.

## 3.2.2 — 2026-09-23

First public release.
