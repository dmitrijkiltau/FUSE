// — Helpers —

function escapeHtml(text) {
  const map = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;' };
  return String(text).replace(/[&<>"']/g, m => map[m]);
}

function escapeAttr(text) {
  return escapeHtml(String(text));
}

function cssEscape(text) {
  return window.CSS?.escape ? CSS.escape(text) : String(text).replace(/"/g, '\\"');
}

function showDialog(dialog) {
  if (dialog?.showModal) dialog.showModal();
}

function closeDialog(dialog) {
  if (dialog?.open) dialog.close();
}

// — Error parsing —

function parseErrorDetails(text) {
  if (!text) return null;
  let data;
  try { data = JSON.parse(text); } catch { return null; }
  if (!data?.error) return null;

  const err = data.error;
  let message = err.message || 'Request failed';

  if (Array.isArray(err.fields) && err.fields.length > 0) {
    message = err.fields
      .map(f => {
        const name = (f.path || '').replace(/^body\./, '') || 'field';
        return `${name}: ${f.message || 'invalid value'}`;
      })
      .join('\n');
  }

  let title = 'Request failed';
  if (typeof err.code === 'string' && err.code.length > 0) {
    title = err.code.replace(/_/g, ' ');
    title = title.charAt(0).toUpperCase() + title.slice(1);
  }

  return { title, message };
}

async function safeErrorMessage(resp) {
  try {
    const details = parseErrorDetails(await resp.text());
    if (details) return details.message;
  } catch { /* ignore */ }
  return `Request failed (${resp.status})`;
}

// — DOM refs —

const editDialog    = document.getElementById('edit-dialog');
const editForm      = document.getElementById('edit-form');
const editTitle     = document.getElementById('edit-title');
const editContent   = document.getElementById('edit-content');
const editCancel    = document.getElementById('edit-cancel');

const confirmDialog = document.getElementById('confirm-dialog');
const confirmCancel = document.getElementById('confirm-cancel');
const confirmOk     = document.getElementById('confirm-ok');

const errorDialog   = document.getElementById('error-dialog');
const errorTitleEl  = document.getElementById('error-title');
const errorMessage  = document.getElementById('error-message');
const errorOk       = document.getElementById('error-ok');

let pendingEditId   = null;
let pendingDeleteId = null;

// — Error dialog —

function showError(message, title) {
  errorTitleEl.textContent = title || 'Request failed';
  errorMessage.textContent = message || 'Request failed';
  showDialog(errorDialog);
}

// — Render note card HTML —

function renderNoteCard(note) {
  return `
    <div class="note-card" data-id="${escapeAttr(note.id)}" data-title="${escapeAttr(note.title)}" data-content="${escapeAttr(note.content)}">
      <h3 class="note-card__title">${escapeHtml(note.title)}</h3>
      <p class="note-card__body">${escapeHtml(note.content)}</p>
      <div class="note-card__meta">${escapeHtml(note.id)}</div>
      <div class="note-card__actions">
        <button type="button" class="btn btn--ghost btn--sm edit-btn">Edit</button>
        <button type="button" class="btn btn--danger btn--sm delete-btn">Delete</button>
      </div>
    </div>`;
}

// — htmx: transform JSON → HTML —

document.body.addEventListener('htmx:beforeSwap', function(evt) {
  if (evt.detail.target.id !== 'notes-list') return;

  // Only process GET requests (list endpoint), not POST (create endpoint)
  if (evt.detail.requestConfig.verb !== 'get') return;

  try {
    const notes = JSON.parse(evt.detail.xhr.response);

    if (!Array.isArray(notes)) {
      const message = notes?.error?.message || 'Unexpected response from server';
      evt.detail.target.innerHTML = `<div class="state state--error">${escapeHtml(message)}</div>`;
      evt.detail.shouldSwap = false;
      return;
    }

    if (notes.length === 0) {
      evt.detail.target.innerHTML = '<div class="state">No notes yet — create one above.</div>';
      evt.detail.shouldSwap = false;
      return;
    }

    evt.detail.target.innerHTML = notes.map(renderNoteCard).join('');
    evt.detail.shouldSwap = false;
  } catch (e) {
    console.error('Parse error:', e);
  }
});

// — htmx: refresh after create — (handled inline via hx-on::after-request)

// — htmx: form-level errors —

document.body.addEventListener('htmx:responseError', function(evt) {
  if (!evt.detail?.elt || evt.detail.elt.tagName !== 'FORM') return;
  const xhr = evt.detail.xhr;
  if (!xhr) { showError('Request failed'); return; }

  const details = parseErrorDetails(xhr.responseText);
  if (details) { showError(details.message, details.title); return; }
  showError(`Request failed (${xhr.status})`);
});

// — Dialog: close handlers —

editCancel.addEventListener('click', () => closeDialog(editDialog));
confirmCancel.addEventListener('click', () => closeDialog(confirmDialog));
errorOk.addEventListener('click', () => closeDialog(errorDialog));

// — Dialog: edit submit —

editForm.addEventListener('submit', async function(evt) {
  evt.preventDefault();
  if (!pendingEditId) { closeDialog(editDialog); return; }

  const title   = editTitle.value.trim();
  const content = editContent.value.trim();

  const resp = await fetch(`/api/notes/${encodeURIComponent(pendingEditId)}`, {
    method:  'PUT',
    headers: { 'Content-Type': 'application/json' },
    body:    JSON.stringify({ title, content })
  });

  if (!resp.ok) { showError(await safeErrorMessage(resp)); return; }

  closeDialog(editDialog);
  pendingEditId = null;
  htmx.trigger('#notes-list', 'notesRefresh');
});

// — Dialog: confirm delete —

confirmOk.addEventListener('click', async function() {
  if (!pendingDeleteId) { closeDialog(confirmDialog); return; }

  const resp = await fetch(`/api/notes/${encodeURIComponent(pendingDeleteId)}`, {
    method: 'DELETE'
  });

  if (!resp.ok) { showError(await safeErrorMessage(resp)); return; }

  const card = document.querySelector(`.note-card[data-id="${cssEscape(pendingDeleteId)}"]`);
  if (card) card.remove();

  closeDialog(confirmDialog);
  pendingDeleteId = null;
});

// — Card button delegation —

document.body.addEventListener('click', function(evt) {
  const button = evt.target.closest('button');
  if (!button) return;
  const card = button.closest('.note-card');
  if (!card) return;
  const id = card.dataset.id;
  if (!id) return;

  if (button.classList.contains('edit-btn')) {
    pendingEditId = id;
    editTitle.value   = card.dataset.title || '';
    editContent.value = card.dataset.content || '';
    showDialog(editDialog);
    return;
  }

  if (button.classList.contains('delete-btn')) {
    pendingDeleteId = id;
    showDialog(confirmDialog);
  }
});
