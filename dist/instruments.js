// Show/hide field configuration options based on procedure kind. Wrapped in
// initInstruments() so app.js can re-run it after every content swap.

function initInstruments() {
  document.querySelectorAll('[data-kind-fields]').forEach((container) => {
    const form = container.closest('form');
    const kindSelect = form.querySelector('select[name="kind"]');
    if (!kindSelect) return;

    function updateFields() {
      const kind = kindSelect.value;
      container.querySelectorAll('[data-field-kind]').forEach((row) => {
        const fieldKind = row.dataset.fieldKind;
        row.hidden = fieldKind && fieldKind !== kind;
        row.querySelectorAll('input').forEach((input) => {
          input.disabled = row.hidden;
        });
      });
    }

    kindSelect.addEventListener('change', updateFields);
    updateFields();
  });
}
