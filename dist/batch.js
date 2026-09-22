// The two batch pages. Wrapped in initBatch() so app.js can re-run it after
// every content swap; on any other page it finds nothing and does nothing.
//
// Both are built for a barcode scanner in keyboard mode, which types the id
// and then sends Enter (sometimes twice, for a CR LF terminator) — so an
// Enter in an id field is never allowed to submit the form, and a repeat
// Enter straight after the first has to be harmless.

function initBatch() {
  initBatchReading();
  initBatchLocation();
}

// Batch add reading: the number of rows offered follows the chosen
// instrument's slot count, and Enter in an id field moves on to the next id
// field so a run of cells can be scanned one after another.
function initBatchReading() {
  const instrumentSelect = document.getElementById('batch-instrument');
  const rows = [...document.querySelectorAll('[data-slot-row]')];

  function refreshRows() {
    const chosen = instrumentSelect.selectedOptions[0];
    const slots = chosen && chosen.value
      ? parseInt(chosen.dataset.slots || '1', 10) || 1
      : 1;

    rows.forEach((row, index) => {
      const active = index < slots;
      row.hidden = !active;
      row.querySelectorAll('input').forEach((field) => { field.disabled = !active; });
    });
  }

  if (instrumentSelect && rows.length) {
    instrumentSelect.addEventListener('change', refreshRows);
    refreshRows();
  }

  const form = document.getElementById('batch-form');
  if (!form) return;
  form.addEventListener('keydown', (event) => {
    const field = event.target;
    if (event.key !== 'Enter' || !field.name || !field.name.startsWith('battery_id_')) return;
    event.preventDefault();
    // An empty field means this is the second half of a CR LF, or a stray
    // press: stay put rather than skipping a slot.
    if (!field.value.trim()) return;
    const ids = [...form.querySelectorAll('input[name^="battery_id_"]')]
      .filter((input) => !input.disabled && input.offsetParent !== null);
    const next = ids[ids.indexOf(field) + 1];
    if (next) { next.focus(); next.select(); }
  });
}

// Batch change location: a scan is appended to the ids field as a
// comma-separated list instead of submitting the form.
function initBatchLocation() {
  const spec = document.querySelector('form[action="/batch/location"] input[name="spec"]');
  if (!spec) return;
  spec.addEventListener('keydown', (event) => {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    const value = spec.value.trim();
    if (value && !value.endsWith(',')) spec.value = value + ',';
  });
}
