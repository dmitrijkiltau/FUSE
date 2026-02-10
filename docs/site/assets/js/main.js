import { enhanceHeadingLinks } from "./modules/heading-links.js";
import { generateToc } from "./modules/toc.js";
import {
  loadOpenApi,
  renderOpenApiHtml,
  wireOpenApiInteractions,
} from "./modules/openapi.js";
import {
  enhanceSpecDom,
  loadSpec,
  renderSpecHtml,
  specFiles,
} from "./modules/specs.js";

const viewRoot = document.querySelector("#view-root");
const specNav = document.querySelector("#spec-nav");
const tabs = Array.from(document.querySelectorAll(".tab"));
const contentGrid = document.querySelector(".content-grid");
const sidebarToggle = document.querySelector("#sidebar-toggle");
const panelOverlay = document.querySelector("#panel-overlay");
const mobileQuery = window.matchMedia("(max-width: 768px)");

let currentView = "specs";
let currentSpecId = "fuse";
let sidebarOpen = false;

function routeFromHash() {
  const raw = window.location.hash.replace(/^#/, "").trim().toLowerCase();
  if (!raw) {
    return { view: "specs", section: "" };
  }

  const [prefix, ...sectionParts] = raw.split(":");
  if (prefix === "openapi" || prefix === "specs") {
    return {
      view: prefix,
      section: decodeURIComponent(sectionParts.join(":")),
    };
  }

  return { view: "specs", section: decodeURIComponent(raw) };
}

function syncHash(view, section = "") {
  const encodedSection = section ? `:${encodeURIComponent(section)}` : "";
  const next = `#${view}${encodedSection}`;
  if (window.location.hash !== next) {
    window.location.hash = next;
  }
}

function scrollToSection(section) {
  if (!section) {
    return;
  }
  const target = document.getElementById(section);
  if (target) {
    target.scrollIntoView({ block: "start", behavior: "smooth" });
  }
}

function isMobileSidebarMode() {
  return mobileQuery.matches;
}

function setSidebarOpen(open) {
  sidebarOpen = open;
  contentGrid.classList.toggle("is-sidebar-open", open);
  sidebarToggle.setAttribute("aria-expanded", String(open));
  panelOverlay.hidden = !open;
}

function syncSidebarUi() {
  const hasSidebar = !specNav.hidden;
  contentGrid.classList.toggle("has-sidebar", hasSidebar);
  const showToggle = hasSidebar && isMobileSidebarMode();
  sidebarToggle.hidden = !showToggle;

  if (!showToggle) {
    setSidebarOpen(false);
    return;
  }

  if (!sidebarOpen) {
    panelOverlay.hidden = true;
  }
}

function setActiveTab(view) {
  for (const tab of tabs) {
    tab.classList.toggle("is-active", tab.dataset.view === view);
  }
}

function setLoading(label) {
  viewRoot.innerHTML = `<p class=\"muted\">${label}</p>`;
}

function renderSpecNav() {
  const files = specFiles();
  specNav.innerHTML = files
    .map((spec) => {
      const active = spec.id === currentSpecId ? " is-active" : "";
      return `<button class=\"nav-item${active}\" data-spec=\"${spec.id}\" type=\"button\">${spec.title}</button>`;
    })
    .join("\n");

  for (const button of specNav.querySelectorAll(".nav-item")) {
    button.addEventListener("click", () => {
      currentSpecId = button.dataset.spec;
      renderSpecNav();
      if (isMobileSidebarMode()) {
        setSidebarOpen(false);
      }
      showSpecs();
    });
  }

  syncSidebarUi();
}

async function showSpecs({ updateHash = true, section = "" } = {}) {
  currentView = "specs";
  setActiveTab("specs");
  specNav.hidden = false;
  syncSidebarUi();
  if (updateHash) {
    syncHash("specs", section);
  }

  const spec = specFiles().find((item) => item.id === currentSpecId) || specFiles()[0];
  setLoading(`Loading ${spec.title}...`);
  try {
    const markdown = await loadSpec(spec.path);
    viewRoot.innerHTML = renderSpecHtml(markdown);
    enhanceSpecDom(viewRoot);
    enhanceHeadingLinks(viewRoot, "specs");
  } catch (error) {
    viewRoot.innerHTML = `<p class=\"muted\">${String(error.message || error)}</p>`;
  }
  generateToc();
  scrollToSection(section);
}

async function showOpenApi({ updateHash = true, section = "" } = {}) {
  currentView = "openapi";
  setActiveTab("openapi");
  specNav.hidden = true;
  syncSidebarUi();
  if (updateHash) {
    syncHash("openapi", section);
  }
  setLoading("Loading OpenAPI...");

  try {
    const doc = await loadOpenApi("/site/openapi.json");
    viewRoot.innerHTML = renderOpenApiHtml(doc);
    wireOpenApiInteractions(viewRoot);
    enhanceHeadingLinks(viewRoot, "openapi");
    scrollToSection(section);
  } catch (error) {
    viewRoot.innerHTML = `
      <h1>OpenAPI</h1>
      <p class="muted">${String(error.message || error)}</p>
      <p class="muted">Run <code>./scripts/fuse build --manifest-path docs</code> to generate <code>docs/site/openapi.json</code>.</p>
    `;
  }
}

for (const tab of tabs) {
  tab.addEventListener("click", () => {
    const view = tab.dataset.view;
    if (view === "openapi") {
      showOpenApi();
      return;
    }
    showSpecs();
  });
}

sidebarToggle.addEventListener("click", () => {
  setSidebarOpen(!sidebarOpen);
});

panelOverlay.addEventListener("click", () => {
  setSidebarOpen(false);
});

mobileQuery.addEventListener("change", () => {
  syncSidebarUi();
});

window.addEventListener("hashchange", () => {
  const route = routeFromHash();
  if (route.view === currentView) {
    scrollToSection(route.section);
    return;
  }
  if (route.view === "openapi") {
    showOpenApi({ updateHash: false, section: route.section });
    return;
  }
  showSpecs({ updateHash: false, section: route.section });
});

renderSpecNav();
const initialRoute = routeFromHash();
if (initialRoute.view === "openapi") {
  showOpenApi({ updateHash: false, section: initialRoute.section });
} else {
  showSpecs({ updateHash: false, section: initialRoute.section });
}
