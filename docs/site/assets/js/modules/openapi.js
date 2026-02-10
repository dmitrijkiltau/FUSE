function escapeHtml(text) {
  return String(text)
    .replaceAll("&", "&amp;")
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
  const summary = op.summary || op.operationId || "";
  const responses = op.responses ? Object.keys(op.responses).join(", ") : "";
  return `
    <article class="endpoint">
      <div><span class="method">${escapeHtml(method)}</span><span class="path">${escapeHtml(route)}</span></div>
      ${summary ? `<div class="summary">${escapeHtml(summary)}</div>` : ""}
      ${responses ? `<div class="muted">responses: ${escapeHtml(responses)}</div>` : ""}
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
      <h2>Endpoints</h2>
      ${pathBlocks.join("\n") || '<p class="muted">No endpoints.</p>'}
      <h2>Schemas</h2>
      ${schemaBlocks.join("\n") || '<p class="muted">No schemas.</p>'}
    </section>
  `;
}
