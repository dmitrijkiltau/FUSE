use std::collections::{BTreeMap, HashMap};

use crate::ast::{
    BinaryOp, EnumDecl, Expr, ExprKind, FieldDecl, HttpVerb, Item, Literal, RouteDecl, ServiceDecl,
    TypeDecl, TypeRef, TypeRefKind,
};
use crate::loader::{ModuleId, ModuleRegistry, ModuleUnit};
use fuse_rt::json::JsonValue;

pub fn generate_openapi(registry: &ModuleRegistry) -> Result<String, String> {
    let root = registry
        .root()
        .ok_or_else(|| "no root module loaded".to_string())?;
    let title = root
        .path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("FUSE API")
        .to_string();
    let mut builder = OpenApiBuilder::new(registry, title);
    let doc = builder.build();
    Ok(fuse_rt::json::encode(&doc))
}

struct OpenApiBuilder<'a> {
    registry: &'a ModuleRegistry,
    title: String,
    schema_names: HashMap<(ModuleId, String), String>,
    module_labels: HashMap<ModuleId, String>,
}

impl<'a> OpenApiBuilder<'a> {
    fn new(registry: &'a ModuleRegistry, title: String) -> Self {
        Self {
            registry,
            title,
            schema_names: HashMap::new(),
            module_labels: HashMap::new(),
        }
    }

    fn build(&mut self) -> JsonValue {
        self.collect_module_labels();
        self.collect_schema_names();

        let mut schemas = BTreeMap::new();
        for id in self.sorted_module_ids() {
            if let Some(unit) = self.registry.modules.get(&id) {
                self.collect_schemas_for_unit(unit, &mut schemas);
            }
        }
        self.insert_error_schemas(&mut schemas);

        let (paths, tags) = self.collect_paths_and_tags();

        let mut root = BTreeMap::new();
        root.insert(
            "openapi".to_string(),
            JsonValue::String("3.0.0".to_string()),
        );
        let mut info = BTreeMap::new();
        info.insert("title".to_string(), JsonValue::String(self.title.clone()));
        info.insert(
            "version".to_string(),
            JsonValue::String("0.1.0".to_string()),
        );
        root.insert("info".to_string(), JsonValue::Object(info));
        root.insert("paths".to_string(), JsonValue::Object(paths));
        let mut components = BTreeMap::new();
        components.insert("schemas".to_string(), JsonValue::Object(schemas));
        root.insert("components".to_string(), JsonValue::Object(components));
        if !tags.is_empty() {
            root.insert("tags".to_string(), JsonValue::Array(tags));
        }
        JsonValue::Object(root)
    }

    fn sorted_module_ids(&self) -> Vec<ModuleId> {
        let mut ids: Vec<ModuleId> = self.registry.modules.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    fn collect_module_labels(&mut self) {
        for (id, unit) in &self.registry.modules {
            let label = unit
                .path
                .file_stem()
                .and_then(|name| name.to_str())
                .map(|name| name.to_string())
                .unwrap_or_else(|| format!("module{id}"));
            self.module_labels.insert(*id, label);
        }
    }

    fn collect_schema_names(&mut self) {
        for (id, unit) in &self.registry.modules {
            for item in &unit.program.items {
                match item {
                    Item::Type(decl) => {
                        let key = (unit.id, decl.name.name.clone());
                        self.schema_names
                            .insert(key, format!("m{}_{}", id, decl.name.name));
                    }
                    Item::Enum(decl) => {
                        let key = (unit.id, decl.name.name.clone());
                        self.schema_names
                            .insert(key, format!("m{}_{}", id, decl.name.name));
                    }
                    _ => {}
                }
            }
        }
    }

    fn collect_schemas_for_unit(
        &self,
        unit: &ModuleUnit,
        schemas: &mut BTreeMap<String, JsonValue>,
    ) {
        for item in &unit.program.items {
            match item {
                Item::Type(decl) => {
                    if let Some(key) = self.schema_key(unit.id, &decl.name.name) {
                        let schema = self.schema_for_type_decl(unit, decl);
                        schemas.insert(key, schema);
                    }
                }
                Item::Enum(decl) => {
                    if let Some(key) = self.schema_key(unit.id, &decl.name.name) {
                        let schema = self.schema_for_enum_decl(unit, decl);
                        schemas.insert(key, schema);
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_paths_and_tags(&self) -> (BTreeMap<String, JsonValue>, Vec<JsonValue>) {
        let mut paths: BTreeMap<String, JsonValue> = BTreeMap::new();
        let mut tags: BTreeMap<String, String> = BTreeMap::new();

        for id in self.sorted_module_ids() {
            let Some(unit) = self.registry.modules.get(&id) else {
                continue;
            };
            for item in &unit.program.items {
                let Item::Service(service) = item else {
                    continue;
                };
                let tag_name = service.name.name.clone();
                if let Some(doc) = &service.doc {
                    tags.insert(tag_name.clone(), doc.clone());
                } else {
                    tags.entry(tag_name.clone()).or_insert_with(String::new);
                }

                for (idx, route) in service.routes.iter().enumerate() {
                    let full_path = join_paths(&service.base_path.value, &route.path.value);
                    let (path_key, params) = normalize_route_path(&full_path);

                    let entry = paths
                        .entry(path_key)
                        .or_insert_with(|| JsonValue::Object(BTreeMap::new()));
                    let JsonValue::Object(path_item) = entry else {
                        continue;
                    };

                    let method = verb_name(&route.verb);
                    let op = self.build_operation(unit, service, route, idx, &params);
                    path_item.insert(method.to_string(), op);
                }
            }
        }

        let mut tag_items = Vec::new();
        for (name, description) in tags {
            let mut tag = BTreeMap::new();
            tag.insert("name".to_string(), JsonValue::String(name));
            if !description.is_empty() {
                tag.insert("description".to_string(), JsonValue::String(description));
            }
            tag_items.push(JsonValue::Object(tag));
        }

        (paths, tag_items)
    }

    fn build_operation(
        &self,
        unit: &ModuleUnit,
        service: &ServiceDecl,
        route: &RouteDecl,
        idx: usize,
        params: &[(String, String)],
    ) -> JsonValue {
        let mut op = BTreeMap::new();
        op.insert(
            "tags".to_string(),
            JsonValue::Array(vec![JsonValue::String(service.name.name.clone())]),
        );
        op.insert(
            "operationId".to_string(),
            JsonValue::String(format!("{}_{}", service.name.name, idx)),
        );

        if !params.is_empty() {
            let mut items = Vec::new();
            for (name, ty_name) in params {
                let mut param = BTreeMap::new();
                param.insert("name".to_string(), JsonValue::String(name.clone()));
                param.insert("in".to_string(), JsonValue::String("path".to_string()));
                param.insert("required".to_string(), JsonValue::Bool(true));
                let schema = self.schema_for_path_param(unit, ty_name);
                param.insert("schema".to_string(), schema);
                items.push(JsonValue::Object(param));
            }
            op.insert("parameters".to_string(), JsonValue::Array(items));
        }

        if let Some(body_ty) = &route.body_type {
            let mut body = BTreeMap::new();
            let schema = self.schema_for_type_ref(unit, body_ty);
            let mut content = BTreeMap::new();
            let mut json = BTreeMap::new();
            json.insert("schema".to_string(), schema);
            content.insert("application/json".to_string(), JsonValue::Object(json));
            body.insert("content".to_string(), JsonValue::Object(content));
            body.insert(
                "required".to_string(),
                JsonValue::Bool(!is_optional_type(body_ty)),
            );
            op.insert("requestBody".to_string(), JsonValue::Object(body));
        }

        let mut responses = BTreeMap::new();
        let ok_schema = self.schema_for_response(unit, &route.ret_type);
        let mut ok = BTreeMap::new();
        ok.insert(
            "description".to_string(),
            JsonValue::String("OK".to_string()),
        );
        let mut ok_content = BTreeMap::new();
        let mut ok_json = BTreeMap::new();
        ok_json.insert("schema".to_string(), ok_schema);
        ok_content.insert("application/json".to_string(), JsonValue::Object(ok_json));
        ok.insert("content".to_string(), JsonValue::Object(ok_content));
        responses.insert("200".to_string(), JsonValue::Object(ok));

        let mut err = BTreeMap::new();
        err.insert(
            "description".to_string(),
            JsonValue::String("Error".to_string()),
        );
        let mut err_content = BTreeMap::new();
        let mut err_json = BTreeMap::new();
        err_json.insert(
            "schema".to_string(),
            JsonValue::Object(BTreeMap::from([(
                "$ref".to_string(),
                JsonValue::String("#/components/schemas/Error".to_string()),
            )])),
        );
        err_content.insert("application/json".to_string(), JsonValue::Object(err_json));
        err.insert("content".to_string(), JsonValue::Object(err_content));
        responses.insert("default".to_string(), JsonValue::Object(err));

        op.insert("responses".to_string(), JsonValue::Object(responses));
        JsonValue::Object(op)
    }

    fn schema_for_path_param(&self, unit: &ModuleUnit, name: &str) -> JsonValue {
        if let Some(schema) = primitive_schema(name) {
            return schema;
        }
        if let Some((module_id, item)) = self.resolve_named_type(unit, name) {
            if let Some(key) = self.schema_key(module_id, &item) {
                return JsonValue::Object(BTreeMap::from([(
                    "$ref".to_string(),
                    JsonValue::String(format!("#/components/schemas/{key}")),
                )]));
            }
        }
        JsonValue::Object(BTreeMap::from([(
            "type".to_string(),
            JsonValue::String("string".to_string()),
        )]))
    }

    fn schema_for_response(&self, unit: &ModuleUnit, ty: &TypeRef) -> JsonValue {
        match &ty.kind {
            TypeRefKind::Result { ok, .. } => self.schema_for_type_ref(unit, ok),
            _ => self.schema_for_type_ref(unit, ty),
        }
    }

    fn schema_for_type_ref(&self, unit: &ModuleUnit, ty: &TypeRef) -> JsonValue {
        match &ty.kind {
            TypeRefKind::Optional(inner) => make_nullable(self.schema_for_type_ref(unit, inner)),
            TypeRefKind::Result { ok, .. } => self.schema_for_type_ref(unit, ok),
            TypeRefKind::Generic { base, args } => match base.name.as_str() {
                "List" if args.len() == 1 => {
                    let mut out = BTreeMap::new();
                    out.insert("type".to_string(), JsonValue::String("array".to_string()));
                    out.insert(
                        "items".to_string(),
                        self.schema_for_type_ref(unit, &args[0]),
                    );
                    JsonValue::Object(out)
                }
                "Map" if args.len() == 2 => {
                    let mut out = BTreeMap::new();
                    out.insert("type".to_string(), JsonValue::String("object".to_string()));
                    out.insert(
                        "additionalProperties".to_string(),
                        self.schema_for_type_ref(unit, &args[1]),
                    );
                    JsonValue::Object(out)
                }
                "Option" if args.len() == 1 => {
                    make_nullable(self.schema_for_type_ref(unit, &args[0]))
                }
                "Result" if !args.is_empty() => self.schema_for_type_ref(unit, &args[0]),
                _ => self.schema_for_named_type(unit, &base.name),
            },
            TypeRefKind::Refined { base, args } => {
                let mut schema = self.schema_for_named_type(unit, &base.name);
                let constraints = refined_constraints(base.name.as_str(), args);
                schema = apply_constraints(schema, constraints);
                schema
            }
            TypeRefKind::Simple(ident) => self.schema_for_named_type(unit, &ident.name),
        }
    }

    fn schema_for_named_type(&self, unit: &ModuleUnit, name: &str) -> JsonValue {
        if let Some(schema) = primitive_schema(name) {
            return schema;
        }
        if let Some((module_id, item)) = self.resolve_named_type(unit, name) {
            if let Some(key) = self.schema_key(module_id, &item) {
                return JsonValue::Object(BTreeMap::from([(
                    "$ref".to_string(),
                    JsonValue::String(format!("#/components/schemas/{key}")),
                )]));
            }
        }
        JsonValue::Object(BTreeMap::from([
            ("type".to_string(), JsonValue::String("string".to_string())),
            (
                "description".to_string(),
                JsonValue::String(format!("unknown type {name}")),
            ),
        ]))
    }

    fn schema_for_type_decl(&self, unit: &ModuleUnit, decl: &TypeDecl) -> JsonValue {
        let mut schema = BTreeMap::new();
        schema.insert("type".to_string(), JsonValue::String("object".to_string()));
        if let Some(doc) = &decl.doc {
            schema.insert("description".to_string(), JsonValue::String(doc.clone()));
        }
        schema.insert(
            "title".to_string(),
            JsonValue::String(self.schema_title(unit.id, &decl.name.name)),
        );
        let mut properties = BTreeMap::new();
        let mut required = Vec::new();
        for field in &decl.fields {
            let field_schema = self.schema_for_type_ref(unit, &field.ty);
            properties.insert(field.name.name.clone(), field_schema);
            if is_required_field(field) {
                required.push(JsonValue::String(field.name.name.clone()));
            }
        }
        schema.insert("properties".to_string(), JsonValue::Object(properties));
        if !required.is_empty() {
            schema.insert("required".to_string(), JsonValue::Array(required));
        }
        JsonValue::Object(schema)
    }

    fn schema_for_enum_decl(&self, unit: &ModuleUnit, decl: &EnumDecl) -> JsonValue {
        let mut schema = BTreeMap::new();
        if let Some(doc) = &decl.doc {
            schema.insert("description".to_string(), JsonValue::String(doc.clone()));
        }
        schema.insert(
            "title".to_string(),
            JsonValue::String(self.schema_title(unit.id, &decl.name.name)),
        );
        let mut variants = Vec::new();
        for variant in &decl.variants {
            variants.push(self.schema_for_enum_variant(
                unit,
                variant.name.name.as_str(),
                &variant.payload,
            ));
        }
        schema.insert("oneOf".to_string(), JsonValue::Array(variants));
        JsonValue::Object(schema)
    }

    fn schema_for_enum_variant(
        &self,
        unit: &ModuleUnit,
        name: &str,
        payload: &[TypeRef],
    ) -> JsonValue {
        let mut variant = BTreeMap::new();
        variant.insert("type".to_string(), JsonValue::String("object".to_string()));
        let mut properties = BTreeMap::new();
        let mut type_prop = BTreeMap::new();
        type_prop.insert("type".to_string(), JsonValue::String("string".to_string()));
        type_prop.insert(
            "enum".to_string(),
            JsonValue::Array(vec![JsonValue::String(name.to_string())]),
        );
        properties.insert("type".to_string(), JsonValue::Object(type_prop));
        let mut required = vec![JsonValue::String("type".to_string())];

        if !payload.is_empty() {
            let data_schema = if payload.len() == 1 {
                self.schema_for_type_ref(unit, &payload[0])
            } else {
                let mut data = BTreeMap::new();
                data.insert("type".to_string(), JsonValue::String("array".to_string()));
                let mut item = BTreeMap::new();
                let mut choices = Vec::new();
                for ty in payload {
                    choices.push(self.schema_for_type_ref(unit, ty));
                }
                item.insert("oneOf".to_string(), JsonValue::Array(choices));
                data.insert("items".to_string(), JsonValue::Object(item));
                data.insert(
                    "minItems".to_string(),
                    JsonValue::Number(payload.len() as f64),
                );
                data.insert(
                    "maxItems".to_string(),
                    JsonValue::Number(payload.len() as f64),
                );
                JsonValue::Object(data)
            };
            properties.insert("data".to_string(), data_schema);
            required.push(JsonValue::String("data".to_string()));
        }

        variant.insert("properties".to_string(), JsonValue::Object(properties));
        variant.insert("required".to_string(), JsonValue::Array(required));
        JsonValue::Object(variant)
    }

    fn insert_error_schemas(&self, schemas: &mut BTreeMap<String, JsonValue>) {
        if schemas.contains_key("ValidationField") || schemas.contains_key("Error") {
            return;
        }
        let mut field = BTreeMap::new();
        field.insert("type".to_string(), JsonValue::String("object".to_string()));
        let mut field_props = BTreeMap::new();
        field_props.insert("path".to_string(), JsonValue::Object(string_schema()));
        field_props.insert("code".to_string(), JsonValue::Object(string_schema()));
        field_props.insert("message".to_string(), JsonValue::Object(string_schema()));
        field.insert("properties".to_string(), JsonValue::Object(field_props));
        field.insert(
            "required".to_string(),
            JsonValue::Array(vec![
                JsonValue::String("path".to_string()),
                JsonValue::String("code".to_string()),
                JsonValue::String("message".to_string()),
            ]),
        );
        schemas.insert("ValidationField".to_string(), JsonValue::Object(field));

        let mut error = BTreeMap::new();
        error.insert("type".to_string(), JsonValue::String("object".to_string()));
        let mut error_props = BTreeMap::new();
        let mut err_obj = BTreeMap::new();
        err_obj.insert("type".to_string(), JsonValue::String("object".to_string()));
        let mut err_props = BTreeMap::new();
        err_props.insert("code".to_string(), JsonValue::Object(string_schema()));
        err_props.insert("message".to_string(), JsonValue::Object(string_schema()));
        let mut fields = BTreeMap::new();
        fields.insert("type".to_string(), JsonValue::String("array".to_string()));
        fields.insert(
            "items".to_string(),
            JsonValue::Object(BTreeMap::from([(
                "$ref".to_string(),
                JsonValue::String("#/components/schemas/ValidationField".to_string()),
            )])),
        );
        err_props.insert("fields".to_string(), JsonValue::Object(fields));
        err_obj.insert("properties".to_string(), JsonValue::Object(err_props));
        err_obj.insert(
            "required".to_string(),
            JsonValue::Array(vec![
                JsonValue::String("code".to_string()),
                JsonValue::String("message".to_string()),
            ]),
        );
        error_props.insert("error".to_string(), JsonValue::Object(err_obj));
        error.insert("properties".to_string(), JsonValue::Object(error_props));
        error.insert(
            "required".to_string(),
            JsonValue::Array(vec![JsonValue::String("error".to_string())]),
        );
        schemas.insert("Error".to_string(), JsonValue::Object(error));
    }

    fn resolve_named_type(&self, unit: &ModuleUnit, name: &str) -> Option<(ModuleId, String)> {
        if is_builtin_type_name(name) {
            return None;
        }
        if let Some((module, item)) = split_qualified(name) {
            if let Some(link) = unit.modules.get(module) {
                return Some((link.id, item.to_string()));
            }
            return None;
        }
        if let Some(link) = unit.import_items.get(name) {
            return Some((link.id, name.to_string()));
        }
        Some((unit.id, name.to_string()))
    }

    fn schema_key(&self, module_id: ModuleId, name: &str) -> Option<String> {
        self.schema_names
            .get(&(module_id, name.to_string()))
            .cloned()
    }

    fn schema_title(&self, module_id: ModuleId, name: &str) -> String {
        let label = self
            .module_labels
            .get(&module_id)
            .cloned()
            .unwrap_or_else(|| format!("module{module_id}"));
        format!("{label}.{name}")
    }
}

fn join_paths(base: &str, route: &str) -> String {
    let base_trim = base.trim_end_matches('/');
    let route_trim = route.trim_start_matches('/');
    let mut out = String::new();
    if base_trim.is_empty() {
        out.push('/');
        out.push_str(route_trim);
    } else {
        out.push_str(base_trim);
        if !route_trim.is_empty() {
            out.push('/');
            out.push_str(route_trim);
        }
    }
    if !out.starts_with('/') {
        out.insert(0, '/');
    }
    out
}

fn normalize_route_path(path: &str) -> (String, Vec<(String, String)>) {
    let mut out = String::new();
    let mut params = Vec::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '{' {
            out.push(ch);
            continue;
        }
        let mut inner = String::new();
        let mut closed = false;
        while let Some(next) = chars.next() {
            if next == '}' {
                closed = true;
                break;
            }
            inner.push(next);
        }
        if closed {
            let mut parts = inner.splitn(2, ':');
            let name = parts.next().unwrap_or("").trim();
            let ty = parts.next().unwrap_or("").trim();
            if !name.is_empty() {
                params.push((name.to_string(), ty.to_string()));
                out.push('{');
                out.push_str(name);
                out.push('}');
            } else {
                out.push('{');
                out.push_str(inner.trim());
                out.push('}');
            }
        } else {
            out.push('{');
            out.push_str(&inner);
        }
    }
    (out, params)
}

fn verb_name(verb: &HttpVerb) -> &'static str {
    match verb {
        HttpVerb::Get => "get",
        HttpVerb::Post => "post",
        HttpVerb::Put => "put",
        HttpVerb::Patch => "patch",
        HttpVerb::Delete => "delete",
    }
}

fn split_qualified(name: &str) -> Option<(&str, &str)> {
    let mut parts = name.split('.');
    let module = parts.next()?;
    let item = parts.next()?;
    if module.is_empty() || item.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((module, item))
}

fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "Int" | "Float" | "Bool" | "String" | "Id" | "Email" | "Bytes"
    )
}

fn primitive_schema(name: &str) -> Option<JsonValue> {
    let mut schema = BTreeMap::new();
    match name {
        "Int" => {
            schema.insert("type".to_string(), JsonValue::String("integer".to_string()));
            schema.insert("format".to_string(), JsonValue::String("int64".to_string()));
        }
        "Float" => {
            schema.insert("type".to_string(), JsonValue::String("number".to_string()));
            schema.insert(
                "format".to_string(),
                JsonValue::String("double".to_string()),
            );
        }
        "Bool" => {
            schema.insert("type".to_string(), JsonValue::String("boolean".to_string()));
        }
        "String" | "Id" => {
            schema.insert("type".to_string(), JsonValue::String("string".to_string()));
        }
        "Email" => {
            schema.insert("type".to_string(), JsonValue::String("string".to_string()));
            schema.insert("format".to_string(), JsonValue::String("email".to_string()));
        }
        "Bytes" => {
            schema.insert("type".to_string(), JsonValue::String("string".to_string()));
            schema.insert("format".to_string(), JsonValue::String("byte".to_string()));
        }
        _ => return None,
    }
    Some(JsonValue::Object(schema))
}

fn string_schema() -> BTreeMap<String, JsonValue> {
    BTreeMap::from([("type".to_string(), JsonValue::String("string".to_string()))])
}

fn is_optional_type(ty: &TypeRef) -> bool {
    match &ty.kind {
        TypeRefKind::Optional(_) => true,
        TypeRefKind::Generic { base, args } => base.name == "Option" && args.len() == 1,
        _ => false,
    }
}

fn is_required_field(field: &FieldDecl) -> bool {
    field.default.is_none() && !is_optional_type(&field.ty)
}

fn make_nullable(schema: JsonValue) -> JsonValue {
    match schema {
        JsonValue::Object(mut map) => {
            if map.contains_key("$ref") {
                let mut out = BTreeMap::new();
                out.insert(
                    "allOf".to_string(),
                    JsonValue::Array(vec![JsonValue::Object(map)]),
                );
                out.insert("nullable".to_string(), JsonValue::Bool(true));
                JsonValue::Object(out)
            } else {
                map.insert("nullable".to_string(), JsonValue::Bool(true));
                JsonValue::Object(map)
            }
        }
        other => other,
    }
}

fn apply_constraints(schema: JsonValue, constraints: BTreeMap<String, JsonValue>) -> JsonValue {
    if constraints.is_empty() {
        return schema;
    }
    match schema {
        JsonValue::Object(mut map) => {
            if map.contains_key("$ref") {
                let mut out = BTreeMap::new();
                out.insert(
                    "allOf".to_string(),
                    JsonValue::Array(vec![JsonValue::Object(map)]),
                );
                for (key, value) in constraints {
                    out.insert(key, value);
                }
                JsonValue::Object(out)
            } else {
                for (key, value) in constraints {
                    map.insert(key, value);
                }
                JsonValue::Object(map)
            }
        }
        other => other,
    }
}

fn refined_constraints(base: &str, args: &[Expr]) -> BTreeMap<String, JsonValue> {
    let mut out = BTreeMap::new();
    let Some((min, max)) = extract_range(args) else {
        return out;
    };
    match base {
        "String" | "Id" | "Email" | "Bytes" => {
            out.insert("minLength".to_string(), JsonValue::Number(min));
            out.insert("maxLength".to_string(), JsonValue::Number(max));
        }
        "Int" | "Float" => {
            out.insert("minimum".to_string(), JsonValue::Number(min));
            out.insert("maximum".to_string(), JsonValue::Number(max));
        }
        _ => {}
    }
    out
}

fn extract_range(args: &[Expr]) -> Option<(f64, f64)> {
    let first = args.first()?;
    if let ExprKind::Binary { op, left, right } = &first.kind {
        if *op != BinaryOp::Range {
            return None;
        }
        let min = literal_number(left)?;
        let max = literal_number(right)?;
        return Some((min, max));
    }
    None
}

fn literal_number(expr: &Expr) -> Option<f64> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(v)) => Some(*v as f64),
        ExprKind::Literal(Literal::Float(v)) => Some(*v),
        _ => None,
    }
}
