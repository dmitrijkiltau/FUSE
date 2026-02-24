const SESSION_STORAGE_KEY = "reference_service_session";

function escapeHtml(text) {
  const map = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;" };
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

function parseErrorDetails(text) {
  if (!text) return null;
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    return null;
  }
  if (!data?.error) return null;

  const err = data.error;
  let message = err.message || "Request failed";

  if (Array.isArray(err.fields) && err.fields.length > 0) {
    message = err.fields
      .map(f => {
        const name = (f.path || "").replace(/^body\./, "") || "field";
        return `${name}: ${f.message || "invalid value"}`;
      })
      .join("\n");
  }

  let title = "Request failed";
  if (typeof err.code === "string" && err.code.length > 0) {
    title = err.code.replace(/_/g, " ");
    title = title.charAt(0).toUpperCase() + title.slice(1);
  }

  return { title, message };
}

async function safeErrorMessage(resp) {
  try {
    const details = parseErrorDetails(await resp.text());
    if (details) return details.message;
  } catch {
    // ignore parse/read errors
  }
  return `Request failed (${resp.status})`;
}

function normalizeSession(data) {
  if (!data || typeof data !== "object") return null;
  const token = data?.token?.value ?? data?.token ?? null;
  const userId = data?.userId?.value ?? data?.userId ?? null;
  if (typeof token !== "string" || token.length === 0) return null;
  if (typeof userId !== "string" || userId.length === 0) return null;
  return {
    token,
    userId,
    scopes: Array.isArray(data.scopes) ? data.scopes : []
  };
}

function loadSession() {
  try {
    return normalizeSession(JSON.parse(localStorage.getItem(SESSION_STORAGE_KEY) || "null"));
  } catch {
    return null;
  }
}

function persistSession(session) {
  if (!session) {
    localStorage.removeItem(SESSION_STORAGE_KEY);
    return;
  }
  localStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(session));
}

function notesEndpoint(token, noteId) {
  const base = `/api/sessions/${token}/notes`;
  return noteId ? `${base}/${noteId}` : base;
}

function noteVisibilityEndpoint(token, noteId) {
  return `/api/sessions/${token}/notes/${noteId}/visibility`;
}

function logoutEndpoint(token) {
  return `/api/auth/sessions/${token}`;
}

function publicNotesEndpoint(noteId) {
  const base = "/api/public/notes";
  return noteId ? `${base}/${noteId}` : base;
}

const registerForm = document.getElementById("register-form");
const loginForm = document.getElementById("login-form");
const logoutButton = document.getElementById("logout-btn");
const sessionStatus = document.getElementById("session-status");
const authForms = document.getElementById("auth-forms");

const createNoteForm = document.getElementById("create-note-form");
const notesList = document.getElementById("notes-list");
const publicNotesList = document.getElementById("public-notes-list");
const createNoteSection = document.getElementById("create-note-section");
const notesSection = document.getElementById("notes-section");

const editDialog = document.getElementById("edit-dialog");
const editForm = document.getElementById("edit-form");
const editTitle = document.getElementById("edit-title");
const editContent = document.getElementById("edit-content");
const editCancel = document.getElementById("edit-cancel");

const confirmDialog = document.getElementById("confirm-dialog");
const confirmCancel = document.getElementById("confirm-cancel");
const confirmOk = document.getElementById("confirm-ok");

const errorDialog = document.getElementById("error-dialog");
const errorTitleEl = document.getElementById("error-title");
const errorMessage = document.getElementById("error-message");
const errorOk = document.getElementById("error-ok");

let currentSession = loadSession();
let pendingEditId = null;
let pendingDeleteId = null;

function showError(message, title) {
  errorTitleEl.textContent = title || "Request failed";
  errorMessage.textContent = message || "Request failed";
  showDialog(errorDialog);
}

function isPublished(note) {
  const raw = note?.is_public ?? note?.published ?? "0";
  return raw === true || raw === 1 || raw === "1" || raw === "true";
}

function renderPrivateNoteCard(note) {
  const published = isPublished(note);
  const visibility = published ? "Public" : "Private";
  const publishLabel = published ? "Unpublish" : "Publish";
  const nextPublished = published ? "0" : "1";

  return `
    <div class="note-card" data-id="${escapeAttr(note.id)}" data-title="${escapeAttr(note.title)}" data-content="${escapeAttr(note.content)}" data-published="${published ? "1" : "0"}">
      <h3 class="note-card__title">${escapeHtml(note.title)}</h3>
      <p class="note-card__body">${escapeHtml(note.content)}</p>
      <div class="note-card__meta-line">
        <div class="note-card__meta">${escapeHtml(note.id)}</div>
        <span class="note-card__pill">${visibility}</span>
      </div>
      <div class="note-card__actions">
        <button type="button" class="btn btn--ghost btn--sm edit-btn">Edit</button>
        <button type="button" class="btn btn--ghost btn--sm publish-btn" data-next-published="${nextPublished}">${publishLabel}</button>
        <button type="button" class="btn btn--danger btn--sm delete-btn">Delete</button>
      </div>
    </div>`;
}

function renderPublicNoteCard(note) {
  return `
    <div class="note-card note-card--public" data-id="${escapeAttr(note.id)}">
      <h3 class="note-card__title">${escapeHtml(note.title)}</h3>
      <p class="note-card__body">${escapeHtml(note.content)}</p>
      <div class="note-card__meta">${escapeHtml(note.id)} by ${escapeHtml(note.owner_id || "unknown")}</div>
    </div>`;
}

function setCreateFormEnabled(enabled) {
  createNoteForm.querySelectorAll("input, textarea, button").forEach(el => {
    el.disabled = !enabled;
  });
}

function initPublicNotesFeed() {
  publicNotesList.setAttribute("hx-get", publicNotesEndpoint());
  publicNotesList.setAttribute("hx-trigger", "load, publicNotesRefresh, every 5s");
  if (window.htmx?.process) window.htmx.process(publicNotesList);
}

function refreshPrivateNotes() {
  if (window.htmx?.trigger) window.htmx.trigger("#notes-list", "notesRefresh");
}

function refreshPublicNotes() {
  if (window.htmx?.trigger) window.htmx.trigger("#public-notes-list", "publicNotesRefresh");
}

function setSession(session) {
  currentSession = session;
  persistSession(session);

  if (!currentSession) {
    sessionStatus.textContent = "Not signed in.";
    authForms.classList.remove("hidden");
    logoutButton.classList.add("hidden");
    logoutButton.disabled = true;
    createNoteSection.classList.add("hidden");
    notesSection.classList.add("hidden");
    setCreateFormEnabled(false);
    createNoteForm.removeAttribute("hx-post");
    notesList.removeAttribute("hx-get");
    notesList.removeAttribute("hx-trigger");
    notesList.innerHTML = '<div class="state">Sign in to load your private notes.</div>';
    refreshPublicNotes();
    return;
  }

  sessionStatus.textContent = `Signed in as ${currentSession.userId}.`;
  authForms.classList.add("hidden");
  logoutButton.classList.remove("hidden");
  logoutButton.disabled = false;
  createNoteSection.classList.remove("hidden");
  notesSection.classList.remove("hidden");
  setCreateFormEnabled(true);

  const endpoint = notesEndpoint(currentSession.token);
  createNoteForm.setAttribute("hx-post", endpoint);
  notesList.setAttribute("hx-get", endpoint);
  notesList.setAttribute("hx-trigger", "load, notesRefresh, every 5s");
  if (window.htmx?.process) {
    window.htmx.process(createNoteForm);
    window.htmx.process(notesList);
  }
  refreshPrivateNotes();
  refreshPublicNotes();
}

async function submitAuth(endpoint, form) {
  const email = form.elements.email?.value?.trim() || "";
  const password = form.elements.password?.value || "";
  const resp = await fetch(endpoint, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email, password })
  });

  if (!resp.ok) {
    const details = parseErrorDetails(await resp.text());
    if (details) {
      showError(details.message, details.title);
      return;
    }
    showError(`Request failed (${resp.status})`);
    return;
  }

  let data = null;
  try {
    data = await resp.json();
  } catch {
    showError("Invalid session response from server.");
    return;
  }
  const session = normalizeSession(data);
  if (!session) {
    showError("Session response is missing token/user information.");
    return;
  }

  setSession(session);
  form.reset();
}

async function togglePublish(noteId, published) {
  if (!currentSession) return;
  const resp = await fetch(noteVisibilityEndpoint(currentSession.token, noteId), {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ published })
  });

  if (!resp.ok) {
    showError(await safeErrorMessage(resp));
    return;
  }

  refreshPrivateNotes();
  refreshPublicNotes();
}

registerForm.addEventListener("submit", async evt => {
  evt.preventDefault();
  await submitAuth("/api/auth/register", registerForm);
});

loginForm.addEventListener("submit", async evt => {
  evt.preventDefault();
  await submitAuth("/api/auth/login", loginForm);
});

logoutButton.addEventListener("click", async () => {
  if (!currentSession) {
    setSession(null);
    return;
  }
  const token = currentSession.token;
  const resp = await fetch(logoutEndpoint(token), { method: "DELETE" });
  if (!resp.ok) {
    showError(await safeErrorMessage(resp));
    return;
  }
  setSession(null);
});

document.body.addEventListener("htmx:beforeSwap", evt => {
  const target = evt.detail.target;
  if (!target || evt.detail.requestConfig?.verb !== "get") return;

  if (target.id === "notes-list") {
    if (!currentSession) return;
    try {
      const notes = JSON.parse(evt.detail.xhr.response);
      if (!Array.isArray(notes)) {
        const message = notes?.error?.message || "Unexpected response from server";
        target.innerHTML = `<div class="state state--error">${escapeHtml(message)}</div>`;
        evt.detail.shouldSwap = false;
        return;
      }
      if (notes.length === 0) {
        target.innerHTML = '<div class="state">No private notes yet - create one above.</div>';
        evt.detail.shouldSwap = false;
        return;
      }
      target.innerHTML = notes.map(renderPrivateNoteCard).join("");
      evt.detail.shouldSwap = false;
      return;
    } catch (err) {
      console.error("Failed to parse private notes response", err);
      return;
    }
  }

  if (target.id === "public-notes-list") {
    try {
      const notes = JSON.parse(evt.detail.xhr.response);
      if (!Array.isArray(notes)) {
        const message = notes?.error?.message || "Unexpected response from server";
        target.innerHTML = `<div class="state state--error">${escapeHtml(message)}</div>`;
        evt.detail.shouldSwap = false;
        return;
      }
      if (notes.length === 0) {
        target.innerHTML = '<div class="state">No public notes yet.</div>';
        evt.detail.shouldSwap = false;
        return;
      }
      target.innerHTML = notes.map(renderPublicNoteCard).join("");
      evt.detail.shouldSwap = false;
      return;
    } catch (err) {
      console.error("Failed to parse public notes response", err);
    }
  }
});

document.body.addEventListener("htmx:responseError", evt => {
  const xhr = evt.detail.xhr;
  if (!xhr) {
    showError("Request failed");
    return;
  }

  if (xhr.status === 401 && currentSession) {
    const details = parseErrorDetails(xhr.responseText);
    setSession(null);
    showError(
      details?.message || "Session expired or invalid. Please sign in again.",
      details?.title || "Unauthorized"
    );
    return;
  }

  if (!evt.detail?.elt || evt.detail.elt.tagName !== "FORM") return;
  const details = parseErrorDetails(xhr.responseText);
  if (details) {
    showError(details.message, details.title);
    return;
  }
  showError(`Request failed (${xhr.status})`);
});

editCancel.addEventListener("click", () => closeDialog(editDialog));
confirmCancel.addEventListener("click", () => closeDialog(confirmDialog));
errorOk.addEventListener("click", () => closeDialog(errorDialog));

editForm.addEventListener("submit", async evt => {
  evt.preventDefault();
  if (!pendingEditId || !currentSession) {
    closeDialog(editDialog);
    return;
  }

  const title = editTitle.value.trim();
  const content = editContent.value.trim();
  const resp = await fetch(notesEndpoint(currentSession.token, pendingEditId), {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ title, content })
  });

  if (!resp.ok) {
    showError(await safeErrorMessage(resp));
    return;
  }

  closeDialog(editDialog);
  pendingEditId = null;
  refreshPrivateNotes();
  refreshPublicNotes();
});

confirmOk.addEventListener("click", async () => {
  if (!pendingDeleteId || !currentSession) {
    closeDialog(confirmDialog);
    return;
  }

  const resp = await fetch(notesEndpoint(currentSession.token, pendingDeleteId), {
    method: "DELETE"
  });

  if (!resp.ok) {
    showError(await safeErrorMessage(resp));
    return;
  }

  const card = document.querySelector(`.note-card[data-id="${cssEscape(pendingDeleteId)}"]`);
  if (card) card.remove();

  closeDialog(confirmDialog);
  pendingDeleteId = null;
  refreshPrivateNotes();
  refreshPublicNotes();
});

document.body.addEventListener("click", async evt => {
  const button = evt.target.closest("button");
  if (!button) return;
  const card = button.closest(".note-card");
  if (!card) return;
  const id = card.dataset.id;
  if (!id) return;

  if (button.classList.contains("edit-btn")) {
    pendingEditId = id;
    editTitle.value = card.dataset.title || "";
    editContent.value = card.dataset.content || "";
    showDialog(editDialog);
    return;
  }

  if (button.classList.contains("publish-btn")) {
    const nextPublished = button.dataset.nextPublished === "1" ? "1" : "0";
    await togglePublish(id, nextPublished);
    return;
  }

  if (button.classList.contains("delete-btn")) {
    pendingDeleteId = id;
    showDialog(confirmDialog);
  }
});

initPublicNotesFeed();
setSession(currentSession);
