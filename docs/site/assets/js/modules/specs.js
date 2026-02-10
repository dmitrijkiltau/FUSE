const SPEC_FILES = [
  { id: "fuse", title: "Product", path: "/fuse.md" },
  { id: "fls", title: "Language Spec", path: "/fls.md" },
  { id: "runtime", title: "Runtime", path: "/runtime.md" },
  { id: "scope", title: "Scope", path: "/scope.md" },
];

function escapeHtml(text) {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function renderMarkdown(markdown) {
  const lines = markdown.replaceAll("\r", "").split("\n");
  const out = [];
  let inCode = false;
  let inList = false;

  for (const rawLine of lines) {
    const line = rawLine.trimEnd();
    if (line.startsWith("```") ) {
      if (!inCode) {
        if (inList) {
          out.push("</ul>");
          inList = false;
        }
        inCode = true;
        out.push("<pre><code>");
      } else {
        inCode = false;
        out.push("</code></pre>");
      }
      continue;
    }

    if (inCode) {
      out.push(`${escapeHtml(rawLine)}\n`);
      continue;
    }

    if (line === "") {
      if (inList) {
        out.push("</ul>");
        inList = false;
      }
      continue;
    }

    if (line.startsWith("### ")) {
      if (inList) {
        out.push("</ul>");
        inList = false;
      }
      out.push(`<h3>${escapeHtml(line.slice(4))}</h3>`);
      continue;
    }

    if (line.startsWith("## ")) {
      if (inList) {
        out.push("</ul>");
        inList = false;
      }
      out.push(`<h2>${escapeHtml(line.slice(3))}</h2>`);
      continue;
    }

    if (line.startsWith("# ")) {
      if (inList) {
        out.push("</ul>");
        inList = false;
      }
      out.push(`<h1>${escapeHtml(line.slice(2))}</h1>`);
      continue;
    }

    if (line.startsWith("- ") || line.startsWith("* ")) {
      if (!inList) {
        out.push("<ul>");
        inList = true;
      }
      out.push(`<li>${escapeHtml(line.slice(2))}</li>`);
      continue;
    }

    if (inList) {
      out.push("</ul>");
      inList = false;
    }
    out.push(`<p>${escapeHtml(line)}</p>`);
  }

  if (inList) {
    out.push("</ul>");
  }
  if (inCode) {
    out.push("</code></pre>");
  }
  return out.join("\n");
}

export function specFiles() {
  return SPEC_FILES;
}

export async function loadSpec(path) {
  const res = await fetch(path);
  if (!res.ok) {
    throw new Error(`failed to load ${path}`);
  }
  return res.text();
}

export function renderSpecHtml(markdown) {
  return renderMarkdown(markdown);
}
