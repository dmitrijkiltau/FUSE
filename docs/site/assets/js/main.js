import { loadOpenApi, renderOpenApiHtml } from "./modules/openapi.js";
import { loadSpec, renderSpecHtml, specFiles } from "./modules/specs.js";

const viewRoot = document.querySelector("#view-root");
const specNav = document.querySelector("#spec-nav");
const tabs = Array.from(document.querySelectorAll(".tab"));

let currentView = "specs";
let currentSpecId = "fuse";

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
      showSpecs();
    });
  }
}

async function showSpecs() {
  currentView = "specs";
  setActiveTab("specs");
  specNav.hidden = false;

  const spec = specFiles().find((item) => item.id === currentSpecId) || specFiles()[0];
  setLoading(`Loading ${spec.title}...`);
  try {
    const markdown = await loadSpec(spec.path);
    viewRoot.innerHTML = renderSpecHtml(markdown);
  } catch (error) {
    viewRoot.innerHTML = `<p class=\"muted\">${String(error.message || error)}</p>`;
  }
}

async function showOpenApi() {
  currentView = "openapi";
  setActiveTab("openapi");
  specNav.hidden = true;
  setLoading("Loading OpenAPI...");

  try {
    const doc = await loadOpenApi("/site/openapi.json");
    viewRoot.innerHTML = renderOpenApiHtml(doc);
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

renderSpecNav();
if (currentView === "specs") {
  showSpecs();
}
