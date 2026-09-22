// Scan page: a USB or Bluetooth barcode scanner in keyboard mode types the id
// into the entry box and presses Enter, which looks up the battery and offers
// the two things you might do with a cell in your hand: file it somewhere, or
// write down what the charger just said about it.
//
// There is no camera path — see labels.rs's payload doc comment: the printed
// code is just the zero-padded id, decoded by the scanner hardware itself,
// not by anything running here.
//
// Wrapped in initScanner() so app.js can re-run it after every content swap;
// it's a no-op (returns immediately) on any page that isn't /scan.

function initScanner() {
  const manual = document.getElementById('manual');
  if (!manual) return; // not the scan page

  const invoke = window.__TAURI__.core.invoke;
  const status = document.getElementById('status');
  const entryPane = document.getElementById('scan-entry');
  const resultPane = document.getElementById('scan-result');
  let current = null;

  function showEntry() {
    resultPane.hidden = true;
    entryPane.hidden = false;
  }

  function showResult() {
    entryPane.hidden = true;
    resultPane.hidden = false;
  }

  function say(text) { status.textContent = text; }

  function showPane(name) {
    document.querySelectorAll('#dlg-panes .pill').forEach((tab) => {
      tab.classList.toggle('on', tab.dataset.pane === name);
    });
    document.getElementById('dlg-move').hidden = name !== 'move';
    document.getElementById('dlg-read').hidden = name !== 'read';
    const pane = document.getElementById(name === 'move' ? 'dlg-move' : 'dlg-read');
    const focus = pane.querySelector('input:not([disabled]), select:not([disabled])');
    if (focus) { focus.focus(); if (focus.select) focus.select(); }
  }

  document.querySelectorAll('#dlg-panes .pill').forEach((tab) => {
    tab.addEventListener('click', () => showPane(tab.dataset.pane));
  });

  function readyForNextScan() {
    showEntry();
    manual.value = '';
    manual.focus();
  }

  async function openBattery(code) {
    say('Looking up ' + code + '…');
    let data;
    try {
      data = await invoke('resolve_code', { code });
    } catch (e) {
      say(typeof e === 'string' ? e : (e && e.message) || 'Lookup failed');
      readyForNextScan();
      return;
    }

    current = data;
    document.getElementById('dlg-title').textContent =
      'Battery ' + String(data.id).padStart(3, '0');
    document.getElementById('dlg-sub').textContent = [
      data.type,
      data.nominal_mah ? data.nominal_mah + ' mAh' : null,
      data.brand || null,
    ].filter(Boolean).join(' · ');
    document.getElementById('dlg-current').textContent =
      data.location || 'nowhere recorded';

    document.getElementById('dlg-deleted').hidden = !data.deleted;
    document.getElementById('dlg-restore').hidden = !data.deleted;
    document.getElementById('dlg-edit').hidden = !data.deleted;
    document.getElementById('dlg-panes').hidden = data.deleted;
    document.getElementById('dlg-edit').href = '/b/' + data.id;

    document.getElementById('dlg-location').value = data.location || '';
    showResult();
    if (!data.deleted) showPane('move');
    say('');
  }

  async function restoreCurrent() {
    try {
      current = await invoke('restore_battery_api', { id: current.id });
      document.getElementById('dlg-deleted').hidden = true;
      document.getElementById('dlg-restore').hidden = true;
      document.getElementById('dlg-edit').hidden = true;
      document.getElementById('dlg-panes').hidden = false;
      showPane('move');
    } catch (e) { say('Restore failed: ' + e); }
  }

  function submitManual() {
    const value = manual.value.trim();
    manual.value = '';
    if (value) openBattery(value);
  }

  // Enter (what a scanner sends) files the cell or logs a reading; the Open
  // button does what it says and goes to the battery's own page.
  async function openPage() {
    const value = manual.value.trim();
    if (!value) { manual.focus(); return; }
    try {
      const data = await invoke('resolve_code', { code: value });
      window.AppShell.navigate('/b/' + data.id);
    } catch (e) {
      say(typeof e === 'string' ? e : (e && e.message) || 'Lookup failed');
      manual.focus();
    }
  }

  document.getElementById('manual-go').addEventListener('click', openPage);

  manual.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') { event.preventDefault(); submitManual(); }
  });

  document.getElementById('dlg-location').addEventListener('keydown', (event) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      document.getElementById('dlg-save').click();
    }
  });

  document.getElementById('dlg-restore').addEventListener('click', restoreCurrent);

  document.getElementById('dlg-save').addEventListener('click', async () => {
    const location = document.getElementById('dlg-location').value.trim();
    try {
      await invoke('set_location_api', { id: current.id, location });
      say('Battery ' + String(current.id).padStart(3, '0') + ' is now at "'
          + (location || 'nowhere recorded') + '".');
      readyForNextScan();
    } catch (e) { say('Save failed: ' + e); }
  });

  function clearReadingValues() {
    ['read-capacity', 'read-ir', 'read-notes']
      .forEach((id) => { document.getElementById(id).value = ''; });
  }

  function readingValue(id) {
    const field = document.getElementById(id);
    return field.disabled ? '' : field.value.trim();
  }

  async function saveReading() {
    const payload = {
      kind: document.getElementById('read-kind').value,
      measuredAt: readingValue('read-date'),
      capacityMah: readingValue('read-capacity'),
      irMohm: readingValue('read-ir'),
      instrumentId: readingValue('read-instrument'),
      modeId: readingValue('read-mode'),
      dischargeMa: readingValue('read-rate'),
      chargeMa: readingValue('read-charge'),
      notes: readingValue('read-notes'),
    };
    try {
      await invoke('add_measurement_api', { id: current.id, ...payload });
      say('Reading saved against battery '
          + String(current.id).padStart(3, '0') + '.');
      clearReadingValues();
      readyForNextScan();
    } catch (e) { say('Save failed: ' + e); }
  }

  document.getElementById('read-save').addEventListener('click', saveReading);

  document.getElementById('read-capacity').addEventListener('keydown', (event) => {
    if (event.key === 'Enter') { event.preventDefault(); saveReading(); }
  });

  document.getElementById('read-cancel').addEventListener('click', () => {
    say('');
    readyForNextScan();
  });

  document.getElementById('dlg-cancel').addEventListener('click', () => {
    say('');
    readyForNextScan();
  });

  manual.focus();
}
