function escapeHtml(text) {
  return String(text)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function escapeAttr(text) {
  return String(text)
    .replaceAll("&", "&amp;")
    .replaceAll("\"", "&quot;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

export async function loadOpenApi(path = "/site/openapi.json") {
  const res = await fetch(path);
  if (!res.ok) {
    throw new Error(`failed to load ${path}`);
  }
  return res.json();
}

function renderEndpoint(method, route, op) {
  const methodUpper = String(method).toUpperCase();
  const key = `${methodUpper} ${route}`;
  const summary = op.summary || op.operationId || "";
  const responses = op.responses ? Object.keys(op.responses).join(", ") : "";
  return `
    <article class="endpoint endpoint-interactive">
      <button
        class="endpoint-trigger"
        type="button"
        data-endpoint-key="${escapeAttr(key)}"
        data-method="${escapeAttr(methodUpper)}"
        data-path="${escapeAttr(route)}"
        aria-label="Call ${escapeAttr(key)}"
      >
        <div><span class="method">${escapeHtml(method)}</span><span class="path">${escapeHtml(route)}</span></div>
        ${summary ? `<div class="summary">${escapeHtml(summary)}</div>` : ""}
        ${responses ? `<div class="muted">responses: ${escapeHtml(responses)}</div>` : ""}
      </button>
    </article>
  `;
}

function renderSchema(name, schema) {
  const type = schema.type || "object";
  const props = schema.properties ? Object.keys(schema.properties) : [];
  return `
    <article class="endpoint">
      <div><span class="path">${escapeHtml(name)}</span></div>
      <div class="summary">type: ${escapeHtml(type)}</div>
      <div class="muted">fields: ${escapeHtml(props.join(", ") || "-")}</div>
    </article>
  `;
}

export function renderOpenApiHtml(doc) {
  const title = doc.info?.title || "OpenAPI";
  const version = doc.info?.version || "";

  const pathBlocks = [];
  for (const [route, ops] of Object.entries(doc.paths || {})) {
    for (const [method, op] of Object.entries(ops || {})) {
      pathBlocks.push(renderEndpoint(method, route, op || {}));
    }
  }

  const schemaBlocks = [];
  for (const [name, schema] of Object.entries(doc.components?.schemas || {})) {
    schemaBlocks.push(renderSchema(name, schema || {}));
  }

  return `
    <section>
      <h1>${escapeHtml(title)}</h1>
      ${version ? `<p class="muted">version ${escapeHtml(version)}</p>` : ""}
      <section class="openapi-runner">
        <div class="openapi-runner__endpoints">
          <h2>Endpoints</h2>
          ${pathBlocks.join("\n") || '<p class="muted">No endpoints.</p>'}
        </div>

        <div class="openapi-runner__output">
          <h2>Output</h2>
          <p class="muted">Click an endpoint to run it from this page.</p>
          <article class="openapi-output" id="openapi-output" hidden>
            <div class="openapi-output-head">
              <span class="path" id="openapi-output-target"></span>
              <span class="muted" id="openapi-output-status"></span>
            </div>
            <pre><code id="openapi-output-body"></code></pre>
          </article>
        </div>
      </section>
      <h2>Schemas</h2>
      ${schemaBlocks.join("\n") || '<p class="muted">No schemas.</p>'}
    </section>
  `;
}

async function fetchEndpoint(method, path) {
  const response = await fetch(path, {
    method,
    headers: {
      Accept: "application/json",
    },
  });
  const contentType = response.headers.get("content-type") || "";

  let bodyText = "";
  let bodyLang = "plaintext";
  if (contentType.includes("application/json")) {
    const json = await response.json().catch(() => null);
    bodyText = json === null ? "" : JSON.stringify(json, null, 2);
    bodyLang = "json";
  } else {
    bodyText = await response.text();
  }

  if (!bodyText) {
    bodyText = "(empty body)";
  }

  return {
    ok: response.ok,
    status: response.status,
    statusText: response.statusText,
    bodyText,
    bodyLang,
  };
}

const runnerState = {
  activeKey: "",
  requestId: 0,
  cache: new Map(),
};

function setActiveTrigger(root, activeKey) {
  for (const trigger of root.querySelectorAll(".endpoint-trigger")) {
    const isActive = trigger.dataset.endpointKey === activeKey;
    trigger.classList.toggle("is-active", isActive);
    trigger.setAttribute("aria-pressed", String(isActive));
  }
}

function highlightOutput(body, lang) {
  // highlight.js marks nodes with data-highlighted and warns on re-highlight.
  delete body.dataset.highlighted;
  body.classList.remove("hljs");
  body.className = "";
  body.classList.add(`language-${lang || "plaintext"}`);

  const hljs = window.hljs;
  if (hljs && typeof hljs.highlightElement === "function") {
    hljs.highlightElement(body);
  }
}

function renderOutput(root, key, status, bodyText, bodyLang = "plaintext") {
  const output = root.querySelector("#openapi-output");
  const target = root.querySelector("#openapi-output-target");
  const statusEl = root.querySelector("#openapi-output-status");
  const body = root.querySelector("#openapi-output-body");
  if (!output || !target || !statusEl || !body) {
    return;
  }

  output.hidden = false;
  target.textContent = key;
  statusEl.textContent = status;
  body.textContent = bodyText;
  highlightOutput(body, bodyLang);
}

export function wireOpenApiInteractions(root) {
  const triggers = Array.from(root.querySelectorAll(".endpoint-trigger"));
  if (triggers.length === 0) {
    return;
  }

  for (const trigger of triggers) {
    trigger.addEventListener("click", async () => {
      const key = trigger.dataset.endpointKey || "";
      const method = trigger.dataset.method || "GET";
      const path = trigger.dataset.path || "";
      const output = root.querySelector("#openapi-output");

      if (!key || !path) {
        return;
      }

      if (runnerState.activeKey === key && output && !output.hidden) {
        return;
      }

      runnerState.activeKey = key;
      setActiveTrigger(root, key);

      const cached = runnerState.cache.get(key);
      if (cached) {
        renderOutput(root, key, cached.status, cached.bodyText, cached.bodyLang);
        return;
      }

      const requestId = ++runnerState.requestId;
      renderOutput(root, key, "loading...", "Fetching...", "plaintext");

      try {
        const result = await fetchEndpoint(method, path);
        const status = `${result.status} ${result.statusText}`.trim();
        runnerState.cache.set(key, {
          status,
          bodyText: result.bodyText,
          bodyLang: result.bodyLang,
        });

        if (requestId !== runnerState.requestId || runnerState.activeKey !== key) {
          return;
        }

        renderOutput(root, key, status, result.bodyText, result.bodyLang);
      } catch (error) {
        if (requestId !== runnerState.requestId || runnerState.activeKey !== key) {
          return;
        }
        renderOutput(
          root,
          key,
          "request failed",
          String(error.message || error),
          "plaintext",
        );
      }
    });
  }
}
