// A small markdown editor: a toolbar that inserts syntax around the current
// selection, and an inline preview rendered by a tiny converter below.
//
// This is deliberately not a full CommonMark implementation. It covers the
// same ground the server's real renderer (`pulldown-cmark`, via the
// `markdown` Tera filter) is asked to cover for a procedure's Details —
// headings, lists, tables, links,
// images, and raw <u>/<s> tags for underline and strikethrough, which both
// converters simply pass through unchanged. The preview exists to show
// roughly what is about to be saved while typing it, not to be the renderer
// of record; the page reload after saving always shows the real thing.
//
// No CDN, no bundler: everything the application ships is self-hosted, and a
// markdown library is a lot of weight for what a one-person protocol write-up
// actually needs.

function mdToHtml(text) {
  const escape = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;')
                         .replace(/>/g, '&gt;');

  // Raw <u> and <s> tags (what the toolbar's Underline/Strikethrough buttons
  // insert) are the one deliberate exception to escaping: they are meant to
  // pass through as HTML, exactly as the real markdown renderer leaves them
  // alone. They are pulled out before escaping and put back afterwards so the
  // angle brackets around everything else stay safe.
  const raw = [];
  let stashed = text.replace(/<\/?(u|s)>/gi, (match) => {
    raw.push(match);
    return '\u0000' + (raw.length - 1) + '\u0000';
  });

  function inline(line) {
    let out = escape(line);
    out = out.replace(/!\[([^\]]*)\]\(([^)\s]+)\)/g,
      '<img alt="$1" src="$2">');
    out = out.replace(/\[([^\]]*)\]\(([^)\s]+)\)/g,
      '<a href="$2" target="_blank" rel="noopener">$1</a>');
    out = out.replace(/`([^`]+)`/g, '<code>$1</code>');
    out = out.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    out = out.replace(/(?<!\*)\*([^*]+)\*(?!\*)/g, '<em>$1</em>');
    out = out.replace(/\u0000(\d+)\u0000/g, (_, i) => raw[Number(i)]);
    return out;
  }

  const lines = stashed.split('\n');
  const blocks = [];
  let paragraph = [];
  let list = null;     // { type: 'ul' | 'ol', items: [...] }
  let table = null;    // { header: [...], rows: [[...], ...] }

  function flushParagraph() {
    if (paragraph.length) {
      blocks.push('<p>' + paragraph.map(inline).join('<br>') + '</p>');
      paragraph = [];
    }
  }
  function flushList() {
    if (list) {
      const tag = list.type;
      blocks.push('<' + tag + '>' + list.items
        .map((item) => '<li>' + inline(item) + '</li>').join('') + '</' + tag + '>');
      list = null;
    }
  }
  function flushTable() {
    if (table) {
      const row = (cells, cellTag) => '<tr>' + cells
        .map((c) => '<' + cellTag + '>' + inline(c.trim()) + '</' + cellTag + '>')
        .join('') + '</tr>';
      blocks.push('<table><thead>' + row(table.header, 'th') + '</thead><tbody>'
        + table.rows.map((r) => row(r, 'td')).join('') + '</tbody></table>');
      table = null;
    }
  }
  function flushAll() { flushParagraph(); flushList(); flushTable(); }

  const splitRow = (line) => line.trim().replace(/^\||\|$/g, '').split('|');

  lines.forEach((rawLine, index) => {
    const line = rawLine;
    const heading = line.match(/^(#{1,4})\s+(.*)$/);
    const bullet = line.match(/^[-*]\s+(.*)$/);
    const numbered = line.match(/^\d+\.\s+(.*)$/);
    const isTableRow = /^\s*\|.*\|\s*$/.test(line);
    const isTableRule = /^\s*\|?[\s:|-]+\|?\s*$/.test(line) && line.includes('-');

    if (heading) {
      flushAll();
      const level = heading[1].length + 1; // start at h2, h1 is the page title
      blocks.push('<h' + level + '>' + inline(heading[2]) + '</h' + level + '>');
    } else if (isTableRow && !table) {
      flushParagraph(); flushList();
      table = { header: splitRow(line), rows: [] };
    } else if (table && isTableRule) {
      // the |---|---| separator line: nothing to render, just confirms the
      // block above was a header row
    } else if (table && isTableRow) {
      table.rows.push(splitRow(line));
    } else if (bullet) {
      flushParagraph(); flushTable();
      if (!list || list.type !== 'ul') { flushList(); list = { type: 'ul', items: [] }; }
      list.items.push(bullet[1]);
    } else if (numbered) {
      flushParagraph(); flushTable();
      if (!list || list.type !== 'ol') { flushList(); list = { type: 'ol', items: [] }; }
      list.items.push(numbered[1]);
    } else if (line.trim() === '') {
      flushAll();
    } else {
      flushList(); flushTable();
      paragraph.push(line);
    }
  });
  flushAll();

  return blocks.join('\n') || '<p class="muted">Nothing to preview yet.</p>';
}

// Wraps the current selection in a textarea with a prefix/suffix pair,
// falling back to a placeholder word when nothing is selected so the result
// is never just an empty pair of markers.
function mdWrap(textarea, before, after, placeholder) {
  const start = textarea.selectionStart, end = textarea.selectionEnd;
  const selected = textarea.value.slice(start, end) || placeholder;
  textarea.setRangeText(before + selected + after, start, end, 'select');
  textarea.dispatchEvent(new Event('input'));
  textarea.focus();
}

// Prefixes every line touched by the current selection (or just the current
// line, if nothing is selected) with the given text — how a heading or a
// list marker gets applied, since those belong at the start of a line rather
// than wrapped around a selection.
function mdPrefixLines(textarea, prefix) {
  const value = textarea.value;
  let lineStart = value.lastIndexOf('\n', textarea.selectionStart - 1) + 1;
  let lineEnd = value.indexOf('\n', textarea.selectionEnd);
  if (lineEnd === -1) lineEnd = value.length;
  const block = value.slice(lineStart, lineEnd);
  const updated = block.split('\n').map((line) => prefix + line).join('\n');
  textarea.setRangeText(updated, lineStart, lineEnd, 'end');
  textarea.dispatchEvent(new Event('input'));
  textarea.focus();
}

function mdInsertBlock(textarea, snippet) {
  const pos = textarea.selectionEnd;
  const needsLeadingBreak = pos > 0 && textarea.value[pos - 1] !== '\n';
  const text = (needsLeadingBreak ? '\n' : '') + snippet;
  textarea.setRangeText(text, pos, pos, 'end');
  textarea.dispatchEvent(new Event('input'));
  textarea.focus();
}

function mdInsertLink(textarea, isImage) {
  const url = window.prompt(isImage ? 'Image URL' : 'Link URL', 'https://');
  if (!url) return;
  const start = textarea.selectionStart, end = textarea.selectionEnd;
  const label = textarea.value.slice(start, end) || (isImage ? 'description' : 'link text');
  const markup = (isImage ? '![' : '[') + label + '](' + url + ')';
  textarea.setRangeText(markup, start, end, 'end');
  textarea.dispatchEvent(new Event('input'));
  textarea.focus();
}

const MD_ACTIONS = {
  bold: (t) => mdWrap(t, '**', '**', 'bold text'),
  italic: (t) => mdWrap(t, '*', '*', 'italic text'),
  underline: (t) => mdWrap(t, '<u>', '</u>', 'underlined text'),
  strike: (t) => mdWrap(t, '<s>', '</s>', 'struck-through text'),
  heading: (t) => mdPrefixLines(t, '## '),
  ul: (t) => mdPrefixLines(t, '- '),
  ol: (t) => mdPrefixLines(t, '1. '),
  table: (t) => mdInsertBlock(t,
    '\n| Column 1 | Column 2 |\n| --- | --- |\n| Value | Value |\n'),
  link: (t) => mdInsertLink(t, false),
  image: (t) => mdInsertLink(t, true),
};

function initMarkdownEditors(root = document) {
  root.querySelectorAll('[data-markdown-editor]').forEach((wrapper) => {
    if (wrapper.dataset.mdReady) return;
    wrapper.dataset.mdReady = '1';

    const textarea = wrapper.querySelector('textarea');
    const preview = wrapper.querySelector('.md-preview');
    const toggle = wrapper.querySelector('[data-md-preview-toggle]');
    if (!textarea) return;

    wrapper.querySelectorAll('[data-md]').forEach((button) => {
      button.addEventListener('click', () => {
        const action = MD_ACTIONS[button.dataset.md];
        if (action) action(textarea);
        if (preview && !preview.hidden) preview.innerHTML = mdToHtml(textarea.value);
      });
    });

    if (toggle && preview) {
      const showPreview = () => {
        preview.innerHTML = mdToHtml(textarea.value);
        preview.hidden = false;
        textarea.hidden = true;
        toggle.textContent = 'Write';
      };
      const showWrite = () => {
        preview.hidden = true;
        textarea.hidden = false;
        toggle.textContent = 'Preview';
        textarea.focus();
      };
      toggle.addEventListener('click', () => {
        (preview.hidden ? showPreview : showWrite)();
      });
      textarea.addEventListener('input', () => {
        if (!preview.hidden) preview.innerHTML = mdToHtml(textarea.value);
      });
    }
  });
}

document.addEventListener('DOMContentLoaded', () => initMarkdownEditors());
