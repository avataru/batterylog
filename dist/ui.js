// Opens the add, scan and print dialogs from the header icons, and reopens one
// when the server has rendered it with content in it, which is how a
// validation error or a print preview survives a submit.
//
// Wrapped in initUi() so app.js can re-run it against fresh DOM after every
// content swap (the original app relied on a full page load doing this once
// per script tag; here there is no page load to hook, only a content swap).

function openDialog(name) {
  const dialog = document.getElementById('dlg-' + name);
  if (!dialog) return;
  if (dialog.open) dialog.close();
  dialog.showModal();
  dialog.dispatchEvent(new CustomEvent('dialog-opened'));
  const first = dialog.querySelector('[data-autofocus]:not([disabled])')
             || dialog.querySelector('[data-autofocus]:not([hidden])');
  if (first) { first.focus(); if (first.select) first.select(); }
}

function initUi() {
  // The icons are ordinary links to /add, /scan, /print, which work from any
  // page. On a page that has the matching dialog in it already, the click is
  // intercepted and it opens in place instead of navigating.
  document.querySelectorAll('[data-dialog]').forEach((trigger) => {
    trigger.addEventListener('click', (event) => {
      const name = trigger.dataset.dialog;
      if (!document.getElementById('dlg-' + name)) return;   // let app.js navigate instead
      event.preventDefault();
      openDialog(name);
    });
  });

  document.querySelectorAll('dialog .dlg-close').forEach((button) => {
    button.addEventListener('click', () => button.closest('dialog').close());
  });

  // Reading forms hide/show fields based on kind. Hidden fields are also
  // disabled, so a value cannot leak across configurations by being
  // submitted anyway; values survive kind changes, so switching back and
  // forth loses nothing.
  document.querySelectorAll('[data-kind-scope]').forEach((scope) => {
    const select = scope.querySelector('[data-kind-select]');
    if (!select) return;
    const blocks = [...scope.querySelectorAll('[data-kinds]')];

    function refresh() {
      blocks.forEach((block) => {
        block.hidden = !block.dataset.kinds.split(' ').includes(select.value);
      });
      scope.querySelectorAll('input, select, textarea').forEach((field) => {
        if (field === select) return;
        field.disabled = Boolean(field.closest('[data-kinds][hidden]'));
      });
    }

    select.addEventListener('change', refresh);
    refresh();
  });

  const requested = document.body.dataset.openDialog;
  if (requested) openDialog(requested);

  // Settings page: import another batteries.db, replacing this app's data
  // live via a native file picker (no plain <form> here since it needs a
  // blocking file dialog, not a form submission).
  const exportBtn = document.getElementById('export-db-btn');
  if (exportBtn) {
    exportBtn.addEventListener('click', async () => {
      const status = document.getElementById('export-db-status');
      const invoke = window.__TAURI__.core.invoke;
      exportBtn.disabled = true;
      status.textContent = '';
      try {
        const path = await invoke('export_database');
        status.textContent = path ? 'Saved to ' + path + '.' : '';
      } catch (e) {
        status.textContent = 'Export failed: ' + e;
      } finally {
        exportBtn.disabled = false;
      }
    });
  }

  const importBtn = document.getElementById('import-db-btn');
  if (importBtn) {
    importBtn.addEventListener('click', async () => {
      const status = document.getElementById('import-db-status');
      const invoke = window.__TAURI__.core.invoke;
      importBtn.disabled = true;
      status.textContent = 'Choose a file in the dialog…';
      try {
        const path = await invoke('pick_database_file');
        if (!path) {
          status.textContent = '';
          return;
        }
        const sure = await invoke('confirm_dialog', {
          message: 'Replace every battery, instrument and reading in this app with the ' +
            'contents of\n\n' + path + '\n\nThis cannot be undone.'
        });
        if (!sure) {
          status.textContent = '';
          return;
        }
        status.textContent = 'Importing…';
        await invoke('import_database', { source: path });
        status.textContent = 'Imported ' + path + '.';
        window.AppShell.navigate('/settings');
      } catch (e) {
        status.textContent = 'Import failed: ' + e;
      } finally {
        importBtn.disabled = false;
      }
    });
  }

  // Battery detail page: preview a battery's label as a rendered image
  // before saving it anywhere, instead of a silent file download.
  const previewBtn = document.getElementById('preview-label-btn');
  if (previewBtn) {
    previewBtn.addEventListener('click', async () => {
      const id = Number(previewBtn.dataset.batteryId);
      const status = document.getElementById('label-preview-status');
      const img = document.getElementById('label-preview-img');
      const saveBtn = document.getElementById('label-save-btn');
      img.removeAttribute('src');
      status.textContent = 'Rendering…';
      saveBtn.disabled = true;
      openDialog('label-preview');
      try {
        const invoke = window.__TAURI__.core.invoke;
        const bytes = await invoke('get_label_png', { id });
        const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' });
        img.src = URL.createObjectURL(blob);
        status.textContent = '';
        saveBtn.disabled = false;
        saveBtn.onclick = async () => {
          saveBtn.disabled = true;
          status.textContent = 'Choose where to save…';
          try {
            const path = await invoke('save_label_png', { id });
            status.textContent = path ? 'Saved to ' + path + '.' : '';
          } catch (e) {
            status.textContent = 'Save failed: ' + e;
          } finally {
            saveBtn.disabled = false;
          }
        };
      } catch (e) {
        status.textContent = 'Preview failed: ' + e;
      }
    });
  }

  // A handful of edit forms on the instruments page sit inside a <details>
  // whose visible <summary> is hidden; a plain button elsewhere in the card
  // toggles it instead, because the delete button sits between them in the
  // markup.
  document.querySelectorAll('[data-toggle-details]').forEach((button) => {
    const target = document.querySelector(button.dataset.target);
    if (!target) return;
    button.addEventListener('click', () => { target.open = !target.open; });
  });

  // Cancelling an edit discards whatever was typed and closes the disclosure
  // back up. type="reset" already restores every field to what was
  // rendered; this only has to close the <details> afterwards, and nudge the
  // markdown editor (if there is one in this form) to redraw its preview.
  document.querySelectorAll('[data-cancel-edit]').forEach((button) => {
    button.addEventListener('click', () => {
      const details = button.closest('details');
      const form = button.closest('form');
      setTimeout(() => {
        if (details) details.open = false;
        if (form) {
          form.querySelectorAll('textarea').forEach((area) => {
            area.dispatchEvent(new Event('input', { bubbles: true }));
          });
        }
      }, 0);
    });
  });

  // Clicking anywhere on a battery row opens it. The id in the first column
  // is a real link, so this is only a convenience.
  document.querySelectorAll('tr[data-href]').forEach((row) => {
    row.addEventListener('click', (event) => {
      if (event.target.closest('a, button, form, input, select, label')) return;
      const selection = window.getSelection && String(window.getSelection());
      if (selection) return;
      window.AppShell.navigate(row.dataset.href);
    });
  });

  // Every text field that offers suggestions keeps its datalist; the chips
  // below it are the short head of that same list, sorted by how often each
  // value has been used.
  document.querySelectorAll('.chips[data-fills]').forEach((group) => {
    const field = document.querySelector(group.dataset.fills);
    if (!field) return;
    group.addEventListener('click', (event) => {
      const chip = event.target.closest('.chip');
      if (!chip) return;
      field.value = chip.dataset.value;
      field.dispatchEvent(new Event('input', { bubbles: true }));
      field.focus();
    });
  });

  // The Procedure dropdown only ever offers what the chosen Instrument
  // actually has: each <option> in the instrument select carries its own
  // procedures as JSON.
  document.querySelectorAll('select[data-modes-target]').forEach((instrumentSelect) => {
    const modeSelect = document.querySelector(instrumentSelect.dataset.modesTarget);
    if (!modeSelect) return;
    const kindSelect = instrumentSelect.dataset.kindFilter
      ? document.querySelector(instrumentSelect.dataset.kindFilter) : null;
    let initial = modeSelect.dataset.selected || '';

    function refresh() {
      const chosen = instrumentSelect.selectedOptions[0];
      let modes = chosen ? JSON.parse(chosen.dataset.modes || '[]') : [];
      if (kindSelect && kindSelect.value) {
        modes = modes.filter((m) => m.kind === kindSelect.value);
      }
      const keep = initial || modeSelect.value;

      modeSelect.replaceChildren(new Option(
        modes.length ? 'Select a procedure…' : 'No procedures for this instrument', ''));
      modes.forEach((m) => modeSelect.add(new Option(m.name, m.id)));
      if (modes.some((m) => String(m.id) === keep)) modeSelect.value = keep;
      initial = '';
    }

    instrumentSelect.addEventListener('change', refresh);
    if (kindSelect) kindSelect.addEventListener('change', refresh);
    refresh();
  });

  // Apply field visibility based on the selected procedure's configuration.
  document.querySelectorAll('select[data-modes-target]').forEach((instrumentSelect) => {
    const modeSelect = document.querySelector(instrumentSelect.dataset.modesTarget);
    if (!modeSelect) return;

    const container = modeSelect.closest('form') || modeSelect.closest('[data-kind-scope]');
    if (!container) return;

    const kindSelect = instrumentSelect.dataset.kindFilter
      ? document.querySelector(instrumentSelect.dataset.kindFilter) : null;

    function applyFieldConfig() {
      const chosen = instrumentSelect.selectedOptions[0];
      const modeId = modeSelect.value;

      const procedureFieldsContainers = container.querySelectorAll('[data-procedure-fields]');
      const hasProcedure = chosen && chosen.value && modeId;

      procedureFieldsContainers.forEach((c) => {
        c.hidden = !hasProcedure;
        c.querySelectorAll('input, select, textarea').forEach((input) => {
          input.disabled = !hasProcedure;
        });
      });

      if (!chosen || !chosen.value || !modeId) return;

      const modes = JSON.parse(chosen.dataset.modes || '[]');
      const mode = modes.find((m) => String(m.id) === String(modeId));
      if (!mode || !mode.fields) return;

      const fields = JSON.parse(mode.fields || '{}');

      ['capacity_mah', 'ir_mohm', 'discharge_ma', 'charge_ma', 'notes'].forEach((fieldName) => {
        const config = fields[fieldName] || {};
        const visible = config.visible !== false;
        const containers = container.querySelectorAll(`[data-field="${fieldName}"]`);

        containers.forEach((c) => {
          c.hidden = !visible;
          const input = c.querySelector('input, select, textarea');
          if (input) {
            input.disabled = !visible;
            const isBatchForm = input.closest('table.batch-rows');
            if (visible && !isBatchForm) {
              if (config.required) input.setAttribute('required', '');
              else input.removeAttribute('required');
            } else {
              input.removeAttribute('required');
            }
          }
        });
      });
    }

    modeSelect.addEventListener('change', applyFieldConfig);
    instrumentSelect.addEventListener('change', () => {
      setTimeout(applyFieldConfig, 50);
    });

    if (kindSelect) {
      kindSelect.addEventListener('change', () => {
        const procedureFieldsContainers = container.querySelectorAll('[data-procedure-fields]');
        procedureFieldsContainers.forEach((c) => {
          c.hidden = true;
          c.querySelectorAll('input, select, textarea').forEach((input) => { input.disabled = true; });
        });
        modeSelect.value = '';
      });
    }

    const initialCheck = () => {
      const procedureFieldsContainers = container.querySelectorAll('[data-procedure-fields]');
      const hasProcedure = instrumentSelect.value && modeSelect.value;
      procedureFieldsContainers.forEach((c) => {
        c.hidden = !hasProcedure;
        c.querySelectorAll('input, select, textarea').forEach((input) => { input.disabled = !hasProcedure; });
      });
      if (hasProcedure) applyFieldConfig();
    };
    setTimeout(initialCheck, 100);
  });
}
