/**
 * FUSE Docs - Minimal JavaScript
 * 
 * This file handles only the essential client-side functionality:
 * - Markdown fetching and rendering for spec pages
 * - Code syntax highlighting
 * - Table of contents generation
 * - Mobile sidebar toggle
 * - OpenAPI interactive runner
 */

import { marked } from "./libs/marked_16.3.0_esm.min.js";
import defineFuseLanguage from "./modules/highlight-fuse.js";

// Initialize highlight.js with FUSE language support
function initHighlight() {
  const hljs = window.hljs;
  if (!hljs) return;

  if (!hljs.getLanguage("fuse")) {
    hljs.registerLanguage("fuse", defineFuseLanguage);
  }
}

// Detect and add language class for code blocks
function detectLanguage(code) {
  if (/\blanguage-[a-z0-9_-]+\b/i.test(code.className)) return;

  const text = code.textContent || "";
  const looksLikeFuse =
    /\b(app|service|config|type|enum|fn|match|spawn|await|box)\b/.test(text) ||
    /->/.test(text);

  if (looksLikeFuse) {
    code.classList.add("language-fuse");
  }
}

// Highlight all code blocks in a container
function highlightCode(container) {
  const hljs = window.hljs;
  if (!hljs || !container) return;

  for (const code of container.querySelectorAll("pre code")) {
    detectLanguage(code);
    hljs.highlightElement(code);
  }
}

// Generate slugified ID from text
function slugify(text) {
  return String(text || "")
    .trim()
    .toLowerCase()
    .replace(/['"`]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "") || "section";
}

// Ensure unique IDs for headings
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

// Add copy link buttons to headings
function addHeadingLinks(container) {
  const used = new Set();
  
  // Collect existing IDs
  for (const el of container.querySelectorAll("[id]")) {
    const id = String(el.id || "").trim();
    if (id) used.add(id);
  }

  const headings = container.querySelectorAll("h2, h3");
  
  for (const heading of headings) {
    const candidate = slugify(heading.id || heading.textContent);
    const id = ensureUniqueId(candidate, used);
    heading.id = id;

    // Skip if already has a copy link
    if (heading.querySelector(".heading-copy-link")) continue;

    const button = document.createElement("button");
    button.type = "button";
    button.className = "heading-copy-link";
    button.setAttribute("aria-label", "Copy link to section");
    button.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M10.6 13.4a1 1 0 0 0 1.4 0l2.8-2.8a3 3 0 1 0-4.2-4.2l-1.7 1.7a1 1 0 1 0 1.4 1.4l1.7-1.7a1 1 0 1 1 1.4 1.4L10.6 12a1 1 0 0 0 0 1.4Z"/><path d="M13.4 10.6a1 1 0 0 0-1.4 0L9.2 13.4a3 3 0 1 0 4.2 4.2l1.7-1.7a1 1 0 0 0-1.4-1.4l-1.7 1.7a1 1 0 0 1-1.4-1.4l2.8-2.8a1 1 0 0 0 0-1.4Z"/></svg>`;
    
    button.addEventListener("click", async () => {
      const url = new URL(window.location.href);
      url.hash = id;
      
      try {
        await navigator.clipboard.writeText(url.toString());
        button.classList.add("is-copied");
        setTimeout(() => button.classList.remove("is-copied"), 1500);
      } catch {
        // Fallback for older browsers
        const input = document.createElement("textarea");
        input.value = url.toString();
        input.style.position = "fixed";
        input.style.top = "-9999px";
        document.body.appendChild(input);
        input.select();
        document.execCommand("copy");
        input.remove();
      }
    });

    heading.appendChild(button);
  }
}

// Generate table of contents
function generateToc(container, tocEl) {
  if (!container || !tocEl) return;

  const headings = Array.from(container.querySelectorAll("h2, h3"))
    .filter(h => String(h.textContent || "").trim().length > 0);

  if (headings.length === 0) {
    tocEl.hidden = true;
    return;
  }

  tocEl.hidden = false;
  tocEl.innerHTML = "";

  const links = [];

  for (const heading of headings) {
    const link = document.createElement("a");
    link.href = `#${heading.id}`;
    link.textContent = heading.textContent?.replace(/[\u00B6\u{1F517}]/gu, "").trim() || heading.id;
    link.className = `toc-item toc-${heading.tagName.toLowerCase()}`;
    
    tocEl.appendChild(link);
    links.push({ link, heading });
  }

  // Scroll spy for active TOC item
  let ticking = false;
  const updateActive = () => {
    let activeIndex = 0;
    
    for (let i = 0; i < links.length; i++) {
      const rect = links[i].heading.getBoundingClientRect();
      if (rect.top <= 100) {
        activeIndex = i;
      } else {
        break;
      }
    }

    links.forEach((item, i) => {
      item.link.classList.toggle("is-active", i === activeIndex);
    });
  };

  window.addEventListener("scroll", () => {
    if (!ticking) {
      ticking = true;
      requestAnimationFrame(() => {
        updateActive();
        ticking = false;
      });
    }
  }, { passive: true });

  updateActive();
}

// Load and render markdown spec
async function loadSpec(slug, container) {
  const specMap = {
    fuse: "/fuse.md",
    fls: "/fls.md",
    runtime: "/runtime.md",
    scope: "/scope.md",
  };

  const path = specMap[slug];
  if (!path) {
    container.innerHTML = `<p class="muted">Unknown spec: ${slug}</p>`;
    return;
  }

  try {
    const response = await fetch(path);
    if (!response.ok) throw new Error(`Failed to load ${path}`);
    
    const markdown = await response.text();
    container.innerHTML = marked.parse(markdown);
    
    highlightCode(container);
    addHeadingLinks(container);
    
    const tocEl = document.getElementById("toc");
    if (tocEl) generateToc(container, tocEl);

    // Handle initial hash navigation
    if (window.location.hash) {
      const target = document.getElementById(window.location.hash.slice(1));
      if (target) {
        setTimeout(() => target.scrollIntoView({ block: "start" }), 100);
      }
    }
  } catch (error) {
    container.innerHTML = `<p class="muted">${error.message}</p>`;
  }
}

// Load and render OpenAPI
async function loadOpenApi(container) {
  try {
    const response = await fetch("/site/openapi.json");
    if (!response.ok) throw new Error("Failed to load OpenAPI spec");
    
    const doc = await response.json();
    renderOpenApi(doc, container);
  } catch (error) {
    container.innerHTML = `
      <h1>API Reference</h1>
      <p class="muted">${error.message}</p>
      <p class="muted">Run <code>./scripts/fuse build --manifest-path docs</code> to generate the OpenAPI spec.</p>
    `;
  }
}

function escapeHtml(text) {
  return String(text)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function renderOpenApi(doc, container) {
  const title = doc.info?.title || "API Reference";
  const version = doc.info?.version || "";

  const endpoints = [];
  for (const [route, ops] of Object.entries(doc.paths || {})) {
    for (const [method, op] of Object.entries(ops || {})) {
      const summary = op.summary || op.operationId || "";
      const responses = op.responses ? Object.keys(op.responses).join(", ") : "";
      
      endpoints.push(`
        <article class="endpoint endpoint-interactive">
          <button class="endpoint-trigger" type="button" 
            data-method="${method.toUpperCase()}" 
            data-path="${escapeHtml(route)}">
            <div>
              <span class="method">${escapeHtml(method)}</span>
              <span class="path">${escapeHtml(route)}</span>
            </div>
            ${summary ? `<div class="summary">${escapeHtml(summary)}</div>` : ""}
            ${responses ? `<div class="muted">responses: ${escapeHtml(responses)}</div>` : ""}
          </button>
        </article>
      `);
    }
  }

  const schemas = [];
  for (const [name, schema] of Object.entries(doc.components?.schemas || {})) {
    const type = schema.type || "object";
    const props = schema.properties ? Object.keys(schema.properties).join(", ") : "-";
    
    schemas.push(`
      <article class="endpoint">
        <div><span class="path">${escapeHtml(name)}</span></div>
        <div class="summary">type: ${escapeHtml(type)}</div>
        <div class="muted">fields: ${escapeHtml(props)}</div>
      </article>
    `);
  }

  container.innerHTML = `
    <h1>${escapeHtml(title)}</h1>
    ${version ? `<p class="muted">version ${escapeHtml(version)}</p>` : ""}
    
    <div class="openapi-runner">
      <div class="openapi-runner__endpoints">
        <h2>Endpoints</h2>
        ${endpoints.join("\n") || '<p class="muted">No endpoints.</p>'}
      </div>
      <div class="openapi-runner__output">
        <h2>Response</h2>
        <p class="muted" id="output-placeholder">Click an endpoint to test it.</p>
        <article class="openapi-output" id="openapi-output" hidden>
          <div class="openapi-output-head">
            <span class="path" id="output-target"></span>
            <span class="muted" id="output-status"></span>
          </div>
          <pre><code id="output-body"></code></pre>
        </article>
      </div>
    </div>
    
    <h2>Schemas</h2>
    ${schemas.join("\n") || '<p class="muted">No schemas.</p>'}
  `;

  // Wire up endpoint triggers
  for (const trigger of container.querySelectorAll(".endpoint-trigger")) {
    trigger.addEventListener("click", async () => {
      const method = trigger.dataset.method || "GET";
      const path = trigger.dataset.path || "";
      
      // Update UI
      container.querySelectorAll(".endpoint-trigger").forEach(t => t.classList.remove("is-active"));
      trigger.classList.add("is-active");
      
      const output = document.getElementById("openapi-output");
      const placeholder = document.getElementById("output-placeholder");
      const targetEl = document.getElementById("output-target");
      const statusEl = document.getElementById("output-status");
      const bodyEl = document.getElementById("output-body");
      
      if (placeholder) placeholder.hidden = true;
      if (output) output.hidden = false;
      if (targetEl) targetEl.textContent = `${method} ${path}`;
      if (statusEl) statusEl.textContent = "loading...";
      if (bodyEl) bodyEl.textContent = "";

      try {
        const response = await fetch(path, {
          method,
          headers: { Accept: "application/json" },
        });

        const contentType = response.headers.get("content-type") || "";
        let body = "";
        
        if (contentType.includes("application/json")) {
          const json = await response.json();
          body = JSON.stringify(json, null, 2);
        } else {
          body = await response.text();
        }

        if (statusEl) statusEl.textContent = `${response.status} ${response.statusText}`;
        if (bodyEl) {
          bodyEl.textContent = body || "(empty response)";
          bodyEl.className = contentType.includes("json") ? "language-json" : "";
          if (window.hljs) window.hljs.highlightElement(bodyEl);
        }
      } catch (error) {
        if (statusEl) statusEl.textContent = "request failed";
        if (bodyEl) bodyEl.textContent = error.message;
      }
    });
  }
}

// Mobile sidebar toggle
function initSidebar() {
  const toggle = document.getElementById("sidebar-toggle");
  const overlay = document.getElementById("panel-overlay");
  const grid = document.querySelector(".content-grid");

  if (!toggle || !grid) return;

  const setSidebarOpen = (open) => {
    grid.classList.toggle("is-sidebar-open", open);
    toggle.setAttribute("aria-expanded", String(open));
    if (overlay) overlay.hidden = !open;
  };

  toggle.addEventListener("click", () => {
    const isOpen = grid.classList.contains("is-sidebar-open");
    setSidebarOpen(!isOpen);
  });

  if (overlay) {
    overlay.addEventListener("click", () => setSidebarOpen(false));
  }

  // Close on escape
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && grid.classList.contains("is-sidebar-open")) {
      setSidebarOpen(false);
    }
  });
}

// Main initialization
function init() {
  initHighlight();
  initSidebar();

  // Check for spec content
  const specContent = document.querySelector(".spec-content");
  if (specContent) {
    const slug = specContent.dataset.spec;
    if (slug) loadSpec(slug, specContent);
  }

  // Check for OpenAPI content
  const openapiContent = document.querySelector(".openapi-content");
  if (openapiContent) {
    loadOpenApi(openapiContent);
  }
}

// Run when DOM is ready
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", init);
} else {
  init();
}
