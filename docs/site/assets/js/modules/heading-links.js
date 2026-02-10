function slugify(text) {
  const base = String(text || "")
    .trim()
    .toLowerCase()
    .replace(/['"`]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return base || "section";
}

function ensureUniqueId(candidate, used) {
  let id = candidate;
  let suffix = 2;
  while (used.has(id)) {
    id = `${candidate}-${suffix}`;
    suffix += 1;
  }
  used.add(id);
  return id;
}

function ensureHeadingIds(root, headings) {
  const used = new Set();
  for (const node of root.querySelectorAll("[id]")) {
    const value = String(node.id || "").trim();
    if (value) {
      used.add(value);
    }
  }

  for (const heading of headings) {
    const current = String(heading.id || "").trim();
    const candidate = slugify(current || heading.textContent);
    const id = ensureUniqueId(candidate, used);
    heading.id = id;
  }
}

function copyText(text) {
  if (navigator.clipboard && typeof navigator.clipboard.writeText === "function") {
    return navigator.clipboard.writeText(text);
  }

  const input = document.createElement("textarea");
  input.value = text;
  input.setAttribute("readonly", "true");
  input.style.position = "fixed";
  input.style.top = "-2000px";
  document.body.appendChild(input);
  input.select();
  document.execCommand("copy");
  input.remove();
  return Promise.resolve();
}

function copyIconMarkup() {
  return `
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M10.6 13.4a1 1 0 0 0 1.4 0l2.8-2.8a3 3 0 1 0-4.2-4.2l-1.7 1.7a1 1 0 1 0 1.4 1.4l1.7-1.7a1 1 0 1 1 1.4 1.4L10.6 12a1 1 0 0 0 0 1.4Z"/>
      <path d="M13.4 10.6a1 1 0 0 0-1.4 0L9.2 13.4a3 3 0 1 0 4.2 4.2l1.7-1.7a1 1 0 0 0-1.4-1.4l-1.7 1.7a1 1 0 0 1-1.4-1.4l2.8-2.8a1 1 0 0 0 0-1.4Z"/>
    </svg>
  `;
}

function buildAbsoluteLink(view, id) {
  const url = new URL(window.location.href);
  url.hash = `${view}:${encodeURIComponent(id)}`;
  return url.toString();
}

function mountCopyButton(heading, view) {
  const existing = heading.querySelector(".heading-copy-link");
  if (existing) {
    existing.dataset.docsView = view;
    return;
  }

  const button = document.createElement("button");
  button.type = "button";
  button.className = "heading-copy-link";
  button.dataset.docsView = view;
  button.setAttribute("aria-label", "Copy section link");
  button.setAttribute("title", "Copy link");
  button.innerHTML = copyIconMarkup();

  button.addEventListener("click", async event => {
    event.preventDefault();
    event.stopPropagation();
    const targetView = button.dataset.docsView || "specs";
    const href = buildAbsoluteLink(targetView, heading.id);
    await copyText(href);
    button.classList.add("is-copied");
    button.setAttribute("title", "Copied");
    window.setTimeout(() => {
      button.classList.remove("is-copied");
      button.setAttribute("title", "Copy link");
    }, 1100);
  });

  heading.appendChild(button);
}

export function enhanceHeadingLinks(root, view) {
  if (!root) {
    return;
  }

  const headings = Array.from(root.querySelectorAll("h2, h3")).filter(
    heading => String(heading.textContent || "").trim().length > 0,
  );
  if (headings.length === 0) {
    return;
  }

  ensureHeadingIds(root, headings);
  for (const heading of headings) {
    mountCopyButton(heading, view);
  }
}
