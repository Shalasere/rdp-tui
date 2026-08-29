//! Validation for the machine-readable architecture contract.

use regex::Regex;
use serde_yaml_ng::{Mapping, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const REQUIRED_DOCUMENTS: [&str; 5] = [
    "00-manifest.yaml",
    "01-types.yaml",
    "02-rules.yaml",
    "03-process.yaml",
    "04-amendments.yaml",
];
const SOURCE_ARCHIVE: &str = "rdp-tui-architecture-contract-source.zip";
const SOURCE_ARCHIVE_SHA256: &str =
    "d716591ace6b17956a5a2c24f65e4b1e73cda42a7d7153d27954a366fff2660b";

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ContractReport {
    pub yaml_documents: usize,
    pub modules: usize,
    pub stable_ids: usize,
    pub binding_types: usize,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ContractError {
    pub location: String,
    pub message: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ContractErrors(pub Vec<ContractError>);

impl fmt::Display for ContractErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            if index > 0 {
                writeln!(formatter)?;
            }
            write!(formatter, "{}: {}", error.location, error.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ContractErrors {}

/// Validate the versioned architecture documents and preserved source archive.
///
/// # Errors
///
/// Returns every validation failure that can be determined safely after the
/// documents have parsed. YAML read or parse failures stop cross-document
/// validation because subsequent checks would be unreliable.
pub fn validate_architecture_contract(
    contract_dir: impl AsRef<Path>,
) -> Result<ContractReport, ContractErrors> {
    let contract_dir = contract_dir.as_ref();
    let mut errors = Vec::new();
    let documents = load_documents(contract_dir, &mut errors);
    if !errors.is_empty() {
        return Err(ContractErrors(errors));
    }

    let manifest = &documents["00-manifest.yaml"];
    let types = &documents["01-types.yaml"];
    let amendments = &documents["04-amendments.yaml"];

    let graph = match merged_module_graph(manifest, amendments) {
        Ok(graph) => {
            match mapping_value(manifest, "forbidden_edges").and_then(parse_forbidden_edges) {
                Ok(forbidden_edges) => validate_module_graph(&graph, &forbidden_edges, &mut errors),
                Err(message) => push_error(&mut errors, "forbidden_edges", message),
            }
            graph
        }
        Err(message) => {
            push_error(&mut errors, "module_dependency_graph", message);
            BTreeMap::new()
        }
    };

    let stable_ids = validate_stable_ids(&documents, &mut errors);
    let binding_types = validate_binding_types(types, amendments, &mut errors);
    validate_source_archive(contract_dir, &mut errors);

    if errors.is_empty() {
        Ok(ContractReport {
            yaml_documents: documents.len(),
            modules: graph.len(),
            stable_ids,
            binding_types,
        })
    } else {
        Err(ContractErrors(errors))
    }
}

fn parse_forbidden_edges(value: &Value) -> Result<BTreeSet<(String, String)>, String> {
    let edges = value
        .as_sequence()
        .ok_or_else(|| "forbidden_edges must be a sequence".to_owned())?;
    let mut parsed = BTreeSet::new();
    for edge in edges {
        let from = mapping_value(edge, "from")?
            .as_str()
            .ok_or_else(|| "forbidden edge 'from' must be a string".to_owned())?;
        let to = mapping_value(edge, "to")?
            .as_str()
            .ok_or_else(|| "forbidden edge 'to' must be a string".to_owned())?;
        parsed.insert((from.to_owned(), to.to_owned()));
    }
    Ok(parsed)
}

fn load_documents(contract_dir: &Path, errors: &mut Vec<ContractError>) -> BTreeMap<String, Value> {
    let mut documents = BTreeMap::new();
    let entries = match fs::read_dir(contract_dir) {
        Ok(entries) => entries,
        Err(error) => {
            push_error(
                errors,
                contract_dir.display().to_string(),
                format!("cannot read contract directory: {error}"),
            );
            return documents;
        }
    };
    let mut yaml_names = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
        })
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect::<Vec<_>>();
    yaml_names.sort();
    for required in REQUIRED_DOCUMENTS {
        if !yaml_names.iter().any(|name| name == required) {
            push_error(errors, required, "required contract document is missing");
        }
    }
    for name in yaml_names {
        let path = contract_dir.join(&name);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                push_error(errors, &name, format!("cannot read document: {error}"));
                continue;
            }
        };
        let document: Value = match serde_yaml_ng::from_str(&text) {
            Ok(document) => document,
            Err(error) => {
                push_error(errors, &name, format!("invalid YAML: {error}"));
                continue;
            }
        };
        match document
            .as_mapping()
            .and_then(|mapping| mapping.get(Value::String("schema_version".into())))
            .and_then(Value::as_u64)
        {
            Some(1) => {}
            Some(version) => push_error(
                errors,
                &name,
                format!("unsupported schema_version {version}; expected 1"),
            ),
            None => push_error(errors, &name, "missing integer schema_version"),
        }
        documents.insert(name, document);
    }
    documents
}

fn merged_module_graph(
    manifest: &Value,
    amendments: &Value,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut graph = parse_graph(mapping_value(manifest, "module_dependency_graph")?)?;
    let graph_amendments = mapping_value(amendments, "module_graph_amendments")?;
    let additions = mapping_value(graph_amendments, "add_module")?;
    for (module, dependencies) in parse_graph(additions)? {
        if graph.insert(module.clone(), dependencies).is_some() {
            return Err(format!("add_module redeclares existing module {module:?}"));
        }
    }
    let replacements = mapping_value(graph_amendments, "replace_dependencies")?;
    for (module, dependencies) in parse_graph(replacements)? {
        if !graph.contains_key(&module) {
            return Err(format!(
                "replace_dependencies references undeclared module {module:?}"
            ));
        }
        graph.insert(module, dependencies);
    }
    Ok(graph)
}

fn parse_graph(value: &Value) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| "dependency graph must be a mapping".to_owned())?;
    let mut graph = BTreeMap::new();
    for (module, dependencies) in mapping {
        let module = module
            .as_str()
            .ok_or_else(|| "module name must be a string".to_owned())?;
        let dependencies = dependencies
            .as_sequence()
            .ok_or_else(|| format!("dependencies for {module:?} must be a sequence"))?;
        let mut parsed = Vec::new();
        for dependency in dependencies {
            parsed.push(
                dependency
                    .as_str()
                    .ok_or_else(|| format!("dependency of {module:?} must be a string"))?
                    .to_owned(),
            );
        }
        graph.insert(module.to_owned(), parsed);
    }
    Ok(graph)
}

fn validate_module_graph(
    graph: &BTreeMap<String, Vec<String>>,
    forbidden_edges: &BTreeSet<(String, String)>,
    errors: &mut Vec<ContractError>,
) {
    for (module, dependencies) in graph {
        for dependency in dependencies {
            if !graph.contains_key(dependency) {
                push_error(
                    errors,
                    "module_dependency_graph",
                    format!("{module:?} depends on undeclared module {dependency:?}"),
                );
            }
            if forbidden_edges.contains(&(module.clone(), dependency.clone())) {
                push_error(
                    errors,
                    "module_dependency_graph",
                    format!("forbidden dependency edge: {module} -> {dependency}"),
                );
            }
        }
    }

    let mut states = BTreeMap::new();
    for module in graph.keys() {
        let mut trail = Vec::new();
        if let Some(cycle) = find_cycle(module, graph, &mut states, &mut trail) {
            push_error(
                errors,
                "module_dependency_graph",
                format!("dependency cycle: {}", cycle.join(" -> ")),
            );
            break;
        }
    }
}

fn find_cycle(
    module: &str,
    graph: &BTreeMap<String, Vec<String>>,
    states: &mut BTreeMap<String, u8>,
    trail: &mut Vec<String>,
) -> Option<Vec<String>> {
    match states.get(module) {
        Some(1) => {
            let start = trail.iter().position(|item| item == module).unwrap_or(0);
            let mut cycle = trail[start..].to_vec();
            cycle.push(module.to_owned());
            return Some(cycle);
        }
        Some(2) => return None,
        _ => {}
    }
    states.insert(module.to_owned(), 1);
    trail.push(module.to_owned());
    if let Some(dependencies) = graph.get(module) {
        for dependency in dependencies {
            if graph.contains_key(dependency)
                && let Some(cycle) = find_cycle(dependency, graph, states, trail)
            {
                return Some(cycle);
            }
        }
    }
    trail.pop();
    states.insert(module.to_owned(), 2);
    None
}

fn validate_stable_ids(
    documents: &BTreeMap<String, Value>,
    errors: &mut Vec<ContractError>,
) -> usize {
    let stable_id = Regex::new(r"\b(?:INV|DEC|AP|GAP)-[A-Za-z0-9_-]+\b").expect("valid regex");
    let mut definitions = BTreeMap::<String, String>::new();
    let mut references = BTreeSet::new();

    for (name, document) in documents {
        collect_stable_ids(
            document,
            name,
            &stable_id,
            &mut definitions,
            &mut references,
            errors,
        );
    }
    for reference in references {
        if !definitions.contains_key(&reference) {
            push_error(
                errors,
                "stable_ids",
                format!("unresolved stable identifier {reference}"),
            );
        }
    }
    definitions.len()
}

fn collect_stable_ids(
    value: &Value,
    location: &str,
    pattern: &Regex,
    definitions: &mut BTreeMap<String, String>,
    references: &mut BTreeSet<String>,
    errors: &mut Vec<ContractError>,
) {
    match value {
        Value::Mapping(mapping) => {
            if let Some(identifier) = mapping
                .get(Value::String("id".into()))
                .and_then(Value::as_str)
                .filter(|identifier| pattern.is_match(identifier))
                && let Some(previous) =
                    definitions.insert(identifier.to_owned(), location.to_owned())
            {
                push_error(
                    errors,
                    location,
                    format!(
                        "duplicate stable identifier {identifier}; first defined in {previous}"
                    ),
                );
            }
            for (key, child) in mapping {
                collect_stable_ids(key, location, pattern, definitions, references, errors);
                collect_stable_ids(child, location, pattern, definitions, references, errors);
            }
        }
        Value::Sequence(sequence) => {
            for child in sequence {
                collect_stable_ids(child, location, pattern, definitions, references, errors);
            }
        }
        Value::String(text) => {
            references.extend(pattern.find_iter(text).map(|item| item.as_str().to_owned()));
        }
        _ => {}
    }
}

fn validate_binding_types(
    types: &Value,
    amendments: &Value,
    errors: &mut Vec<ContractError>,
) -> usize {
    let mut definitions = BTreeMap::<String, &Value>::new();
    let Some(type_sections) = types.as_mapping() else {
        push_error(errors, "01-types.yaml", "document root must be a mapping");
        return 0;
    };
    for section in type_sections.values().filter_map(Value::as_mapping) {
        collect_named_type_definitions(section, &mut definitions);
    }

    if let Ok(additions) = mapping_value(amendments, "binding_type_additions")
        && let Some(additions) = additions.as_mapping()
    {
        collect_named_type_definitions(additions, &mut definitions);
        if let Some(ids) = additions
            .get(Value::String("ids".into()))
            .and_then(Value::as_mapping)
        {
            collect_named_type_definitions(ids, &mut definitions);
        }
    }
    if let Ok(pipeline) = mapping_value(amendments, "credential_pipeline")
        && let Some(binding_types) = pipeline
            .get(Value::String("binding_types".into()))
            .and_then(Value::as_mapping)
    {
        collect_named_type_definitions(binding_types, &mut definitions);
    }

    let external = external_types(amendments, errors);
    let known: BTreeSet<_> = definitions
        .keys()
        .cloned()
        .chain(external.iter().cloned())
        .collect();
    let mut referenced = BTreeSet::new();
    for definition in definitions.values() {
        collect_type_references(definition, &known, &mut referenced);
    }
    for reference in referenced {
        if !known.contains(&reference) {
            push_error(
                errors,
                "binding_types",
                format!("undefined binding type {reference}"),
            );
        }
    }
    definitions.len()
}

fn collect_named_type_definitions<'a>(
    mapping: &'a Mapping,
    definitions: &mut BTreeMap<String, &'a Value>,
) {
    for (name, definition) in mapping {
        if let Some(name) = name
            .as_str()
            .filter(|name| name.chars().next().is_some_and(char::is_uppercase))
        {
            definitions.insert(name.to_owned(), definition);
        }
    }
}

fn external_types(amendments: &Value, errors: &mut Vec<ContractError>) -> BTreeSet<String> {
    let mut external = BTreeSet::new();
    let result = mapping_value(amendments, "binding_type_additions")
        .and_then(|additions| mapping_value(additions, "external_types"));
    let Ok(groups) = result else {
        push_error(
            errors,
            "04-amendments.yaml",
            "binding_type_additions.external_types must be a mapping",
        );
        return external;
    };
    let Some(groups) = groups.as_mapping() else {
        push_error(
            errors,
            "04-amendments.yaml",
            "binding_type_additions.external_types must be a mapping",
        );
        return external;
    };
    for values in groups.values() {
        let Some(values) = values.as_sequence() else {
            push_error(
                errors,
                "04-amendments.yaml",
                "each external_types group must be a sequence",
            );
            continue;
        };
        for value in values {
            if let Some(name) = value.as_str() {
                external.insert(name.rsplit("::").next().unwrap_or(name).to_owned());
            } else {
                push_error(
                    errors,
                    "04-amendments.yaml",
                    "external type names must be strings",
                );
            }
        }
    }
    external
}

fn collect_type_references(
    definition: &Value,
    known: &BTreeSet<String>,
    referenced: &mut BTreeSet<String>,
) {
    match definition {
        Value::Mapping(mapping) => {
            for (field, specification) in mapping {
                let Some(field) = field.as_str() else {
                    continue;
                };
                if matches!(
                    field,
                    "constraint"
                        | "constraints"
                        | "purity"
                        | "invariant"
                        | "parse_rule"
                        | "match_rule"
                        | "rule"
                        | "note"
                ) {
                    continue;
                }
                if field == "enum" {
                    collect_enum_references(specification, known, referenced);
                } else if let Some(type_expression) = specification.as_str() {
                    collect_expression_types(type_expression, known, referenced);
                }
            }
        }
        Value::String(specification) if specification.trim_start().starts_with("enum [") => {
            collect_enum_string_references(specification, known, referenced);
        }
        Value::String(specification)
            if specification.trim_start().starts_with('{') || known.contains(specification) =>
        {
            collect_expression_types(specification, known, referenced);
        }
        _ => {}
    }
}

fn collect_enum_references(
    specification: &Value,
    known: &BTreeSet<String>,
    referenced: &mut BTreeSet<String>,
) {
    if let Some(variants) = specification.as_sequence() {
        for variant in variants.iter().filter_map(Value::as_str) {
            collect_variant_payload_types(variant, known, referenced);
        }
    }
}

fn collect_enum_string_references(
    specification: &str,
    known: &BTreeSet<String>,
    referenced: &mut BTreeSet<String>,
) {
    let body = specification
        .trim()
        .strip_prefix("enum [")
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or_default();
    for variant in body.split(',') {
        collect_variant_payload_types(variant.trim(), known, referenced);
    }
}

fn collect_variant_payload_types(
    variant: &str,
    known: &BTreeSet<String>,
    referenced: &mut BTreeSet<String>,
) {
    if let Some(open) = variant.find('(')
        && let Some(close) = variant.rfind(')')
    {
        collect_expression_types(&variant[open + 1..close], known, referenced);
    } else if let Some(open) = variant.find('{')
        && let Some(close) = variant.rfind('}')
    {
        collect_expression_types(&variant[open + 1..close], known, referenced);
    }
}

fn collect_expression_types(
    expression: &str,
    known: &BTreeSet<String>,
    referenced: &mut BTreeSet<String>,
) {
    let token =
        Regex::new(r"(?:[a-z][a-z0-9_]*::)?[A-Z][A-Za-z0-9_]*|\b(?:bool|u8|u16|u32|u64|i32)\b")
            .expect("valid regex");
    for found in token.find_iter(expression) {
        let name = found.as_str().rsplit("::").next().unwrap_or_default();
        if known.contains(name) || name.chars().next().is_some_and(char::is_uppercase) {
            referenced.insert(name.to_owned());
        }
    }
}

fn validate_source_archive(contract_dir: &Path, errors: &mut Vec<ContractError>) {
    let path = contract_dir.join(SOURCE_ARCHIVE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            push_error(
                errors,
                SOURCE_ARCHIVE,
                format!("cannot read source archive: {error}"),
            );
            return;
        }
    };
    let digest = Sha256::digest(bytes);
    let mut actual = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(actual, "{byte:02x}").expect("writing to a String cannot fail");
    }
    if actual != SOURCE_ARCHIVE_SHA256 {
        push_error(
            errors,
            SOURCE_ARCHIVE,
            format!("SHA-256 mismatch: expected {SOURCE_ARCHIVE_SHA256}, found {actual}"),
        );
    }
}

fn mapping_value<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value
        .as_mapping()
        .and_then(|mapping| mapping.get(Value::String(key.to_owned())))
        .ok_or_else(|| format!("missing mapping key {key:?}"))
}

fn push_error(
    errors: &mut Vec<ContractError>,
    location: impl Into<String>,
    message: impl Into<String>,
) {
    errors.push(ContractError {
        location: location.into(),
        message: message.into(),
    });
}
