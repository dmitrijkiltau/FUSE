import { marked } from "../libs/marked_16.3.0_esm.min.js";
import defineFuseLanguage from "./highlight-fuse.js";

const SPECS = [
  { id: "reference", title: "Developer Reference", path: "/site/specs/reference.md" },
];

let highlightInitialized = false;

function ensureHighlightReady() {
  const hljs = window.hljs;
  if (!hljs || highlightInitialized) {
    return;
  }

  if (!hljs.getLanguage("fuse")) {
    hljs.registerLanguage("fuse", defineFuseLanguage);
  }
  highlightInitialized = true;
}

function ensureLanguageClass(code) {
  if (/\blanguage-[a-z0-9_-]+\b/i.test(code.className)) {
    return;
  }

  const text = code.textContent || "";
  const looksLikeFuse =
    /\b(app|service|config|type|enum|fn|match|spawn|await|box)\b/.test(text) ||
    /->/.test(text);
  if (looksLikeFuse) {
    code.classList.add("language-fuse");
  }
}

export function specFiles() {
  return SPECS;
}

export async function loadSpec(path) {
  const response = await fetch(path);
  if (!response.ok) {
    throw new Error(`failed to load ${path}`);
  }
  return response.text();
}

export function renderSpecHtml(markdown) {
  return marked.parse(markdown);
}

export function enhanceSpecDom(root) {
  ensureHighlightReady();
  const hljs = window.hljs;
  if (!hljs || !root) {
    return;
  }

  for (const code of root.querySelectorAll("pre code")) {
    ensureLanguageClass(code);
    hljs.highlightElement(code);
  }
}
