// The navigation/forms shell. index.html loads exactly one real page; every
// other "page" is an HTML fragment rendered server-side (in the Rust
// process, via Tera) and swapped into #app-content over Tauri's IPC, instead
// of a real HTTP navigation. Links and forms are intercepted generically —
// nothing in the ported templates needs a per-form JS hook — and after every
// swap the existing page-behavior scripts (ui.js, scanner.js, batch.js,
// instruments.js, markdown-editor.js) are re-run against the fresh DOM,
// exactly as they'd run again on a normal full-page load.

const invoke = window.__TAURI__.core.invoke;
const content = document.getElementById('app-content');

// window.alert() never displays anything in this webview, so errors go
// through a native dialog instead.
function showError(message) {
  invoke('error_dialog', { message }).catch(() => {});
}

// Splits "/instruments?show=active#procedure-5" into its path, query and
// fragment. The fragment matters: anchors like #procedure-5 (used to keep a
// card open/scrolled-to after a move/edit) must never be sent to the Rust
// router as part of the path, or routing breaks outright (the router has no
// route named "instruments#procedure-5").
function splitHref(href) {
  const hashIndex = href.indexOf('#');
  const fragment = hashIndex === -1 ? '' : href.slice(hashIndex + 1);
  const withoutHash = hashIndex === -1 ? href : href.slice(0, hashIndex);
  const qIndex = withoutHash.indexOf('?');
  const path = qIndex === -1 ? withoutHash : withoutHash.slice(0, qIndex);
  const query = qIndex === -1 ? '' : withoutHash.slice(qIndex + 1);
  return [path, query, fragment];
}

function reinitPage() {
  if (typeof initUi === 'function') initUi();
  if (typeof initScanner === 'function') initScanner();
  if (typeof initBatch === 'function') initBatch();
  if (typeof initInstruments === 'function') initInstruments();
  if (typeof initMarkdownEditors === 'function') initMarkdownEditors(content);
}

async function applyResult(result, fragment) {
  if (result.kind === 'redirect') {
    // Defense in depth: a redirect's path should already be bare (query
    // and fragment sent separately), but route it through splitHref anyway
    // so a stray "?"/"#" that slips in server-side degrades gracefully
    // instead of producing a path the router can never match.
    const [path, extraQuery, redirectFragment] = splitHref(result.path);
    const query = [result.query, extraQuery].filter(Boolean).join('&');
    await go(path, query, redirectFragment);
    return;
  }
  content.innerHTML = result.html;
  document.title = result.title || 'Batteries';
  reinitPage();
  if (result.open_dialog && typeof openDialog === 'function') {
    openDialog(result.open_dialog);
  }
  if (fragment) {
    const target = document.getElementById(fragment);
    if (target) target.scrollIntoView({ block: 'center' });
  }
}

// Remembered so a delete/restore/reset initiated from a battery's own page
// (which has no idea what the list was filtered/sorted/paged to) can return
// there instead of resetting to the unfiltered list. The list's own row
// actions already carry an explicit "next" field with their own list_url
// and always win over this.
let lastListQuery = null;

// Every print action (add-with-print, single reprint, batch print) redirects
// here the same way whether it printed or not; in PDF mode (see
// router::label_output_is_pdf) the redirect's query carries a save_pdf_ids
// field instead of actually printing. It's stripped before the query ever
// reaches the Rust router — it's not a real filter/page param — and instead
// triggers the native save dialog once the redirected-to page has loaded.
async function go(path, query, fragment) {
  const params = new URLSearchParams(query || '');
  const pdfIds = params.get('save_pdf_ids');
  if (pdfIds) params.delete('save_pdf_ids');
  const cleanQuery = params.toString();
  const result = await invoke('render_page', { path, query: cleanQuery });
  if (path === '/') lastListQuery = cleanQuery;
  await applyResult(result, fragment);
  if (pdfIds) {
    const ids = pdfIds.split(',').map(Number).filter((n) => Number.isInteger(n));
    if (ids.length) {
      try {
        await invoke('save_labels_pdf', { ids });
      } catch (e) {
        showError('Could not save the PDF: ' + e);
      }
    }
  }
}

function rememberedListUrl() {
  if (lastListQuery === null) return '/';
  return lastListQuery ? '/?' + lastListQuery : '/';
}

// Exposed for the ported page scripts (ui.js's row-click, scanner.js's
// "open and edit") to navigate in place instead of a real page load, which
// would reload index.html from scratch and lose all app state.
window.AppShell = { navigate: (href) => { const [p, q, f] = splitHref(href); go(p, q, f); } };

function formToFields(form, submitter) {
  const data = new FormData(form, submitter || undefined);
  const fields = {};
  for (const [key, value] of data.entries()) {
    if (typeof value === 'string') fields[key] = value;
  }
  return fields;
}

async function handleDownload(link) {
  const url = new URL(link.href);
  const match = url.pathname.match(/^\/label\/(\d+)\.png$/);
  if (!match) return;
  const id = Number(match[1]);
  try {
    const bytes = await invoke('get_label_png', { id });
    const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' });
    const objectUrl = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = objectUrl;
    a.download = `battery-${String(id).padStart(3, '0')}.png`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(objectUrl), 5000);
  } catch (e) {
    showError('Could not render the label: ' + e);
  }
}

document.addEventListener('click', (event) => {
  if (event.defaultPrevented) return;
  const link = event.target.closest('a[href]');
  if (!link) return;
  const href = link.getAttribute('href');
  if (!href || !href.startsWith('/')) return;
  if (link.target === '_blank') return;
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
  if (link.hasAttribute('download')) {
    event.preventDefault();
    handleDownload(link);
    return;
  }
  event.preventDefault();
  const [path, query, fragment] = splitHref(href);
  go(path, query, fragment);
});

document.addEventListener('submit', async (event) => {
  if (event.defaultPrevented) return;
  const form = event.target;
  if (!(form instanceof HTMLFormElement)) return;
  event.preventDefault();
  const submitter = event.submitter;

  // A plain window.confirm() never actually shows a dialog in this webview
  // (it resolves without prompting), so every destructive form carries its
  // question as data-confirm instead and is checked here, through a real
  // native message box, before anything is submitted.
  const confirmMessage = (submitter && submitter.getAttribute('data-confirm')) || form.dataset.confirm;
  if (confirmMessage) {
    const ok = await invoke('confirm_dialog', { message: confirmMessage });
    if (!ok) return;
  }
  const actionAttr = (submitter && submitter.getAttribute('formaction')) || form.getAttribute('action') || '/';
  const [path] = splitHref(actionAttr);
  const method = (form.getAttribute('method') || 'get').toLowerCase();
  const fields = formToFields(form, submitter);

  // A battery's own page (or the Add page, reached from a header icon) has
  // no "next" field of its own — fall back to wherever the list last was,
  // so acting from a filtered view doesn't dump you back on the unfiltered
  // one.
  if (!fields.next && /^\/(add|b\/\d+\/(delete|restore|reset|purge))$/.test(path)) {
    fields.next = rememberedListUrl();
  }

  if (method === 'get') {
    const query = new URLSearchParams(fields).toString();
    await go(path, query);
  } else {
    try {
      const result = await invoke('submit_form', { path, fields });
      await applyResult(result);
    } catch (e) {
      showError('Save failed: ' + e);
    }
  }
});

document.addEventListener('DOMContentLoaded', () => {
  go('/', '');
  invoke('app_version').then((v) => {
    const el = document.getElementById('app-version');
    if (el) el.textContent = 'v' + v;
  });
});
