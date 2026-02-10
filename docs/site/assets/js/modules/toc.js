const TOC_OFFSET = 100;

const tocState = {
  links: [],
  headings: [],
  listenerAttached: false,
  framePending: false,
};

const slugify = text => {
  const base = String(text || "")
    .trim()
    .toLowerCase()
    .replace(/['"`]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return base || "section";
}

const uniqueId = (candidate, used) => {
  let id = candidate;
  let suffix = 2;
  while (used.has(id)) {
    id = `${candidate}-${suffix}`;
    suffix += 1;
  }
  used.add(id);
  return id;
}

const clearExistingToc = sidebar => {
  const existing = sidebar.querySelector(".toc");
  if (existing) {
    existing.remove();
  }
}

const resolveHeadingId = (heading, used) => {
  const fromHeading = String(heading.id || "").trim();
  const preferred = slugify(fromHeading || heading.textContent);
  const id = uniqueId(preferred, used);
  heading.id = id;
  return id;
}

const setActiveByIndex = activeIndex => {
  tocState.links.forEach((link, index) => {
    link.classList.toggle("is-active", index === activeIndex);
  });
}

const updateTocActiveLink = () => {
  if (tocState.headings.length === 0) {
    return;
  }

  let activeIndex = 0;
  for (let index = 0; index < tocState.headings.length; index += 1) {
    const rect = tocState.headings[index].getBoundingClientRect();
    if (rect.top <= TOC_OFFSET) {
      activeIndex = index;
    } else {
      break;
    }
  }

  setActiveByIndex(activeIndex);
}

const queueTocActiveUpdate = () => {
  if (tocState.framePending) {
    return;
  }
  tocState.framePending = true;
  window.requestAnimationFrame(() => {
    tocState.framePending = false;
    updateTocActiveLink();
  });
}

const attachListenersOnce = () => {
  if (tocState.listenerAttached) {
    return;
  }
  window.addEventListener("scroll", queueTocActiveUpdate, { passive: true });
  window.addEventListener("resize", queueTocActiveUpdate, { passive: true });
  tocState.listenerAttached = true;
}

const generateToc = () => {
  const sidebar = document.querySelector(".sidebar");
  const panel = document.querySelector(".panel");
  if (!sidebar || !panel) {
    return;
  }

  clearExistingToc(sidebar);
  tocState.links = [];
  tocState.headings = [];

  const headings = Array.from(panel.querySelectorAll("h2, h3"))
    .filter(heading => String(heading.textContent || "").trim().length > 0);
  if (headings.length === 0) {
    return;
  }

  const toc = document.createElement("nav");
  toc.classList.add("toc");
  sidebar.appendChild(toc);

  const usedIds = new Set();
  for (const heading of headings) {
    const id = resolveHeadingId(heading, usedIds);
    const link = document.createElement("a");
    link.href = `#${id}`;
    link.textContent = heading.textContent || id;
    link.classList.add("toc-item", `toc-${heading.tagName.toLowerCase()}`);
    link.dataset.tocTarget = id;
    link.addEventListener("click", () => {
      queueTocActiveUpdate();
    });

    toc.appendChild(link);
    tocState.headings.push(heading);
    tocState.links.push(link);
  }

  attachListenersOnce();
  queueTocActiveUpdate();
}

export { generateToc };
