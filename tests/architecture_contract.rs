use rdp_tui::config::contract::validate_architecture_contract;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const CONTRACT_FILES: [&str; 6] = [
    "00-manifest.yaml",
    "01-types.yaml",
    "02-rules.yaml",
    "03-process.yaml",
    "04-amendments.yaml",
    "rdp-tui-architecture-contract-source.zip",
];

fn repository_contract_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/architecture")
}

fn copied_contract() -> TempDir {
    let temporary = TempDir::new().expect("create temporary contract directory");
    for name in CONTRACT_FILES {
        fs::copy(
            repository_contract_dir().join(name),
            temporary.path().join(name),
        )
        .expect("copy contract fixture");
    }
    temporary
}

fn replace_in_file(path: &Path, from: &str, to: &str) {
    let original = fs::read_to_string(path).expect("read contract fixture");
    assert!(original.contains(from), "fixture does not contain {from:?}");
    fs::write(path, original.replacen(from, to, 1)).expect("write contract fixture");
}

#[test]
fn repository_contract_is_valid() {
    let report = validate_architecture_contract(repository_contract_dir()).expect("valid contract");
    assert_eq!(report.yaml_documents, 5);
    assert_eq!(report.modules, 13);
    assert_eq!(report.stable_ids, 49);
    assert_eq!(report.binding_types, 44);
}

#[test]
fn yaml_duplicate_keys_are_rejected() {
    let contract = copied_contract();
    let path = contract.path().join("00-manifest.yaml");
    let mut text = fs::read_to_string(&path).expect("read fixture");
    text.push_str("\nschema_version: 1\n");
    fs::write(path, text).expect("write fixture");

    let error = validate_architecture_contract(contract.path()).expect_err("duplicate must fail");
    assert!(error.to_string().contains("duplicate entry"));
}

#[test]
fn unsupported_schema_versions_are_rejected() {
    let contract = copied_contract();
    replace_in_file(
        &contract.path().join("03-process.yaml"),
        "schema_version: 1",
        "schema_version: 2",
    );

    let error = validate_architecture_contract(contract.path()).expect_err("version must fail");
    assert!(error.to_string().contains("unsupported schema_version 2"));
}

#[test]
fn module_cycles_are_rejected() {
    let contract = copied_contract();
    replace_in_file(
        &contract.path().join("04-amendments.yaml"),
        "session: [model, planner, credentials",
        "session: [session, model, planner, credentials",
    );

    let error = validate_architecture_contract(contract.path()).expect_err("cycle must fail");
    assert!(error.to_string().contains("dependency cycle"));
}

#[test]
fn forbidden_module_edges_are_rejected() {
    let contract = copied_contract();
    replace_in_file(
        &contract.path().join("00-manifest.yaml"),
        "model: []",
        "model: [runtime]",
    );

    let error =
        validate_architecture_contract(contract.path()).expect_err("forbidden edge must fail");
    assert!(
        error
            .to_string()
            .contains("forbidden dependency edge: model -> runtime")
    );
}

#[test]
fn unresolved_stable_ids_are_rejected() {
    let contract = copied_contract();
    let path = contract.path().join("03-process.yaml");
    let mut text = fs::read_to_string(&path).expect("read fixture");
    text.push_str("\ninvalid_reference: INV-does-not-exist\n");
    fs::write(path, text).expect("write fixture");

    let error = validate_architecture_contract(contract.path()).expect_err("reference must fail");
    assert!(
        error
            .to_string()
            .contains("unresolved stable identifier INV-does-not-exist")
    );
}

#[test]
fn undefined_binding_types_are_rejected() {
    let contract = copied_contract();
    replace_in_file(
        &contract.path().join("01-types.yaml"),
        "    name: String",
        "    name: MissingType",
    );

    let error = validate_architecture_contract(contract.path()).expect_err("type must fail");
    assert!(
        error
            .to_string()
            .contains("undefined binding type MissingType")
    );
}

#[test]
fn changed_source_archive_is_rejected() {
    let contract = copied_contract();
    let path = contract
        .path()
        .join("rdp-tui-architecture-contract-source.zip");
    let mut bytes = fs::read(&path).expect("read fixture");
    bytes.push(0);
    fs::write(path, bytes).expect("write fixture");

    let error = validate_architecture_contract(contract.path()).expect_err("hash must fail");
    assert!(error.to_string().contains("SHA-256 mismatch"));
}
